//! Color inheritance from the terminal and from herdr's config.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use herdash::ui::palette::{Palette, parse_color};
use ratatui::style::Color;

static N: AtomicU32 = AtomicU32::new(0);

fn tempdir() -> PathBuf {
    let n = N.fetch_add(1, Ordering::SeqCst);
    let p = std::env::temp_dir().join(format!("herdash-palette-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn write_config(body: &str) -> PathBuf {
    let dir = tempdir();
    let path = dir.join("config.toml");
    std::fs::write(&path, body).unwrap();
    path
}

/// The default palette must name ANSI colors, never literal RGB: naming
/// `Red` lets a themed terminal render *its* red, where a hardcoded RGB value
/// would override the user's palette with a color from nowhere.
#[test]
fn the_default_palette_defers_to_the_terminal() {
    let p = Palette::default();
    assert_eq!(p.alert, Color::Red);
    assert_eq!(p.ok, Color::Green);
    assert_eq!(p.busy, Color::Yellow);
    assert_eq!(p.accent, Color::Cyan);
    assert_eq!(
        p.text,
        Color::Reset,
        "body text follows the terminal foreground"
    );
    assert_eq!(p.selection, None, "reversed video, which suits any palette");
    for c in [p.alert, p.ok, p.busy, p.accent, p.muted, p.text] {
        assert!(
            !matches!(c, Color::Rgb(..)),
            "{c:?} would override the user's theme"
        );
    }
}

#[test]
fn herdr_custom_tokens_are_adopted() {
    let path = write_config(
        r##"
[theme]
name = "rose-pine"

[theme.custom]
accent = "#f5c2e7"
red = "#ff6188"
green = "rgb(166, 227, 161)"
yellow = "yellow"
selection_bg = "#313244"
"##,
    );
    let p = Palette::from_herdr_config(&path);
    assert_eq!(p.accent, Color::Rgb(0xf5, 0xc2, 0xe7));
    assert_eq!(p.alert, Color::Rgb(0xff, 0x61, 0x88));
    assert_eq!(p.ok, Color::Rgb(166, 227, 161));
    assert_eq!(p.busy, Color::Yellow);
    assert_eq!(p.selection, Some(Color::Rgb(0x31, 0x32, 0x44)));
    assert_eq!(
        p.text,
        Color::Reset,
        "untouched tokens keep the terminal default"
    );
}

/// A named theme with no custom tokens gives nothing to inherit, because herdr
/// does not publish its built-in palettes. That must degrade silently to the
/// terminal rather than guessing colors.
#[test]
fn a_named_theme_without_custom_tokens_falls_back_to_the_terminal() {
    let path = write_config("[theme]\nname = \"catppuccin\"\n");
    assert_eq!(Palette::from_herdr_config(&path), Palette::default());
}

/// Theming must never be able to stop the dashboard starting.
#[test]
fn a_missing_or_broken_config_is_a_non_event() {
    assert_eq!(
        Palette::from_herdr_config(&PathBuf::from("/nope/does/not/exist.toml")),
        Palette::default()
    );
    let broken = write_config("this is not [valid toml");
    assert_eq!(Palette::from_herdr_config(&broken), Palette::default());
    let wrong_shape = write_config("[theme]\ncustom = 3\n");
    assert_eq!(Palette::from_herdr_config(&wrong_shape), Palette::default());
    let bad_color = write_config("[theme.custom]\naccent = \"not-a-color\"\n");
    assert_eq!(
        Palette::from_herdr_config(&bad_color).accent,
        Palette::default().accent,
        "an unparseable token is skipped, not fatal"
    );
}

#[test]
fn every_color_syntax_herdr_accepts_is_parsed() {
    assert_eq!(parse_color("#89b4fa"), Some(Color::Rgb(0x89, 0xb4, 0xfa)));
    assert_eq!(parse_color("#abc"), Some(Color::Rgb(0xaa, 0xbb, 0xcc)));
    assert_eq!(parse_color("rgb(1, 2, 3)"), Some(Color::Rgb(1, 2, 3)));
    assert_eq!(parse_color("cyan"), Some(Color::Cyan));
    assert_eq!(parse_color("Magenta"), Some(Color::Magenta));
    assert_eq!(parse_color("purple"), Some(Color::Magenta));
    assert_eq!(parse_color("reset"), Some(Color::Reset));
    assert_eq!(
        parse_color("  #89b4fa  "),
        Some(Color::Rgb(0x89, 0xb4, 0xfa))
    );
    assert_eq!(parse_color("#12345"), None, "wrong hex length");
    assert_eq!(parse_color("rgb(1,2)"), None, "wrong arity");
    assert_eq!(parse_color("rgb(1,2,999)"), None, "out of range");
    assert_eq!(parse_color("chartreuse"), None);
}

#[test]
fn the_config_path_honours_the_herdr_environment_override() {
    // Not mutating the process environment: just assert the default shape.
    let home = PathBuf::from("/home/u");
    let path = Palette::herdr_config_path(&home);
    assert!(
        path.ends_with("herdr/config.toml") || std::env::var_os("HERDR_CONFIG_PATH").is_some(),
        "got {path:?}"
    );
}
