//! Rendering assertions against an in-memory `TestBackend` buffer.
//!
//! These catch the bugs that "looks fine on my terminal" misses: truncation,
//! sort order, empty states and panics at awkward sizes.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use herdash::app::{App, ConnState, SummariesMode};
use herdash::herdr::types::Snapshot;
use herdash::summary::AgentSummary;
use herdash::ui::{fmt_age, wrap_to};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;

const SNAPSHOT_FIXTURE: &str = include_str!("fixtures/snapshot.json");

fn fixture() -> Snapshot {
    let v: serde_json::Value = serde_json::from_str(SNAPSHOT_FIXTURE).unwrap();
    serde_json::from_value(v["result"]["snapshot"].clone()).unwrap()
}

fn app() -> App {
    let mut a = App::new(SummariesMode::On);
    a.apply_snapshot(&fixture());
    a
}

fn buffer_lines(buf: &Buffer) -> Vec<String> {
    (0..buf.area.height)
        .map(|y| {
            (0..buf.area.width)
                .map(|x| buf.cell((x, y)).map(|c| c.symbol()).unwrap_or(" "))
                .collect::<String>()
        })
        .collect()
}

fn lines_of(app: &App, w: u16, h: u16) -> Vec<String> {
    let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
    term.draw(|f| herdash::ui::draw(f, app)).unwrap();
    buffer_lines(&term.backend().buffer().clone())
}

fn render(app: &App, w: u16, h: u16) -> String {
    lines_of(app, w, h).join("\n")
}

fn summary() -> AgentSummary {
    AgentSummary {
        headline: "Verifying doc-accuracy findings".into(),
        task: "Verify two documentation findings from an external review".into(),
        now: "Running git show to judge the recommendation".into(),
        recent: vec![
            "Confirmed the first finding".into(),
            "Applied the comment fix".into(),
        ],
        needs_attention: false,
        attention_reason: String::new(),
    }
}

// ---------------------------------------------------------------- helpers --

#[test]
fn wrap_breaks_on_words_and_respects_the_line_budget() {
    let out = wrap_to("the quick brown fox jumps over the lazy dog", 12, 2);
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|l| l.chars().count() <= 12), "{out:?}");
    assert!(out[0].starts_with("the quick"));
}

#[test]
fn wrap_ellipsizes_when_the_text_exceeds_the_budget() {
    let out = wrap_to("aaaa bbbb cccc dddd eeee ffff", 10, 2);
    assert_eq!(out.len(), 2);
    assert!(out[1].ends_with('…'));
}

#[test]
fn wrap_breaks_words_longer_than_the_width() {
    let out = wrap_to("supercalifragilistic", 8, 2);
    assert!(out.iter().all(|l| l.chars().count() <= 8), "{out:?}");
}

#[test]
fn wrap_of_empty_text_yields_nothing() {
    assert!(wrap_to("   ", 10, 2).is_empty());
    assert!(wrap_to("text", 0, 2).is_empty());
    assert!(wrap_to("text", 10, 0).is_empty());
}

#[test]
fn wrap_never_splits_a_multibyte_character() {
    let out = wrap_to(&"日本語".repeat(20), 7, 2);
    assert!(
        out.iter()
            .all(|l| std::str::from_utf8(l.as_bytes()).is_ok())
    );
    assert!(out.iter().all(|l| l.chars().count() <= 7), "{out:?}");
}

#[test]
fn ages_render_compactly() {
    assert_eq!(fmt_age(Duration::from_secs(5), false), "5s");
    assert_eq!(fmt_age(Duration::from_secs(90), false), "1m");
    assert_eq!(fmt_age(Duration::from_secs(3600), false), "1h");
    assert_eq!(fmt_age(Duration::from_secs(90000), false), "1d");
}

#[test]
fn lower_bound_ages_are_marked_with_a_tilde() {
    assert_eq!(fmt_age(Duration::from_secs(3600), true), "~1h");
}

// ---------------------------------------------------------------- sidebar --

#[test]
fn the_sidebar_shows_a_header_for_every_repo_group() {
    let text = render(&app(), 120, 40);
    assert!(text.contains("alpha"), "named repo group header");
    assert!(text.contains("beta"), "named repo group header");
    assert!(
        text.contains("ungrouped"),
        "fallback for workspaces with no worktree"
    );
}

#[test]
fn the_sidebar_shows_the_workspace_label_for_each_agent() {
    let text = render(&app(), 120, 40);
    assert!(text.contains("feat-beta"));
    assert!(text.contains("feat-alpha"));
}

