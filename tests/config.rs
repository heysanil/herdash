//! Settings resolution order.
//!
//! Both resolvers take an injected environment accessor and home path, so
//! these tests never mutate process-global state (which races across
//! parallel test threads).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use clap::Parser;
use herdash::config::{Cli, resolve_api_key, resolve_socket};

fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    move |k: &str| map.get(k).cloned()
}

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Unique temp dir per call — avoids a dev-dependency and cross-test races.
fn tempdir() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    let base = std::env::temp_dir().join(format!("herdash-cfg-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    base
}

#[test]
fn the_socket_flag_wins_over_everything() {
    let env = env_from(&[("HERDR_SOCKET_PATH", "/from/env.sock")]);
    let got = resolve_socket(
        Some(Path::new("/from/flag.sock")),
        &env,
        Path::new("/home/u"),
    );
    assert_eq!(got, PathBuf::from("/from/flag.sock"));
}

#[test]
fn the_env_var_wins_over_the_default() {
    let env = env_from(&[("HERDR_SOCKET_PATH", "/from/env.sock")]);
    assert_eq!(
        resolve_socket(None, &env, Path::new("/home/u")),
        PathBuf::from("/from/env.sock")
    );
}

#[test]
fn the_socket_falls_back_to_the_herdr_config_directory() {
    let env = env_from(&[]);
    assert_eq!(
        resolve_socket(None, &env, Path::new("/home/u")),
        PathBuf::from("/home/u/.config/herdr/herdr.sock")
    );
}

#[test]
fn an_empty_env_var_is_ignored_rather_than_yielding_an_empty_path() {
    let env = env_from(&[("HERDR_SOCKET_PATH", "   ")]);
    assert_eq!(
        resolve_socket(None, &env, Path::new("/home/u")),
        PathBuf::from("/home/u/.config/herdr/herdr.sock")
    );
}

#[test]
fn the_api_key_prefers_the_environment() {
    let tmp = tempdir();
    std::fs::write(tmp.join(".openrouter-key"), "sk-from-file\n").unwrap();
    let env = env_from(&[("OPENROUTER_API_KEY", "sk-from-env")]);
    assert_eq!(resolve_api_key(&env, &tmp), Some("sk-from-env".into()));
}

#[test]
fn the_api_key_falls_back_to_the_dotfile_and_strips_whitespace() {
    let tmp = tempdir();
    std::fs::write(tmp.join(".openrouter-key"), "  sk-from-file\n\n").unwrap();
    let env = env_from(&[]);
    assert_eq!(resolve_api_key(&env, &tmp), Some("sk-from-file".into()));
}

#[test]
fn an_empty_env_key_falls_through_to_the_dotfile() {
    let tmp = tempdir();
    std::fs::write(tmp.join(".openrouter-key"), "sk-from-file\n").unwrap();
    let env = env_from(&[("OPENROUTER_API_KEY", "  ")]);
    assert_eq!(resolve_api_key(&env, &tmp), Some("sk-from-file".into()));
}

#[test]
fn a_missing_key_yields_none_so_the_dashboard_still_runs() {
    let tmp = tempdir();
    assert_eq!(resolve_api_key(&env_from(&[]), &tmp), None);
}

#[test]
fn an_empty_key_file_yields_none() {
    let tmp = tempdir();
    std::fs::write(tmp.join(".openrouter-key"), "\n \n").unwrap();
    assert_eq!(resolve_api_key(&env_from(&[]), &tmp), None);
}

#[test]
fn cli_defaults_match_the_spec() {
    let cli = Cli::parse_from(["herdash"]);
    assert_eq!(cli.interval, 1);
    assert_eq!(cli.cooldown, 45);
    assert_eq!(cli.lines, 200);
    assert_eq!(cli.model, "meta-llama/llama-4-scout:nitro");
    assert!(!cli.no_summaries);
    assert!(cli.socket.is_none());
}

#[test]
fn cli_flags_parse() {
    let cli = Cli::parse_from([
        "herdash",
        "--interval",
        "5",
        "--cooldown",
        "90",
        "--lines",
        "50",
        "--model",
        "other/model",
        "--no-summaries",
        "--socket",
        "/x.sock",
    ]);
    assert_eq!(cli.interval, 5);
    assert_eq!(cli.cooldown, 90);
    assert_eq!(cli.lines, 50);
    assert_eq!(cli.model, "other/model");
    assert!(cli.no_summaries);
    assert_eq!(cli.socket, Some(PathBuf::from("/x.sock")));
}

#[test]
fn the_cli_definition_is_internally_valid() {
    use clap::CommandFactory;
    Cli::command().debug_assert();
}
