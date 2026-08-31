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

/// Braille spinner frames for the in-flight indicator.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

pub fn render(frame: &mut Frame, area: Rect, app: &App) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(theme::dim());
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
    // The selected agent occupies a *block* — its identity row plus headline
    // rows — and scrolling must keep the identity row itself visible.
    let mut selected_start: Option<usize> = None;
    let mut selected_end: Option<usize> = None;

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
                    selected_start = Some(lines.len());
                }
                lines.push(agent_row(a, width, now, is_selected));
                for l in secondary(app, a, width.saturating_sub(2)) {
                    lines.push(Line::from(Span::styled(format!("  {l}"), theme::dim())));
                }
                if is_selected {
                    selected_end = Some(lines.len());
                }
            }
        }
    }

    let scroll = scroll_offset(
        selected_start,
        selected_end,
        lines.len(),
        inner.height as usize,
    );
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
    let age = fmt_age(
        now.saturating_duration_since(a.status_since),
        a.age_is_lower_bound,
    );
    let ws = a.workspace_id.clone();
    // glyph + space, then label, then " {ws}  {age}"
    let fixed = 2 + 1 + ws.chars().count() + 2 + age.chars().count();
    let label_budget = width.saturating_sub(fixed).max(1);
    let label = truncate_chars(&a.label, label_budget);
    let pad = label_budget.saturating_sub(label.chars().count());

    let base: Style = if selected {
        theme::selected()
    } else {
        Style::default()
    };
    Line::from(vec![
        Span::styled(
            format!("{} ", theme::glyph(a.status)),
            theme::status_style(a.status).patch(base),
        ),
        Span::styled(
            format!("{label}{}", " ".repeat(pad)),
            theme::label().patch(base),
        ),
        Span::styled(format!(" {ws}  {age}"), theme::dim().patch(base)),
    ])
}

/// The dim line(s) under an agent: headline, progress, error, or raw title.
fn secondary(app: &App, a: &Agent, width: usize) -> Vec<String> {
    if let Some(slot) = app.slots.get(&a.pane_id) {
        if let Some(err) = &slot.error {
            return wrap_to(
                &format!("⚠ summary unavailable — {err}"),
                width,
                HEADLINE_LINES,
            );
        }
        if slot.state.in_flight {
            let frame = SPINNER[(app.tick as usize) % SPINNER.len()];
            return vec![format!("{frame} summarising…")];
        }
        if let Some(s) = &slot.summary {
            return wrap_to(&s.headline, width, HEADLINE_LINES);
        }
    }
    // No summary yet, or summaries are disabled entirely: the terminal title
    // still tells the user what the pane is.
    wrap_to(&a.title, width, 1)
}

/// Scroll just enough to keep the selected block on screen.
///
/// Reserving room for the headline rows must never push the identity row
/// above the viewport, which is what `.min(start)` guarantees — on a one- or
/// two-row viewport the naive calculation scrolls the selection off entirely.
pub(crate) fn scroll_offset(
    selected_start: Option<usize>,
    selected_end: Option<usize>,
    total: usize,
    height: usize,
) -> usize {
    if height == 0 || total <= height {
        return 0;
    }
    let Some(start) = selected_start else {
        return 0;
    };
    let end = selected_end.unwrap_or(start + 1).min(total);
    end.saturating_sub(height)
        .min(start)
        .min(total.saturating_sub(height))
}

#[cfg(test)]
mod tests {
    use super::scroll_offset;

    #[test]
    fn no_scrolling_when_everything_fits() {
        assert_eq!(scroll_offset(Some(0), Some(3), 10, 20), 0);
        assert_eq!(scroll_offset(Some(5), Some(8), 10, 10), 0);
    }

    #[test]
    fn a_zero_height_viewport_is_handled() {
        assert_eq!(scroll_offset(Some(5), Some(8), 10, 0), 0);
    }

    #[test]
    fn nothing_selected_means_no_scrolling() {
        assert_eq!(scroll_offset(None, None, 100, 10), 0);
    }

    /// Regression: reserving room for the headline rows must never push the
    /// selected identity row above the viewport. A naive `end - height`
    /// scrolls the selection clean off a one- or two-row pane.
    #[test]
    fn the_selected_row_stays_visible_in_a_tiny_viewport() {
        for height in 1..=3usize {
            for start in 0..8usize {
                let offset = scroll_offset(Some(start), Some(start + 3), 40, height);
                assert!(
                    offset <= start,
                    "height={height} start={start} scrolled the selection off (offset={offset})"
                );
                assert!(offset + height <= 40);
            }
        }
    }

    #[test]
    fn scrolling_reveals_a_selection_below_the_fold() {
        // 40 rows of content, 10 visible, selection near the bottom.
        let offset = scroll_offset(Some(35), Some(38), 40, 10);
        assert!(offset <= 35 && offset + 10 > 35, "offset={offset}");
        assert!(offset <= 30, "must not scroll past the end");
    }

    #[test]
    fn the_offset_never_exceeds_the_last_full_page() {
        assert_eq!(scroll_offset(Some(39), Some(42), 40, 10), 30);
    }
}
