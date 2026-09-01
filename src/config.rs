//! Command-line interface and settings resolution.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Parser;

use crate::app::SummariesMode;

/// Where herdash takes its colors from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
pub enum ThemeSource {
    /// Terminal palette, overlaid with any `[theme.custom]` herdr tokens.
    Auto,
    /// Terminal palette only.
    Ansi,
}

/// Terminal dashboard for herdr agent fleets.
#[derive(Debug, Clone, Parser)]
#[command(name = "herdash", version, about)]
pub struct Cli {
    /// Seconds between herdr snapshot polls.
    #[arg(long, default_value_t = 1)]
    pub interval: u64,

    /// Minimum seconds between summaries for the same agent.
    #[arg(long, default_value_t = 45)]
    pub cooldown: u64,

    /// OpenRouter model slug used for summaries.
    ///
    /// The default was chosen by `examples/bench.rs`; see `docs/benchmark.md`.
    /// It is the only model measured at 100% accuracy on the attention
    /// classification while also scoring in the top tier for prose quality.
    #[arg(long, default_value = "openai/gpt-oss-120b")]
    pub model: String,

    /// Transcript lines requested from each agent.
    #[arg(long, default_value_t = 200)]
    pub lines: u32,

    /// Run as a pure status board with no external network egress.
    #[arg(long)]
    pub no_summaries: bool,

    /// Path to the herdr socket. Overrides $HERDR_SOCKET_PATH.
    #[arg(long)]
    pub socket: Option<PathBuf>,

    /// Disable mouse capture, restoring your terminal's own text selection.
    #[arg(long)]
    pub no_mouse: bool,

    /// Color source. `auto` picks up `[theme.custom]` tokens from herdr's
    /// config; `ansi` uses terminal-palette colors only.
    #[arg(long, value_enum, default_value_t = ThemeSource::Auto)]
    pub theme: ThemeSource,

    /// Do not publish a `$herdash` token to herdr's sidebar.
    #[arg(long)]
    pub no_sidebar_token: bool,
}

/// The user's home directory, falling back to `.` if it cannot be determined.
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Resolve the herdr socket: `--socket`, then `$HERDR_SOCKET_PATH`, then
/// `~/.config/herdr/herdr.sock`. Empty env values are treated as unset.
pub fn resolve_socket(
    cli: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
) -> PathBuf {
    if let Some(p) = cli {
        return p.to_path_buf();
    }
    if let Some(v) = env("HERDR_SOCKET_PATH")
        && !v.trim().is_empty()
    {
        return PathBuf::from(v);
    }
    home.join(".config").join("herdr").join("herdr.sock")
}

/// Resolve the OpenRouter key: `$OPENROUTER_API_KEY`, then `~/.openrouter-key`.
///
/// Returns `None` rather than failing — with no key herdash still runs as a
/// status board, which is a legitimate zero-egress mode.
pub fn resolve_api_key(env: &dyn Fn(&str) -> Option<String>, home: &Path) -> Option<String> {
    if let Some(v) = env("OPENROUTER_API_KEY") {
        let v = v.trim().to_string();
        if !v.is_empty() {
            return Some(v);
        }
    }
    let contents = std::fs::read_to_string(home.join(".openrouter-key")).ok()?;
    let trimmed = contents.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

/// Bundle of resolved runtime settings.
#[derive(Debug, Clone)]
pub struct Settings {
    pub socket: PathBuf,
    /// Distinguishes "you turned it off" from "no key found".
    pub summaries: SummariesMode,
    pub api_key: Option<String>,
    pub model: String,
    pub interval: Duration,
    pub cooldown: Duration,
    pub lines: u32,
    pub mouse: bool,
    pub palette: crate::ui::palette::Palette,
    /// The herdr workspace this process runs in, from `$HERDR_WORKSPACE_ID`.
    /// `None` when herdash is not running inside a herdr pane.
    pub workspace_id: Option<String>,
}

impl Settings {
    /// Build settings from parsed args and the real process environment.
    pub fn from_cli(cli: &Cli) -> Self {
        let home = home_dir();
        let env = |k: &str| std::env::var(k).ok();
        let api_key = if cli.no_summaries {
            None
        } else {
            resolve_api_key(&env, &home)
        };
        let summaries = if cli.no_summaries {
            SummariesMode::OffByFlag
        } else if api_key.is_some() {
            SummariesMode::On
        } else {
            SummariesMode::OffNoKey
        };
        Self {
            socket: resolve_socket(cli.socket.as_deref(), &env, &home),
            api_key,
            summaries,
            model: cli.model.clone(),
            interval: Duration::from_secs(cli.interval.max(1)),
            cooldown: Duration::from_secs(cli.cooldown),
            lines: cli.lines,
            mouse: !cli.no_mouse,
            workspace_id: if cli.no_sidebar_token {
                None
            } else {
                // herdr injects this into every managed pane; its absence
                // simply means there is no sidebar to report to.
                std::env::var("HERDR_WORKSPACE_ID")
                    .ok()
                    .filter(|s| !s.is_empty())
            },
            palette: match cli.theme {
                ThemeSource::Ansi => crate::ui::palette::Palette::default(),
                ThemeSource::Auto => crate::ui::palette::Palette::from_herdr_config(
                    &crate::ui::palette::Palette::herdr_config_path(&home),
                ),
            },
        }
    }

    /// Summaries run only when a key resolved and `--no-summaries` was absent.
    pub fn summaries_enabled(&self) -> bool {
        self.summaries.enabled()
    }
}
