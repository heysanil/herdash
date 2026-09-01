//! herdash — terminal dashboard for herdr agent fleets.
//!
//! This binary is deliberately thin. It owns the terminal, multiplexes input
//! sources with `select!`, and performs the I/O that
//! [`herdash::orchestrator`] describes. Every non-trivial state transition
//! lives in the library, where it is tested.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, EventStream, KeyEventKind, MouseButton,
    MouseEvent, MouseEventKind,
};
use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio::task::JoinSet;

use herdash::app::{Action, App, ConnState};
use herdash::config::{Cli, Settings};
use herdash::herdr::client::Client;
use herdash::herdr::types::Snapshot;
use herdash::orchestrator::{self, FLEET_COOLDOWN, FleetJob, FleetRequest, SummaryJob, Update};
use herdash::summary::Summarizer;
use herdash::summary::openrouter::OpenRouter;
use herdash::summary::policy::Cfg;
use herdash::ui;

/// Redraw cadence, so ages and the spinner stay live.
const TICK: Duration = Duration::from_millis(120);

/// How long herdr keeps the sidebar token without a refresh.
///
/// Short enough that a herdash killed rather than closed disappears from the
/// sidebar promptly, long enough that a slow poll cannot make it flicker.
const SIDEBAR_TOKEN_TTL: Duration = Duration::from_secs(30);
/// Refresh well inside the TTL so one dropped call is not visible.
const SIDEBAR_TOKEN_REFRESH: Duration = Duration::from_secs(10);
/// Token name; surfaces in herdr as `$herdash`.
const SIDEBAR_TOKEN: &str = "herdash";

/// Ceiling on simultaneous summary calls.
///
/// Without a bound, a fifty-agent session would fire fifty OpenRouter requests
/// the moment it starts. Agents beyond the limit keep their latched triggers
/// and are picked up on a later pass, so nothing is dropped — only deferred.
const MAX_SUMMARY_TASKS: usize = 6;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let settings = Settings::from_cli(&cli);
    let client = Client::new(settings.socket.clone());

    // Prove herdr is reachable before taking over the terminal, so the error
    // stays readable instead of flashing past an alternate screen. The result
    // seeds the first frame rather than being thrown away.
    let initial = client.snapshot().await.with_context(|| {
        format!(
            "herdash could not reach herdr at {}.\n\
             Is the server running? Try `herdr status server`, or start it with `herdr server`.",
            settings.socket.display()
        )
    })?;

    // Install colors before the first frame so nothing flashes in the wrong
    // palette.
    herdash::ui::palette::init(settings.palette);

    let terminal = ratatui::init();
    install_panic_hook();
    if settings.mouse
        && let Err(err) = enable_mouse()
    {
        eprintln!("herdash: mouse capture unavailable: {err}");
    }
    let result = run(terminal, settings.clone(), client, initial).await;
    if settings.mouse {
        let _ = disable_mouse();
    }
    ratatui::restore();
    result
}

fn enable_mouse() -> std::io::Result<()> {
    crossterm::execute!(std::io::stdout(), EnableMouseCapture)
}

fn disable_mouse() -> std::io::Result<()> {
    crossterm::execute!(std::io::stdout(), DisableMouseCapture)
}

/// Restore the terminal before a panic prints, so a crash never leaves a
/// broken tty behind — mouse capture included, or the user's terminal keeps
/// emitting escape codes on every click.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_mouse();
        ratatui::restore();
        previous(info);
    }));
}

async fn run(
    mut terminal: ratatui::DefaultTerminal,
    settings: Settings,
    client: Client,
    initial: Snapshot,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<Update>(128);
    tokio::spawn(poll_snapshots(
        client.clone(),
        settings.interval,
        tx.clone(),
    ));

    // Name this space in herdr's sidebar for as long as herdash is running.
    if let Some(workspace) = settings.workspace_id.clone() {
        tokio::spawn(publish_sidebar_token(client.clone(), workspace));
    }

    let summarizer: Option<Arc<dyn Summarizer>> = settings
        .api_key
        .clone()
        .map(|key| Arc::new(OpenRouter::new(key, settings.model.clone())) as Arc<dyn Summarizer>);

    let mut app = App::new(settings.summaries);
    app.apply_snapshot(&initial);

    let mut fleet_job = FleetJob::default();
    let cfg = Cfg {
        cooldown: settings.cooldown,
        enabled: settings.summaries_enabled(),
    };
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(TICK);
    let mut summary_tasks: JoinSet<()> = JoinSet::new();
    let mut fleet_task: JoinSet<()> = JoinSet::new();
    let mut focus_task: JoinSet<()> = JoinSet::new();

    loop {
        terminal.draw(|frame| ui::draw(frame, &app))?;
        let area = terminal.get_frame().area();

        tokio::select! {
            maybe_event = events.next() => match maybe_event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                    match app.on_key(key) {
                        Action::Quit => break,
                        Action::Focus(pane_id) => {
                            spawn_focus(&mut focus_task, &client, &tx, pane_id);
                        }
                        Action::ForceOne(pane_id) => orchestrator::force_one(&mut app, &pane_id),
                        Action::ForceAll => orchestrator::force_all(&mut app),
                        Action::None => {}
                    }
                }
                Some(Ok(Event::Mouse(mouse))) => {
                    if let Some(pane_id) = on_mouse(&mut app, area, mouse) {
                        spawn_focus(&mut focus_task, &client, &tx, pane_id);
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(err)) => app.notice = Some(format!("input error: {err}")),
                None => break,
            },
            Some(update) = rx.recv() => orchestrator::apply_update(&mut app, &mut fleet_job, update),
            _ = ticker.tick() => app.tick = app.tick.wrapping_add(1),
            // Reap finished workers so the sets do not grow without bound.
            Some(_) = summary_tasks.join_next(), if !summary_tasks.is_empty() => {}
            Some(_) = fleet_task.join_next(), if !fleet_task.is_empty() => {}
            Some(_) = focus_task.join_next(), if !focus_task.is_empty() => {}
        }

        if app.should_quit {
            break;
        }

        if let Some(summarizer) = &summarizer {
            let now = Instant::now();
            let capacity = MAX_SUMMARY_TASKS.saturating_sub(summary_tasks.len());
            for job in orchestrator::plan_summaries(&mut app, now, &cfg, capacity) {
                summary_tasks.spawn(summarize_agent(
                    client.clone(),
                    Arc::clone(summarizer),
                    job,
                    settings.lines,
                    tx.clone(),
                ));
            }
            if let Some(req) =
                orchestrator::plan_fleet(&mut app, &mut fleet_job, now, FLEET_COOLDOWN)
            {
                fleet_task.spawn(summarize_fleet(Arc::clone(summarizer), req, tx.clone()));
            }
        }
    }
    // Best-effort: drop the sidebar token on a clean exit rather than making
    // the user wait out the TTL. The TTL remains the backstop for a hard kill.
    if let Some(workspace) = settings.workspace_id.as_deref() {
        let _ = client
            .report_workspace_token(workspace, SIDEBAR_TOKEN, None, Duration::from_secs(1))
            .await;
    }

    Ok(())
}

