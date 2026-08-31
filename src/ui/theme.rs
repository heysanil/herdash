//! Glyphs, layout constants, and styles built from the active palette.
//!
//! Glyph and color both encode one idea: how much does this want attention.
//! Colors come from [`super::palette`], which inherits the terminal and any
//! `[theme.custom]` tokens set in herdr's config.

use ratatui::style::{Color, Modifier, Style};

use super::palette::active;
use crate::herdr::types::AgentStatus;

/// Fixed sidebar width; the detail pane takes the remainder.
pub const SIDEBAR_WIDTH: u16 = 38;
/// Below this width the detail pane is hidden.
pub const NARROW_COLS: u16 = 100;
/// Below this height the fleet summary collapses to one line.
pub const SHORT_ROWS: u16 = 20;

pub fn glyph(status: AgentStatus) -> &'static str {
    match status {
        AgentStatus::Blocked => "⊘",
        AgentStatus::Done => "◆",
        AgentStatus::Working => "●",
        AgentStatus::Idle => "○",
        AgentStatus::Unknown => "?",
    }
}

pub fn status_color(status: AgentStatus) -> Color {
    let p = active();
    match status {
        AgentStatus::Blocked => p.alert,
        AgentStatus::Done => p.ok,
        AgentStatus::Working => p.busy,
        // Idle text is ordinary text, so it follows the terminal foreground
        // rather than being forced to a color of our choosing.
        AgentStatus::Idle => p.text,
        AgentStatus::Unknown => p.muted,
    }
}

pub fn status_style(status: AgentStatus) -> Style {
    Style::default().fg(status_color(status))
}

pub fn dim() -> Style {
    Style::default().fg(active().muted)
}

pub fn heading() -> Style {
    Style::default()
        .fg(active().accent)
        .add_modifier(Modifier::BOLD)
}

pub fn selected() -> Style {
    // Reversed video by default, which works against any terminal palette.
    // A theme that names a selection background gets that instead.
    match active().selection {
        Some(bg) => Style::default().bg(bg),
        None => Style::default().add_modifier(Modifier::REVERSED),
    }
}

/// Body text.
///
/// Defaults to [`Color::Reset`], i.e. the terminal's own foreground. Forcing
/// white here was the single biggest reason herdash looked foreign next to
/// herdr on a themed terminal.
pub fn label() -> Style {
    Style::default().fg(active().text)
}

pub fn alert() -> Style {
    Style::default().fg(active().alert)
}