#[test]
fn the_blocked_agent_is_rendered_above_everything_else() {
    let lines = lines_of(&app(), 120, 40);
    let beta_at = lines.iter().position(|l| l.contains("feat-beta")).unwrap();
    let alpha_at = lines.iter().position(|l| l.contains("feat-alpha")).unwrap();
    assert!(
        beta_at < alpha_at,
        "urgency drives order:\n{}",
        lines.join("\n")
    );
}

#[test]
fn without_a_summary_the_second_line_falls_back_to_the_terminal_title() {
    let text = render(&app(), 120, 40);
    assert!(
        text.contains("beta migration"),
        "terminal title keeps the row useful"
    );
}

#[test]
fn a_summary_headline_replaces_the_terminal_title() {
    let mut a = app();
    a.slots.entry("w3:p1".into()).or_default().summary = Some(summary());
    let text = render(&a, 120, 40);
    assert!(text.contains("Verifying doc-accuracy"));
    assert!(
        !text.contains("beta migration"),
        "the headline supersedes the raw title"
    );
}

#[test]
fn an_in_flight_summary_shows_progress() {
    let mut a = app();
    a.slots.entry("w3:p1".into()).or_default().state.in_flight = true;
    assert!(render(&a, 120, 40).contains("summarizing"));
}

/// The indicator animates off the redraw tick, so rendering stays a pure
/// function of state rather than reading a clock.
#[test]
fn the_in_flight_indicator_animates_with_the_tick() {
    let mut a = app();
    a.slots.entry("w3:p1".into()).or_default().state.in_flight = true;
    let frames: std::collections::HashSet<char> = (0..10)
        .map(|t| {
            a.tick = t;
            let text = render(&a, 120, 40);
            let line = text
                .lines()
                .find(|l| l.contains("summarizing"))
                .unwrap()
                .to_string();
            line.trim_start().chars().next().unwrap()
        })
        .collect();
    assert!(
        frames.len() > 1,
        "expected distinct spinner frames, got {frames:?}"
    );
}

#[test]
fn the_header_distinguishes_no_key_from_an_explicit_flag() {
    let mut no_key = App::new(SummariesMode::OffNoKey);
    no_key.apply_snapshot(&fixture());
    assert!(render(&no_key, 140, 40).contains("summaries off (no key)"));

    let mut by_flag = App::new(SummariesMode::OffByFlag);
    by_flag.apply_snapshot(&fixture());
    let text = render(&by_flag, 140, 40);
    assert!(text.contains("summaries off"));
    assert!(
        !text.contains("(no key)"),
        "the flag case must not blame a missing key"
    );
}

#[test]
fn a_failed_summary_is_surfaced_without_killing_the_row() {
    let mut a = app();
    a.slots.entry("w3:p1".into()).or_default().error = Some("429 rate limited".into());
    let text = render(&a, 120, 40);
    assert!(text.contains("summary unavailable"));
    assert!(text.contains("feat-beta"), "the agent row still renders");
}

/// Only the sidebar's own columns are examined — the detail pane on the right
/// legitimately has room for a long label, and matching the whole line would
/// silently test the wrong pane.
fn sidebar_columns(app: &App, w: u16, h: u16) -> Vec<String> {
    lines_of(app, w, h)
        .iter()
        .map(|l| {
            l.chars()
                .take(herdash::ui::theme::SIDEBAR_WIDTH as usize)
                .collect()
        })
        .collect()
}

#[test]
fn a_long_label_is_truncated_rather_than_overflowing_the_sidebar() {
    let mut snap = fixture();
    snap.workspaces[2].label = "an-extremely-long-workspace-label-that-cannot-possibly-fit".into();
    let mut a = App::new(SummariesMode::On);
    a.apply_snapshot(&snap);
    let cols = sidebar_columns(&a, 120, 40);

    let row = cols
        .iter()
        .find(|l| l.contains("an-extremely-long"))
        .unwrap_or_else(|| panic!("label row missing from sidebar:\n{}", cols.join("\n")));
    assert!(row.contains('…'), "expected an ellipsis in: {row:?}");
    assert!(
        row.chars().count() <= herdash::ui::theme::SIDEBAR_WIDTH as usize,
        "sidebar content must not bleed past its column budget"
    );
}

/// The right border of the sidebar must stay a solid vertical rule; if any row
/// overflowed, the divider would be punched out at that line.
#[test]
fn the_sidebar_border_is_never_overwritten_by_content() {
    let mut snap = fixture();
    for w in &mut snap.workspaces {
        w.label = "x".repeat(120);
    }
    let mut a = App::new(SummariesMode::On);
    a.apply_snapshot(&snap);
    let lines = lines_of(&a, 120, 40);
    let border_col = (herdash::ui::theme::SIDEBAR_WIDTH - 1) as usize;
    // Skip the header rows and the footer, which span the full width.
    for line in lines.iter().skip(1).take(lines.len().saturating_sub(2)) {
        let ch = line.chars().nth(border_col).unwrap();
        assert!(
            ch == '\u{2502}' || ch == ' ',
            "border column corrupted by {ch:?} in {line:?}"
        );
    }
}

