//! Keybinding hints and the modal help overlay.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

use super::theme;
use crate::app::App;

const HINTS: &str = " ↑↓ select  ⏎ focus pane  r resummarise  R all  a active-only  ? keys  q quit";

/// Every binding, as (keys, description). Shared by the footer and the overlay
/// so the two can never drift apart.
pub const BINDINGS: &[(&str, &str)] = &[
    ("↑ ↓ / k j", "select an agent"),
    ("g / G", "first / last agent"),
    ("⏎", "focus pane in herdr"),
    ("r", "resummarise the selected agent"),
    ("R", "resummarise every agent"),
    ("a", "toggle active-only (hide idle and unknown)"),
    ("→ / ←", "open / close detail on narrow terminals"),
    ("?", "toggle this help"),
    ("q / Ctrl-C", "quit"),
];

pub fn render(frame: &mut Frame, area: Rect, _app: &App) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    frame.render_widget(Paragraph::new(HINTS).style(theme::dim()), area);
}

pub fn render_help(frame: &mut Frame, area: Rect) {
    let width = 52u16.min(area.width);
    let height = (BINDINGS.len() as u16 + 2).min(area.height);
    if width < 12 || height < 4 {
        return;
    }
    let popup = Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    };
    let lines: Vec<Line> = BINDINGS
        .iter()
        .map(|(keys, desc)| {
            Line::from(vec![
                Span::styled(format!(" {keys:<12}"), theme::heading()),
                Span::styled((*desc).to_string(), theme::label()),
            ])
        })
        .collect();
    frame.render_widget(Clear, popup);
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(" keys ")
                .border_style(theme::dim()),
        ),
        popup,
    );
}
