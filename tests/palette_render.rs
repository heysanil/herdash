//! Proves custom palette tokens actually reach the rendered buffer.
//!
//! This file exists because a live run found `heading()` still hardcoding
//! `Color::Cyan` while every other style had been converted. The palette unit
//! tests all passed — they tested the parser, not the paint. Only rendering
//! and inspecting real cell styles catches a style that forgot to ask.
//!
//! It is a separate test binary on purpose: the palette is a process-wide
//! `OnceLock`, so installing a custom one here cannot affect other suites.

use herdash::app::{App, SummariesMode};
use herdash::herdr::types::Snapshot;
use herdash::ui::palette::{self, Palette};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::style::Color;

const SNAPSHOT_FIXTURE: &str = include_str!("fixtures/snapshot.json");

const ACCENT: Color = Color::Rgb(0xc4, 0xa7, 0xe7);
const ALERT: Color = Color::Rgb(0xeb, 0x6f, 0x92);
const OK: Color = Color::Rgb(0x31, 0x74, 0x8f);
const BUSY: Color = Color::Rgb(0xf6, 0xc1, 0x77);

fn install() {
    palette::init(Palette {
        accent: ACCENT,
        alert: ALERT,
        ok: OK,
        busy: BUSY,
        muted: Color::DarkGray,
        text: Color::Reset,
        selection: None,
    });
}

fn fixture() -> Snapshot {
    let v: serde_json::Value = serde_json::from_str(SNAPSHOT_FIXTURE).unwrap();
    serde_json::from_value(v["result"]["snapshot"].clone()).unwrap()
}

/// Every distinct foreground colour present in the rendered frame.
fn rendered_colors(app: &App) -> Vec<Color> {
    let mut term = Terminal::new(TestBackend::new(140, 40)).unwrap();
    term.draw(|f| herdash::ui::draw(f, app)).unwrap();
    let buf = term.backend().buffer().clone();
    let mut seen = Vec::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if let Some(cell) = buf.cell((x, y))
                && !seen.contains(&cell.fg)
            {
                seen.push(cell.fg);
            }
        }
    }
    seen
}

#[test]
fn custom_palette_tokens_reach_the_rendered_frame() {
    install();
    let mut app = App::new(SummariesMode::On);
    app.apply_snapshot(&fixture());
    let colors = rendered_colors(&app);

    assert!(
        colors.contains(&ACCENT),
        "no style used the accent token — a heading is probably still hardcoded.\nsaw: {colors:?}"
    );
    assert!(
        colors.contains(&ALERT),
        "the blocked agent must use the alert token: {colors:?}"
    );
    assert!(
        colors.contains(&BUSY),
        "the working agent must use the busy token: {colors:?}"
    );
}

/// The real regression guard: nothing may paint the ANSI defaults the palette
/// was supposed to replace.
#[test]
fn no_style_falls_back_to_a_hardcoded_ansi_colour() {
    install();
    let mut app = App::new(SummariesMode::On);
    app.apply_snapshot(&fixture());
    let colors = rendered_colors(&app);

    for forbidden in [
        Color::Cyan,
        Color::Red,
        Color::Yellow,
        Color::Green,
        Color::White,
    ] {
        assert!(
            !colors.contains(&forbidden),
            "{forbidden:?} is hardcoded somewhere; it should come from the palette.\nsaw: {colors:?}"
        );
    }
}
