//! herdash — terminal dashboard for herdr agent fleets.
//!
//! This binary is deliberately thin. It owns the terminal, multiplexes three
//! input sources with `select!`, and performs the I/O that
//! [`herdash::orchestrator`] describes. Every non-trivial state transition
//! lives in the library, where it is tested.

use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use crossterm::event::{Event, EventStream, KeyEventKind};
use futures_util::StreamExt;
use tokio::sync::mpsc;

use herdash::app::{Action, App, ConnState};
use herdash::config::{Cli, Settings};
use herdash::herdr::client::Client;
use herdash::orchestrator::{self, FLEET_COOLDOWN, FleetJob, FleetRequest, SummaryJob, Update};
use herdash::summary::Summarizer;
use herdash::summary::openrouter::OpenRouter;
use herdash::summary::policy::Cfg;
use herdash::ui;

/// Redraw cadence, so ages and the spinner stay live.
const TICK: Duration = Duration::from_millis(120);

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let settings = Settings::from_cli(&cli);
    let client = Client::new(settings.socket.clone());

    // Prove herdr is reachable before taking over the terminal, so the error
    // stays readable instead of flashing past an alternate screen.
    client.snapshot().await.with_context(|| {
        format!(
            "herdash could not reach herdr at {}.\n\
             Is the server running? Try `herdr status server`, or start it with `herdr server`.",
            settings.socket.display()
        )
    })?;

    let terminal = ratatui::init();
    install_panic_hook();
    let result = run(terminal, settings, client).await;
    ratatui::restore();
    result
}

/// Restore the terminal before a panic prints, so a crash never leaves a
/// broken tty behind.
fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        previous(info);
    }));
}

async fn run(
    mut terminal: ratatui::DefaultTerminal,
    settings: Settings,
    client: Client,
) -> Result<()> {
    let (tx, mut rx) = mpsc::channel::<Update>(128);
    tokio::spawn(poll_snapshots(
        client.clone(),
        settings.interval,
        tx.clone(),
    ));

    let summarizer: Option<Arc<dyn Summarizer>> = settings
        .api_key
        .clone()
        .map(|key| Arc::new(OpenRouter::new(key, settings.model.clone())) as Arc<dyn Summarizer>);

    let mut app = App::new(settings.summaries);
    let mut fleet_job = FleetJob::default();
    let cfg = Cfg {
        cooldown: settings.cooldown,
        enabled: settings.summaries_enabled(),
    };
    let mut events = EventStream::new();
    let mut ticker = tokio::time::interval(TICK);

    loop {
        terminal.draw(|frame| ui::draw(frame, &app))?;

        tokio::select! {
            maybe_event = events.next() => match maybe_event {
                Some(Ok(Event::Key(key))) if key.kind == KeyEventKind::Press => {
                    match app.on_key(key) {
                        Action::Quit => break,
                        Action::Focus(pane_id) => {
                            let client = client.clone();
                            let tx = tx.clone();
                            tokio::spawn(async move {
                                if let Err(err) = client.focus_agent(&pane_id).await {
                                    let _ = tx.send(Update::Notice(format!("focus failed: {err}"))).await;
                                }
                            });
                        }
                        Action::ForceOne(pane_id) => orchestrator::force_one(&mut app, &pane_id),
                        Action::ForceAll => orchestrator::force_all(&mut app),
                        Action::None => {}
                    }
                }
                Some(Ok(_)) => {}
                Some(Err(err)) => app.notice = Some(format!("input error: {err}")),
                None => break,
            },
            Some(update) = rx.recv() => orchestrator::apply_update(&mut app, &mut fleet_job, update),
            _ = ticker.tick() => app.tick = app.tick.wrapping_add(1),
        }

        if app.should_quit {
            break;
        }

        if let Some(summarizer) = &summarizer {
            let now = Instant::now();
            for job in orchestrator::plan_summaries(&mut app, now, &cfg) {
                tokio::spawn(summarize_agent(
                    client.clone(),
                    Arc::clone(summarizer),
                    job,
                    settings.lines,
                    tx.clone(),
                ));
            }
            if let Some(req) = orchestrator::plan_fleet(&app, &mut fleet_job, now, FLEET_COOLDOWN) {
                tokio::spawn(summarize_fleet(Arc::clone(summarizer), req, tx.clone()));
            }
        }
    }
    Ok(())
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
    // defeat the "only summarise when output actually moved" rule entirely.
    let result = match client.read_agent(&job.pane_id, lines).await {
        Ok(read) if read.text.trim().is_empty() => {
            Err("pane produced no output to summarise".to_string())
        }
        Ok(read) => summarizer
            .summarize_agent(&read.text)
            .await
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
