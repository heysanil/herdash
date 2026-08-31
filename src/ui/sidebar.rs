//! The agent list: repo group headers, each with its agents and a one-line
//! summary headline underneath.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::{fmt_age, theme, wrap_to};
use crate::app::{App, Row};
use crate::fleet::Agent;
use crate::summary::types::truncate_chars;

/// Lines a headline may occupy under an agent row.
const HEADLINE_LINES: usize = 2;

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let block = Block::default().borders(Borders::RIGHT).border_style(theme::dim());
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.width == 0 || inner.height == 0 {
        return;
    }

    if app.groups.is_empty() {
        let msg = if app.active_only {
            "No active agents.\nPress `a` to show idle ones."
        } else {
            "No agents.\nStart one in herdr and it will appear here."
        };
        frame.render_widget(Paragraph::new(msg).style(theme::dim()), inner);
        return;
    }

    let width = inner.width as usize;
    let now = Instant::now();
    let mut lines: Vec<Line> = Vec::new();
    let mut selected_line: Option<usize> = None;

    for row in app.rows() {
        match row {
            Row::Group(g) => {
                if !lines.is_empty() {
                    lines.push(Line::from(""));
                }
                lines.push(group_header(g.name(), g.agents.len(), width));
            }
            Row::Agent(a) => {
                let is_selected = app.selected.as_deref() == Some(a.pane_id.as_str());
                if is_selected {
                    selected_line = Some(lines.len());
                }
                lines.push(agent_row(a, width, now, is_selected));
                for l in secondary(app, a, width.saturating_sub(2)) {
                    lines.push(Line::from(Span::styled(format!("  {l}"), theme::dim())));
                }
            }
        }
    }

    let scroll = scroll_offset(selected_line, lines.len(), inner.height as usize);
    frame.render_widget(Paragraph::new(lines).scroll((scroll as u16, 0)), inner);
}

/// `alpha ─────────────── 2`
fn group_header(name: &str, count: usize, width: usize) -> Line<'static> {
    let count_s = count.to_string();
    let used = name.chars().count() + count_s.chars().count() + 2;
    let rule = "─".repeat(width.saturating_sub(used).max(1));
    Line::from(vec![
        Span::styled(name.to_string(), theme::heading()),
        Span::styled(format!(" {rule} "), theme::dim()),
        Span::styled(count_s, theme::dim()),
    ])
}

/// `● feat-alpha            w1  5m`
fn agent_row(a: &Agent, width: usize, now: Instant, selected: bool) -> Line<'static> {
    let age = fmt_age(now.saturating_duration_since(a.status_since), a.age_is_lower_bound);
    let ws = a.workspace_id.clone();
    // glyph + space, then label, then " {ws}  {age}"
    let fixed = 2 + 1 + ws.chars().count() + 2 + age.chars().count();
    let label_budget = width.saturating_sub(fixed).max(1);
    let label = truncate_chars(&a.label, label_budget);
    let pad = label_budget.saturating_sub(label.chars().count());

    let base: Style = if selected { theme::selected() } else { Style::default() };
    Line::from(vec![
        Span::styled(
            format!("{} ", theme::glyph(a.status)),
            theme::status_style(a.status).patch(base),
        ),
        Span::styled(format!("{label}{}", " ".repeat(pad)), theme::label().patch(base)),
        Span::styled(format!(" {ws}  {age}"), theme::dim().patch(base)),
    ])
}

/// The dim line(s) under an agent: headline, progress, error, or raw title.
fn secondary(app: &App, a: &Agent, width: usize) -> Vec<String> {
    if let Some(slot) = app.slots.get(&a.pane_id) {
        if let Some(err) = &slot.error {
            return wrap_to(&format!("⚠ summary unavailable — {err}"), width, HEADLINE_LINES);
        }
        if slot.state.in_flight {
            return vec!["summarising…".to_string()];
        }
        if let Some(s) = &slot.summary {
            return wrap_to(&s.headline, width, HEADLINE_LINES);
        }
    }
    // No summary yet, or summaries are disabled entirely: the terminal title
    // still tells the user what the pane is.
    wrap_to(&a.title, width, 1)
}

/// Scroll just enough to keep the selected row on screen.
fn scroll_offset(selected: Option<usize>, total: usize, height: usize) -> usize {
    if total <= height || height == 0 {
        return 0;
    }
    let Some(sel) = selected else { return 0 };
    // Keep the selection plus its headline lines visible.
    let want_end = (sel + HEADLINE_LINES + 1).min(total);
    want_end.saturating_sub(height).min(total.saturating_sub(height))
}
