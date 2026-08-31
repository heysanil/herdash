//! Rendering. Every function here is a pure projection of [`App`] state onto
//! a ratatui frame — no I/O, no mutation.

pub mod detail;
pub mod footer;
pub mod header;
pub mod sidebar;
pub mod theme;

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use unicode_width::UnicodeWidthStr;

use crate::app::App;

/// Terminal columns a string occupies.
///
/// Not the same as `chars().count()`: CJK and emoji are double-width, so a
/// label of 11 characters can consume 22 columns. Counting characters silently
/// pushed the workspace id and age off the right edge of the sidebar.
pub fn display_width(s: &str) -> usize {
    UnicodeWidthStr::width(s)
}

/// Truncate to at most `max` display columns, appending `…` when cut.
///
/// Never splits a character, and accounts for the ellipsis itself.
pub fn truncate_to_width(s: &str, max: usize) -> String {
    if display_width(s) <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    // Reserve one column for the ellipsis.
    let budget = max - 1;
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let w = display_width(ch.encode_utf8(&mut [0u8; 4]));
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    out = out.trim_end().to_string();
    out.push('…');
    out
}

/// Compose the whole frame.
/// Where each pane sits in the frame.
///
/// Computed by one function so rendering and mouse hit-testing can never
/// disagree about which pixel belongs to which pane.
#[derive(Debug, Clone, Copy)]
pub struct Panes {
    pub header: Rect,
    /// `None` when a narrow terminal has the detail view open instead.
    pub sidebar: Option<Rect>,
    pub detail: Option<Rect>,
    pub footer: Rect,
}

/// Split the frame. Pure and deterministic in `App` state plus size.
pub fn layout(app: &App, area: Rect) -> Panes {
    let header_height = header::height(app, area.height).min(area.height);
    let footer_height = if area.height > header_height { 1 } else { 0 };

    let rows = Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Min(0),
        Constraint::Length(footer_height),
    ])
    .split(area);

    if area.width < theme::NARROW_COLS {
        // Narrow terminals show one pane at a time.
        if app.detail_open {
            Panes {
                header: rows[0],
                sidebar: None,
                detail: Some(rows[1]),
                footer: rows[2],
            }
        } else {
            Panes {
                header: rows[0],
                sidebar: Some(rows[1]),
                detail: None,
                footer: rows[2],
            }
        }
    } else {
        let cols =
            Layout::horizontal([Constraint::Length(theme::SIDEBAR_WIDTH), Constraint::Min(0)])
                .split(rows[1]);
        Panes {
            header: rows[0],
            sidebar: Some(cols[0]),
            detail: Some(cols[1]),
            footer: rows[2],
        }
    }
}

/// Compose the whole frame.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let panes = layout(app, area);
    header::render(frame, panes.header, app);
    if let Some(sidebar) = panes.sidebar {
        // The divider only earns its column when a detail pane sits beside it.
        sidebar::render(frame, sidebar, app, panes.detail.is_some());
    }
    if let Some(detail) = panes.detail {
        detail::render(frame, detail, app);
    }
    footer::render(frame, panes.footer, app);

    if app.show_help {
        footer::render_help(frame, area);
    }
}

/// What sits under a mouse position, if anything actionable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    /// The sidebar row for this pane id.
    Agent(String),
}

/// Resolve a mouse position to something the app can act on.
pub fn hit_test(app: &App, area: Rect, column: u16, row: u16) -> Option<Hit> {
    if area.width == 0 || area.height == 0 {
        return None;
    }
    let panes = layout(app, area);
    let sidebar = panes.sidebar?;
    sidebar::agent_at(app, sidebar, panes.detail.is_some(), column, row).map(Hit::Agent)
}

/// Greedy word wrap into at most `max_lines` lines of `width` characters,
/// ellipsising the final line when text remains. Words longer than `width`
/// are hard-broken rather than overflowing.
pub fn wrap_to(text: &str, width: usize, max_lines: usize) -> Vec<String> {
    if width == 0 || max_lines == 0 {
        return Vec::new();
    }
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let mut word = word.to_string();
        loop {
            let candidate_len = if current.is_empty() {
                display_width(&word)
            } else {
                display_width(&current) + 1 + display_width(&word)
            };
            if candidate_len <= width {
                if !current.is_empty() {
                    current.push(' ');
                }
                current.push_str(&word);
                break;
            }
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                if lines.len() == max_lines {
                    return ellipsize_last(lines, width, true);
                }
                continue;
            }
            // A single word wider than the line: hard-break it on a column
            // boundary, so a double-width glyph is never split in half.
            let mut head = String::new();
            let mut used = 0usize;
            let mut split_at = 0usize;
            for (i, ch) in word.char_indices() {
                let w = display_width(ch.encode_utf8(&mut [0u8; 4]));
                if used + w > width {
                    break;
                }
                head.push(ch);
                used += w;
                split_at = i + ch.len_utf8();
            }
            if head.is_empty() {
                // `width` is narrower than a single glyph; emit one and move on
                // rather than looping forever.
                let mut it = word.chars();
                if let Some(ch) = it.next() {
                    head.push(ch);
                    split_at = ch.len_utf8();
                } else {
                    break;
                }
            }
            let tail: String = word[split_at..].to_string();
            lines.push(head);
            if lines.len() == max_lines {
                return ellipsize_last(lines, width, !tail.is_empty());
            }
            word = tail;
            if word.is_empty() {
                break;
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.len() > max_lines {
        lines.truncate(max_lines);
        return ellipsize_last(lines, width, true);
    }
    lines
}

/// Replace the final line with an ellipsised version that fits `width`
/// columns and always ends in `…`.
///
/// Unlike [`truncate_to_width`], this appends the ellipsis unconditionally:
/// the caller only reaches here because text was dropped, and a line that
/// silently ends mid-thought reads as complete.
fn ellipsize_last(mut lines: Vec<String>, width: usize, truncated: bool) -> Vec<String> {
    if truncated && let Some(last) = lines.last_mut() {
        *last = ellipsize_to_width(last, width);
    }
    lines
}

fn ellipsize_to_width(s: &str, width: usize) -> String {
    if width == 0 {
        return String::new();
    }
    if width == 1 {
        return "…".to_string();
    }
    // One column is reserved for the ellipsis itself.
    let budget = width - 1;
    let mut out = String::new();
    let mut used = 0usize;
    for ch in s.chars() {
        let w = display_width(ch.encode_utf8(&mut [0u8; 4]));
        if used + w > budget {
            break;
        }
        out.push(ch);
        used += w;
    }
    let mut out = out.trim_end().to_string();
    out.push('…');
    out
}

/// Compact duration: `5s`, `1m`, `1h`, `1d`. A `~` prefix marks a lower bound.
pub fn fmt_age(elapsed: Duration, lower_bound: bool) -> String {
    let s = elapsed.as_secs();
    let body = if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else if s < 86_400 {
        format!("{}h", s / 3600)
    } else {
        format!("{}d", s / 86_400)
    };
    if lower_bound {
        format!("~{body}")
    } else {
        body
    }
}