#[test]
fn an_empty_fleet_renders_an_explanatory_message() {
    assert!(render(&App::new(SummariesMode::On), 120, 40).contains("No agents"));
}

#[test]
fn filtering_everything_out_explains_how_to_get_back() {
    let mut snap = fixture();
    snap.agents.retain(|a| a.pane_id == "w2:p1"); // the idle one
    let mut a = App::new(SummariesMode::On);
    a.apply_snapshot(&snap);
    a.on_key(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
    let text = render(&a, 120, 40);
    assert!(text.contains("No active agents"), "got:\n{text}");
}

#[test]
fn a_long_selection_list_scrolls_to_keep_the_selection_visible() {
    let mut a = app();
    a.on_key(KeyEvent::new(KeyCode::Char('G'), KeyModifiers::NONE));
    let last = a.selected_agent().unwrap().label.clone();
    let text = render(&a, 120, 14);
    assert!(
        text.contains(&last),
        "selection scrolled out of view:\n{text}"
    );
}

// ----------------------------------------------------------------- header --

#[test]
fn the_header_reports_status_counts() {
    let text = render(&app(), 140, 40);
    assert!(text.contains("5 agents"));
    assert!(text.contains("1 working"));
    assert!(text.contains("1 blocked"));
}

#[test]
fn the_header_shows_the_fleet_summary_when_present() {
    let mut a = app();
    a.fleet_summary = Some("Two repos are active; one agent needs approval.".into());
    assert!(render(&a, 140, 40).contains("Two repos are active"));
}

#[test]
fn the_header_flags_disabled_summaries() {
    let mut a = App::new(SummariesMode::OffNoKey);
    a.apply_snapshot(&fixture());
    assert!(render(&a, 140, 40).contains("summaries off"));
}

#[test]
fn the_header_flags_a_dropped_connection() {
    let mut a = app();
    a.conn = ConnState::Reconnecting {
        since: Instant::now(),
    };
    assert!(render(&a, 140, 40).contains("reconnecting"));
}

#[test]
fn a_stale_fleet_still_renders_while_reconnecting() {
    let mut a = app();
    a.conn = ConnState::Reconnecting {
        since: Instant::now(),
    };
    assert!(
        render(&a, 140, 40).contains("feat-beta"),
        "last known state must stay on screen"
    );
}

// ----------------------------------------------------------------- detail --

#[test]
fn the_detail_pane_shows_the_three_summary_facets() {
    let mut a = app();
    let id = a.selected.clone().unwrap();
    a.slots.entry(id).or_default().summary = Some(summary());
    let text = render(&a, 140, 40);
    assert!(text.contains("TASK"));
    assert!(text.contains("NOW"));
    assert!(text.contains("RECENT"));
    assert!(text.contains("Verify two documentation findings"));
    assert!(text.contains("Running git show"));
    assert!(text.contains("Confirmed the first finding"));
}

#[test]
fn the_detail_pane_identifies_the_agent() {
    let text = render(&app(), 140, 40);
    assert!(text.contains("feat-beta"), "label");
    assert!(text.contains("w3:p1"), "pane id");
    assert!(text.contains("codex"), "agent kind");
    assert!(text.contains("blocked"), "status");
}

#[test]
fn empty_summary_fields_render_as_an_em_dash() {
    let mut a = app();
    let id = a.selected.clone().unwrap();
    a.slots.entry(id).or_default().summary = Some(AgentSummary {
        headline: "h".into(),
        task: String::new(),
        now: String::new(),
        recent: vec![],
        needs_attention: false,
        attention_reason: String::new(),
    });
    assert!(render(&a, 140, 40).contains("—"));
}

#[test]
fn the_detail_pane_explains_itself_before_a_summary_arrives() {
    assert!(render(&app(), 140, 40).contains("No summary yet"));
}

#[test]
fn the_detail_pane_explains_when_summaries_are_off() {
    let mut a = App::new(SummariesMode::OffNoKey);
    a.apply_snapshot(&fixture());
    assert!(render(&a, 140, 40).contains("Summaries are off"));
}

#[test]
fn the_detail_pane_surfaces_a_summary_error() {
    let mut a = app();
    let id = a.selected.clone().unwrap();
    a.slots.entry(id).or_default().error = Some("429 rate limited".into());
    assert!(render(&a, 140, 40).contains("429 rate limited"));
}

// --------------------------------------------------------- footer / modal --

#[test]
fn the_footer_lists_the_keybindings() {
    let text = render(&app(), 140, 40);
    assert!(text.contains("focus"));
    assert!(text.contains("quit"));
}

#[test]
fn the_help_overlay_documents_every_binding() {
    let mut a = app();
    a.show_help = true;
    let text = render(&a, 140, 40);
    for hint in [
        "select an agent",
        "focus pane in herdr",
        "resummarize",
        "active-only",
        "quit",
    ] {
        assert!(text.contains(hint), "help missing `{hint}`:\n{text}");
    }
}

// ------------------------------------------------------------- responsive --

#[test]
fn narrow_terminals_hide_the_detail_pane() {
    let mut a = app();
    let id = a.selected.clone().unwrap();
    a.slots.entry(id).or_default().summary = Some(summary());
    let narrow = render(&a, 80, 30);
    assert!(
        !narrow.contains("TASK"),
        "detail is hidden below 100 columns"
    );
    assert!(narrow.contains("feat-beta"), "the sidebar still renders");

    a.detail_open = true;
    assert!(
        render(&a, 80, 30).contains("TASK"),
        "→ opens detail full-width"
    );
}

#[test]
fn short_terminals_collapse_the_fleet_summary() {
    let mut a = app();
    a.fleet_summary = Some("A very long fleet summary that would otherwise eat the screen".into());
    let count = |t: &str| t.lines().filter(|l| l.contains("FLEET")).count();
    assert_eq!(count(&render(&a, 140, 40)), 1);
    let short = render(&a, 140, 12);
    assert_eq!(count(&short), 1);
    assert!(
        short.contains("feat-beta"),
        "agents still fit on a short screen"
    );
}

#[test]
fn rendering_never_panics_at_awkward_sizes() {
    let mut a = app();
    let id = a.selected.clone().unwrap();
    a.slots.entry(id).or_default().summary = Some(summary());
    a.fleet_summary = Some("fleet".into());
    for (w, h) in [
        (1u16, 1u16),
        (2, 2),
        (20, 5),
        (40, 10),
        (99, 24),
        (100, 24),
        (140, 4),
        (300, 80),
    ] {
        let _ = render(&a, w, h);
    }
    a.show_help = true;
    for (w, h) in [(1u16, 1u16), (20, 5), (140, 40)] {
        let _ = render(&a, w, h);
    }
}

/// Column math must use display width, not character count. CJK and emoji are
/// double-width, so an 11-character label occupies 22 columns — which silently
/// pushed the workspace id and age off the right edge of the sidebar.
#[test]
fn wide_characters_do_not_push_the_id_and_age_off_the_sidebar() {
    let mut snap = fixture();
    snap.workspaces[2].label = "日本語のリポジトリ名前".into();
    let mut a = App::new(SummariesMode::On);
    a.apply_snapshot(&snap);

    let row = lines_of(&a, 120, 40)
        .into_iter()
        .map(|l| l.chars().take(38).collect::<String>())
        .find(|l| l.contains('日'))
        .expect("the wide-character label must render");

    assert!(row.contains("w3"), "workspace id survived: {row:?}");
    assert!(
        row.contains("~0s") || row.contains("0s"),
        "age survived: {row:?}"
    );
}

#[test]
fn truncation_measures_display_columns_not_characters() {
    use herdash::ui::{display_width, truncate_to_width};
    assert_eq!(
        display_width("日本語"),
        6,
        "three CJK glyphs occupy six columns"
    );
    assert_eq!(display_width("abc"), 3);

    let t = truncate_to_width("日本語のテキスト", 7);
    assert!(
        display_width(&t) <= 7,
        "{t:?} is {} columns",
        display_width(&t)
    );
    assert!(t.ends_with('…'));
    assert!("日本語のテキスト".starts_with(t.trim_end_matches('…')));

    assert_eq!(
        truncate_to_width("short", 20),
        "short",
        "untouched when it fits"
    );
    assert_eq!(truncate_to_width("anything", 0), "");
}

#[test]
fn wrapping_measures_display_columns_and_never_splits_a_glyph() {
    let out = wrap_to("日本語のテキストです ここも日本語", 10, 3);
    assert!(!out.is_empty());
    for line in &out {
        assert!(
            herdash::ui::display_width(line) <= 10,
            "{line:?} is {} columns",
            herdash::ui::display_width(line)
        );
        assert!(std::str::from_utf8(line.as_bytes()).is_ok());
    }
}

/// A width narrower than a single glyph must not loop forever.
#[test]
fn wrapping_terminates_when_the_width_is_narrower_than_one_glyph() {
    let out = wrap_to("日本語", 1, 3);
    assert!(out.len() <= 3);
}
