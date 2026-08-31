//! Colours, glyphs and layout constants.
//!
//! Glyph and colour both encode one idea: how much does this want attention.

use ratatui::style::{Color, Modifier, Style};

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
    match status {
        AgentStatus::Blocked => Color::Red,
        AgentStatus::Done => Color::Green,
        AgentStatus::Working => Color::Yellow,
        AgentStatus::Idle => Color::Gray,
        AgentStatus::Unknown => Color::DarkGray,
    }
}

pub fn status_style(status: AgentStatus) -> Style {
    Style::default().fg(status_color(status))
}

pub fn dim() -> Style {
    Style::default().fg(Color::DarkGray)
}

pub fn heading() -> Style {
    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
}

pub fn selected() -> Style {
    Style::default().add_modifier(Modifier::REVERSED)
}

pub fn label() -> Style {
    Style::default().fg(Color::White)
}

pub fn alert() -> Style {
    Style::default().fg(Color::Red)
}
