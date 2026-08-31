//! Rendering. Every function here is a pure projection of [`App`] state onto
//! a ratatui frame — no I/O, no mutation.

pub mod detail;
pub mod footer;
pub mod header;
pub mod sidebar;
pub mod theme;

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};

use crate::app::App;

/// Compose the whole frame.
pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width == 0 || area.height == 0 {
        return;
    }
    let header_height = header::height(app, area.height).min(area.height);
    let footer_height = if area.height > header_height { 1 } else { 0 };

    let rows = Layout::vertical([
        Constraint::Length(header_height),
        Constraint::Min(0),
        Constraint::Length(footer_height),
    ])
    .split(area);

    header::render(frame, rows[0], app);

    if area.width < theme::NARROW_COLS {
        // Narrow terminals show one pane at a time.
        if app.detail_open {
            detail::render(frame, rows[1], app);
        } else {
            sidebar::render(frame, rows[1], app);
        }
    } else {
        let cols =
            Layout::horizontal([Constraint::Length(theme::SIDEBAR_WIDTH), Constraint::Min(0)])
                .split(rows[1]);
        sidebar::render(frame, cols[0], app);
        detail::render(frame, cols[1], app);
    }

    footer::render(frame, rows[2], app);

    if app.show_help {
        footer::render_help(frame, area);
    }
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
                word.chars().count()
            } else {
                current.chars().count() + 1 + word.chars().count()
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
            // A single word wider than the line: hard-break it.
            let head: String = word.chars().take(width).collect();
            let tail: String = word.chars().skip(width).collect();
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

fn ellipsize_last(mut lines: Vec<String>, width: usize, truncated: bool) -> Vec<String> {
    if truncated && let Some(last) = lines.last_mut() {
        let keep = width.saturating_sub(1);
        let mut s: String = last.chars().take(keep).collect();
        s = s.trim_end().to_string();
        s.push('…');
        *last = s;
    }
    lines
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
