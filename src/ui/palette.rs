//! Color palette, inherited from the terminal and optionally from herdr.
//!
//! # What can and cannot be inherited
//!
//! herdr's `[theme]` (catppuccin, rose-pine, …) styles **herdr's own chrome** —
//! its sidebar, borders and status bar. It does not repaint a pane's output,
//! and herdr publishes no palette over its socket API: all 91 methods were
//! checked, and there is no theme or color surface. The built-in palettes are
//! computed in herdr's code rather than stored as literals, so they cannot be
//! read out of the binary either.
//!
//! So herdash does the two things that are actually available:
//!
//! 1. **Inherit the terminal.** Body text uses [`Color::Reset`] and no
//!    background is painted, so herdash adopts whatever foreground, background
//!    and ANSI palette the terminal already uses — which is what makes it look
//!    native beside herdr.
//! 2. **Honor `[theme.custom]`.** herdr's own override mechanism accepts hex,
//!    named and `rgb()` colors. Anything set there themes herdr *and* herdash
//!    identically, which is the one way to make the two match exactly.

use std::path::Path;
use std::sync::OnceLock;

use ratatui::style::Color;

/// Colors herdash draws with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    /// Section headings.
    pub accent: Color,
    /// Blocked agents and anything wanting the user.
    pub alert: Color,
    /// Finished work.
    pub ok: Color,
    /// Active work.
    pub busy: Color,
    /// Secondary text, rules, chrome.
    pub muted: Color,
    /// Body text. `Reset` means "whatever the terminal's foreground is".
    pub text: Color,
    /// Background for the selected row, when the theme names one.
    pub selection: Option<Color>,
}

impl Default for Palette {
    /// ANSI-named defaults, which the terminal maps through its own theme.
    ///
    /// Deliberately not literal RGB: naming `Color::Red` lets a rose-pine or
    /// gruvbox terminal render *its* red, where `Color::Rgb(255, 0, 0)` would
    /// override the user's palette with a color from nowhere.
    fn default() -> Self {
        Self {
            accent: Color::Cyan,
            alert: Color::Red,
            ok: Color::Green,
            busy: Color::Yellow,
            muted: Color::DarkGray,
            text: Color::Reset,
            selection: None,
        }
    }
}

impl Palette {
    /// Overlay any `[theme.custom]` tokens found in a herdr config file.
    ///
    /// Missing file, unreadable file, malformed TOML and unknown color syntax
    /// are all non-events: theming must never stop the dashboard starting.
    pub fn from_herdr_config(path: &Path) -> Self {
        let mut palette = Self::default();
        let Ok(text) = std::fs::read_to_string(path) else {
            return palette;
        };
        // `toml::Value` parses a bare value; a whole document needs `Table`.
        let Ok(document) = text.parse::<toml::Table>() else {
            return palette;
        };
        let Some(custom) = document.get("theme").and_then(|t| t.get("custom")) else {
            return palette;
        };
        let token = |name: &str| {
            custom
                .get(name)
                .and_then(|v| v.as_str())
                .and_then(parse_color)
        };

        if let Some(c) = token("accent") {
            palette.accent = c;
        }
        if let Some(c) = token("red") {
            palette.alert = c;
        }
        if let Some(c) = token("green") {
            palette.ok = c;
        }
        if let Some(c) = token("yellow") {
            palette.busy = c;
        }
        if let Some(c) = token("selection_bg") {
            palette.selection = Some(c);
        }
        palette
    }

    /// Default location of herdr's config, honoring `HERDR_CONFIG_PATH`.
    pub fn herdr_config_path(home: &Path) -> std::path::PathBuf {
        std::env::var_os("HERDR_CONFIG_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| home.join(".config").join("herdr").join("config.toml"))
    }
}

/// Parse the color syntax herdr's `[theme.custom]` accepts.
pub fn parse_color(raw: &str) -> Option<Color> {
    let s = raw.trim();
    if s.eq_ignore_ascii_case("reset") || s.eq_ignore_ascii_case("none") {
        return Some(Color::Reset);
    }
    if let Some(hex) = s.strip_prefix('#') {
        return match hex.len() {
            6 => {
                let n = u32::from_str_radix(hex, 16).ok()?;
                Some(Color::Rgb((n >> 16) as u8, (n >> 8) as u8, n as u8))
            }
            // #rgb shorthand, each digit doubled.
            3 => {
                let mut bytes = hex.chars().map(|c| {
                    let v = c.to_digit(16)? as u8;
                    Some(v * 17)
                });
                Some(Color::Rgb(bytes.next()??, bytes.next()??, bytes.next()??))
            }
            _ => None,
        };
    }
    if let Some(inner) = s.strip_prefix("rgb(").and_then(|v| v.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
        if parts.len() == 3 {
            return Some(Color::Rgb(
                parts[0].parse().ok()?,
                parts[1].parse().ok()?,
                parts[2].parse().ok()?,
            ));
        }
        return None;
    }
    match s.to_ascii_lowercase().as_str() {
        "black" => Some(Color::Black),
        "red" => Some(Color::Red),
        "green" => Some(Color::Green),
        "yellow" => Some(Color::Yellow),
        "blue" => Some(Color::Blue),
        "magenta" | "purple" => Some(Color::Magenta),
        "cyan" => Some(Color::Cyan),
        "white" => Some(Color::White),
        "gray" | "grey" => Some(Color::Gray),
        "darkgray" | "darkgrey" => Some(Color::DarkGray),
        _ => None,
    }
}

static PALETTE: OnceLock<Palette> = OnceLock::new();

/// Install the palette for the process. Ignored if already set.
pub fn init(palette: Palette) {
    let _ = PALETTE.set(palette);
}

/// The active palette, ANSI defaults until [`init`] is called.
pub fn active() -> &'static Palette {
    PALETTE.get_or_init(Palette::default)
}