/// Keep a `$herdash` token alive on this workspace while the process runs.
///
/// herdr renders it wherever `ui.sidebar.spaces.rows` mentions `$herdash`,
/// alongside the repo and branch. Failures are deliberately silent: a
/// cosmetic sidebar entry must never interrupt the dashboard.
async fn publish_sidebar_token(client: Client, workspace_id: String) {
    let mut ticker = tokio::time::interval(SIDEBAR_TOKEN_REFRESH);
    loop {
        ticker.tick().await;
        let _ = client
            .report_workspace_token(
                &workspace_id,
                SIDEBAR_TOKEN,
                Some(SIDEBAR_TOKEN),
                SIDEBAR_TOKEN_TTL,
            )
            .await;
    }
}

/// Translate a mouse event, returning a pane id when the click asks to focus.
///
/// A first click selects; clicking the already-selected agent focuses it in
/// herdr. That keeps a stray click from yanking the user's view away.
fn on_mouse(app: &mut App, area: ratatui::layout::Rect, mouse: MouseEvent) -> Option<String> {
    match mouse.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let ui::Hit::Agent(pane_id) = ui::hit_test(app, area, mouse.column, mouse.row)?;
            if app.selected.as_deref() == Some(pane_id.as_str()) {
                return Some(pane_id);
            }
            app.select(&pane_id);
            None
        }
        MouseEventKind::ScrollDown => {
            app.scroll_selection(1);
            None
        }
        MouseEventKind::ScrollUp => {
            app.scroll_selection(-1);
            None
        }
        _ => None,
    }
}

fn spawn_focus(
    tasks: &mut JoinSet<()>,
    client: &Client,
    tx: &mpsc::Sender<Update>,
    pane_id: String,
) {
    let client = client.clone();
    let tx = tx.clone();
    tasks.spawn(async move {
        if let Err(err) = client.focus_agent(&pane_id).await {
            let _ = tx
                .send(Update::Notice(format!("focus failed: {err}")))
                .await;
        }
    });
}

/// Poll `session.snapshot` forever.
///
/// Each call opens its own connection (herdr closes the socket after one
/// response), so "reconnecting" simply means the last attempt failed at the
/// transport level. A protocol-level failure means herdr is alive but
/// answering oddly, so it is reported as a notice and polling continues.
async fn poll_snapshots(client: Client, interval: Duration, tx: mpsc::Sender<Update>) {
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        ticker.tick().await;
        let update = match client.snapshot().await {
            Ok(snapshot) => Update::Snapshot(Box::new(snapshot)),
            Err(err) if err.is_transport() => Update::Connection(ConnState::Reconnecting {
                since: Instant::now(),
            }),
            Err(err) => Update::Notice(format!("herdr: {err}")),
        };
        if tx.send(update).await.is_err() {
            return;
        }
    }
}

async fn summarize_agent(
    client: Client,
    summarizer: Arc<dyn Summarizer>,
    job: SummaryJob,
    lines: u32,
    tx: mpsc::Sender<Update>,
) {
    // herdr always reports `revision: 0` on a read, so the snapshot revision
    // captured at dispatch is the only usable change signal. Recording the
    // read's value instead would make every agent look permanently changed and
    // defeat the "only summarize when output actually moved" rule entirely.
    let result = match client.read_agent(&job.pane_id, lines).await {
        // Nothing to describe is a normal state, not an error: reporting it as
        // a failure would re-read an empty pane on every backoff tick forever.
        Ok(read) if read.text.trim().is_empty() => Ok(None),
        Ok(read) => summarizer
            .summarize_agent(&read.text)
            .await
            .map(Some)
            .map_err(|e| e.to_string()),
        Err(err) => Err(err.to_string()),
    };
    let _ = tx
        .send(Update::Summary {
            pane_id: job.pane_id,
            revision: job.revision,
            result,
        })
        .await;
}

async fn summarize_fleet(
    summarizer: Arc<dyn Summarizer>,
    req: FleetRequest,
    tx: mpsc::Sender<Update>,
) {
    let result = summarizer
        .summarize_fleet(&req.headlines)
        .await
        .map_err(|e| e.to_string());
    let _ = tx
        .send(Update::Fleet {
            hash: req.hash,
            result,
        })
        .await;
}
