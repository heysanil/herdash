//! The agent list: repo group headers, each with its agents and a one-line
//! summary headline underneath.

use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::{display_width, fmt_age, theme, truncate_to_width, wrap_to};
use crate::app::{App, Row};
use crate::fleet::Agent;

/// Lines a headline may occupy under an agent row.
const HEADLINE_LINES: usize = 2;

/// Braille spinner frames for the in-flight indicator.
const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// The sidebar's line buffer, plus which agent owns each line.
///
/// Built once and used by both rendering and mouse hit-testing, so a click
/// can never resolve to a different row than the one drawn.
struct Rendered {
    lines: Vec<Line<'static>>,
    /// Parallel to `lines`: the pane id each line belongs to, if any.
    owners: Vec<Option<String>>,
    selected_start: Option<usize>,
    selected_end: Option<usize>,
}

fn inner_area(area: Rect, show_divider: bool) -> Rect {
    if show_divider {
        Block::default().borders(Borders::RIGHT).inner(area)
    } else {
        area
    }
}

fn build(app: &App, width: usize, now: Instant) -> Rendered {
    let mut lines: Vec<Line> = Vec::new();
    let mut owners: Vec<Option<String>> = Vec::new();
    let mut selected_start: Option<usize> = None;
    let mut selected_end: Option<usize> = None;

    let push = |line: Line<'static>,
                    owner: Option<String>,
                    lines: &mut Vec<Line<'static>>,
                    owners: &mut Vec<Option<String>>| {
        lines.push(line);
        owners.push(owner);
    };

    for row in app.rows() {
        match row {
            Row::AttentionHeader(count) => {
                push(
                    attention_header(count, width),
                    None,
                    &mut lines,
                    &mut owners,
                );
            }
            Row::Group(g, shown) => {
                if !lines.is_empty() {
                    push(Line::from(""), None, &mut lines, &mut owners);
                }
                push(
                    group_header(g.name(), shown, width),
                    None,
                    &mut lines,
                    &mut owners,
                );
            }
            Row::Agent(a) => {
                let is_selected = app.selected.as_deref() == Some(a.pane_id.as_str());
                if is_selected {
                    selected_start = Some(lines.len());
                }
                let id = Some(a.pane_id.clone());
                push(
                    agent_row(a, width, now, is_selected),
                    id.clone(),
                    &mut lines,
                    &mut owners,
                );
                // An agent waiting on the user leads with what it wants,
                // because that is the only thing the reader needs to act on.
                if let Some(reason) = app.attention_reason(a) {
                    for (i, l) in wrap_to(reason, width.saturating_sub(4), 2)
                        .into_iter()
                        .enumerate()
                    {
                        let prefix = if i == 0 { "  → " } else { "    " };
                        let line = Line::from(Span::styled(format!("{prefix}{l}"), theme::alert()));
                        push(line, id.clone(), &mut lines, &mut owners);
                    }
                } else {
                    for l in secondary(app, a, width.saturating_sub(2)) {
                        let line = Line::from(Span::styled(format!("  {l}"), theme::dim()));
                        push(line, id.clone(), &mut lines, &mut owners);
                    }
                }
                if is_selected {
                    selected_end = Some(lines.len());
                }
            }
        }
    }
    Rendered {
        lines,
        owners,
        selected_start,
        selected_end,
    }
}

pub fn render(frame: &mut Frame, area: Rect, app: &App, show_divider: bool) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let inner = inner_area(area, show_divider);
    if show_divider {
        let block = Block::default()
            .borders(Borders::RIGHT)
            .border_style(theme::dim());
        frame.render_widget(block, area);
    }
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

    let r = build(app, inner.width as usize, Instant::now());
    let scroll = scroll_offset(
        r.selected_start,
        r.selected_end,
        r.lines.len(),
        inner.height as usize,
    );
    frame.render_widget(Paragraph::new(r.lines).scroll((scroll as u16, 0)), inner);
}

/// Which agent, if any, sits under this screen position.
pub fn agent_at(
    app: &App,
    area: Rect,
    show_divider: bool,
    column: u16,
    row: u16,
) -> Option<String> {
    if app.groups.is_empty() {
        return None;
    }
    let inner = inner_area(area, show_divider);
    if column < inner.x || column >= inner.x.saturating_add(inner.width) {
        return None;
    }
    if row < inner.y || row >= inner.y.saturating_add(inner.height) {
        return None;
    }
    let r = build(app, inner.width as usize, Instant::now());
    let scroll = scroll_offset(
        r.selected_start,
        r.selected_end,
        r.lines.len(),
        inner.height as usize,
    );
    let index = (row - inner.y) as usize + scroll;
    r.owners.get(index).cloned().flatten()
}

/// `⚠ waiting on you ───────── 2`
fn attention_header(count: usize, width: usize) -> Line<'static> {
    let label = "⚠ waiting on you";
    let count_s = count.to_string();
    let used = display_width(label) + display_width(&count_s) + 2;
    let rule = "─".repeat(width.saturating_sub(used).max(1));
    Line::from(vec![
        Span::styled(label, theme::alert().add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {rule} "), theme::dim()),
        Span::styled(count_s, theme::alert()),
    ])
}

/// `alpha ─────────────── 2`
fn group_header(name: &str, count: usize, width: usize) -> Line<'static> {
    let count_s = count.to_string();
    // Reserve the count and at least a stub rule, then fit the name into
    // whatever remains — a long repo name must not clip the count away.
    let reserved = display_width(&count_s) + 4;
    let name = truncate_to_width(name, width.saturating_sub(reserved).max(1));
    let used = display_width(&name) + display_width(&count_s) + 2;
    let rule = "─".repeat(width.saturating_sub(used).max(1));
    Line::from(vec![
        Span::styled(name, theme::heading()),
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
    // glyph + space, then label, then " {ws}  {age}". Measured in display
    // columns: a CJK or emoji label is twice as wide as its character count,
    // and counting characters pushes the id and age off the right edge.
    let fixed = 2 + 1 + display_width(&ws) + 2 + display_width(&age);
    let label_budget = width.saturating_sub(fixed).max(1);
    let label = truncate_to_width(&a.label, label_budget);
    let pad = label_budget.saturating_sub(display_width(&label));

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
            // Keep the last good headline visible; the error annotates it
            // rather than replacing it, because stale detail beats none.
            return match &slot.summary {
                Some(summary) => {
                    let mut out = wrap_to(&summary.headline, width, 1);
                    out.extend(wrap_to(&format!("⚠ {err}"), width, 1));
                    out
                }
                None => wrap_to(
                    &format!("⚠ summary unavailable — {err}"),
                    width,
                    HEADLINE_LINES,
                ),
            };
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
