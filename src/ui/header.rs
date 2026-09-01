//! Title row, status tally, connection state, and the fleet summary block.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{theme, wrap_to};
use crate::app::{App, ConnState, SummariesMode};

/// Header height: one title line, plus up to two fleet-summary lines when the
/// terminal is tall enough to spare them.
pub fn height(app: &App, total_rows: u16) -> u16 {
    if app.fleet_summary.is_none() {
        return 1;
    }
    if total_rows < theme::SHORT_ROWS { 2 } else { 3 }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let c = app.counts();
    let mut tally: Vec<String> = vec![format!("{} agents", c.total)];
    for (n, name) in [
        (c.blocked, "blocked"),
        (c.done, "done"),
        (c.working, "working"),
        (c.idle, "idle"),
        (c.unknown, "unknown"),
    ] {
        if n > 0 {
            tally.push(format!("{n} {name}"));
        }
    }

    let mut spans = vec![
        Span::styled("herdash ", theme::heading()),
        Span::styled(tally.join(" · "), theme::dim()),
    ];
    if let ConnState::Reconnecting { .. } = app.conn {
        spans.push(Span::styled("  ⟳ reconnecting", theme::alert()));
    }
    // A detail, when present, replaces the generic note with a specific one.
    // `note()` keeps its existing `&'static str` contract for the cases that
    // need no provider name, so `tests/app.rs`'s assertions still hold.
    let annotation = match (app.summaries, app.summaries_detail.as_deref()) {
        (SummariesMode::OnLocal, Some(p)) => Some(format!("summaries on · {p} (local)")),
        // Without a provider name there is still something worth saying: the
        // transcripts are not leaving the machine.
        (SummariesMode::OnLocal, None) => Some("summaries on · local".to_string()),
        (SummariesMode::OffNoKey, Some(v)) => Some(format!("summaries off (no key: {v})")),
        (mode, _) => mode.note().map(str::to_string),
    };
    if let Some(a) = annotation {
        spans.push(Span::styled(format!("  {a}"), theme::dim()));
    }
    if let Some(notice) = &app.notice {
        spans.push(Span::styled(format!("  {notice}"), theme::dim()));
    }

    let mut lines = vec![Line::from(spans)];
    if let Some(fleet) = &app.fleet_summary {
        let budget = (area.height as usize).saturating_sub(1);
        let width = area.width.saturating_sub(8) as usize;
        for (i, l) in wrap_to(fleet, width, budget).into_iter().enumerate() {
            let prefix = if i == 0 { "FLEET  " } else { "       " };
            lines.push(Line::from(vec![
                Span::styled(prefix, theme::dim()),
                Span::styled(l, theme::label()),
            ]));
        }
    }
    frame.render_widget(Paragraph::new(lines), area);
}
