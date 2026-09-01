//! Command-line interface and settings resolution.

use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::Parser;

use crate::app::SummariesMode;
use crate::summary::provider::{KeyRequirement, ProviderId, ResolvedProvider, preset, same_origin};

pub const DEFAULT_INTERVAL: u64 = 1;
pub const DEFAULT_COOLDOWN: u64 = 45;
pub const DEFAULT_LINES: u32 = 200;

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
    /// Seconds between herdr snapshot polls. [default: 1]
    #[arg(long)]
    pub interval: Option<u64>,

    /// Minimum seconds between summaries for the same agent. [default: 45]
    #[arg(long)]
    pub cooldown: Option<u64>,

    /// Transcript lines requested from each agent. [default: 200]
    #[arg(long)]
    pub lines: Option<u32>,

    /// Backend preset. Sets the wire protocol and a default endpoint.
    #[arg(long, value_enum)]
    pub provider: Option<ProviderId>,

    /// Override the provider's endpoint. OpenAI-style URLs include `/v1`;
    /// Anthropic-style URLs do not.
    #[arg(long)]
    pub base_url: Option<String>,

    /// Model name or slug, interpreted by the provider.
    #[arg(long)]
    pub model: Option<String>,

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

/// Resolve the API key for `id` at `base_url`.
///
/// Order: `$HERDASH_API_KEY`, then the provider's own variable, then
/// `~/.openrouter-key` for openrouter. Returns `None` rather than failing —
/// a provider that needs no key is a legitimate configuration.
///
/// **Vendor keys are bound to the preset's own origin.** `--base-url` can
/// point a vendor preset anywhere, and silently forwarding a real key to an
/// unrelated host would turn a convenience into credential exfiltration.
/// `HERDASH_API_KEY` is exempt: naming it is an explicit act for the run at
/// hand.
pub fn resolve_api_key(
    id: ProviderId,
    base_url: &str,
    env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
) -> Option<String> {
    let non_empty = |v: String| {
        let v = v.trim().to_string();
        (!v.is_empty()).then_some(v)
    };
    if let Some(v) = env("HERDASH_API_KEY").and_then(non_empty) {
        return Some(v);
    }
    let p = preset(id);
    let on_own_origin = p.default_base_url.is_some_and(|d| same_origin(d, base_url));
    if !on_own_origin {
        return None;
    }
    if let Some(v) = p.id.env_var().and_then(env).and_then(non_empty) {
        return Some(v);
    }
    if id == ProviderId::Openrouter {
        return std::fs::read_to_string(home.join(".openrouter-key"))
            .ok()
            .and_then(non_empty);
    }
    None
}

/// Build the provider from flags and the environment.
///
/// `Ok(None)` means `--no-summaries`. An `Err` is a configuration mistake
/// worth exiting for, reported before the terminal is taken over.
pub fn resolve_provider(
    cli: &Cli,
    env: &dyn Fn(&str) -> Option<String>,
    home: &Path,
) -> anyhow::Result<Option<ResolvedProvider>> {
    if cli.no_summaries {
        return Ok(None);
    }
    let id = cli.provider.unwrap_or(ProviderId::Openrouter);
    let p = preset(id);

    let base_url = cli
        .base_url
        .clone()
        .or_else(|| p.default_base_url.map(str::to_string))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "--base-url is required for {}.\nExample: --base-url http://localhost:8000/v1",
                id.as_str()
            )
        })?;

    let model = cli
        .model
        .clone()
        .or_else(|| p.default_model.map(str::to_string))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "--model is required for {}.\n{}",
                id.as_str(),
                id.model_hint()
            )
        })?;

    Ok(Some(ResolvedProvider {
        id,
        dialect: p.dialect,
        api_key: resolve_api_key(id, &base_url, env, home),
        base_url,
        model,
    }))
}

/// Whether this provider still needs a key it does not have.
pub fn needs_missing_key(p: &ResolvedProvider) -> bool {
    preset(p.id).key == KeyRequirement::Required && p.api_key.is_none()
}

/// Bundle of resolved runtime settings.
#[derive(Debug, Clone)]
pub struct Settings {
    pub socket: PathBuf,
    /// Distinguishes "you turned it off" from "no key found".
    pub summaries: SummariesMode,
    pub provider: Option<ResolvedProvider>,
    pub interval: Duration,
    pub cooldown: Duration,
    pub lines: u32,
    pub mouse: bool,
    pub palette: crate::ui::palette::Palette,
}

impl Settings {
    /// Build settings from parsed args and the real process environment.
    pub fn from_cli(cli: &Cli) -> anyhow::Result<Self> {
        let home = home_dir();
        let env = |k: &str| std::env::var(k).ok();
        let provider = resolve_provider(cli, &env, &home)?;
        let summaries = if cli.no_summaries {
            SummariesMode::OffByFlag
        } else if provider.as_ref().is_some_and(needs_missing_key) {
            SummariesMode::OffNoKey
        } else {
            SummariesMode::On
        };
        Ok(Self {
            socket: resolve_socket(cli.socket.as_deref(), &env, &home),
            summaries,
            provider,
            interval: Duration::from_secs(cli.interval.unwrap_or(DEFAULT_INTERVAL).max(1)),
            cooldown: Duration::from_secs(cli.cooldown.unwrap_or(DEFAULT_COOLDOWN)),
            lines: cli.lines.unwrap_or(DEFAULT_LINES),
            mouse: !cli.no_mouse,
            palette: match cli.theme {
                ThemeSource::Ansi => crate::ui::palette::Palette::default(),
                ThemeSource::Auto => crate::ui::palette::Palette::from_herdr_config(
                    &crate::ui::palette::Palette::herdr_config_path(&home),
                ),
            },
        })
    }

    /// Summaries run only when a key resolved and `--no-summaries` was absent.
    pub fn summaries_enabled(&self) -> bool {
        self.summaries.enabled()
    }
}
