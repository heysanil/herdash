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
use herdash::summary::provider::ProviderId;

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
    assert_eq!(
        resolve_api_key(
            ProviderId::Openrouter,
            "https://openrouter.ai/api/v1",
            &env,
            &tmp
        ),
        Some("sk-from-env".into())
    );
}

#[test]
fn the_api_key_falls_back_to_the_dotfile_and_strips_whitespace() {
    let tmp = tempdir();
    std::fs::write(tmp.join(".openrouter-key"), "  sk-from-file\n\n").unwrap();
    let env = env_from(&[]);
    assert_eq!(
        resolve_api_key(
            ProviderId::Openrouter,
            "https://openrouter.ai/api/v1",
            &env,
            &tmp
        ),
        Some("sk-from-file".into())
    );
}

#[test]
fn an_empty_env_key_falls_through_to_the_dotfile() {
    let tmp = tempdir();
    std::fs::write(tmp.join(".openrouter-key"), "sk-from-file\n").unwrap();
    let env = env_from(&[("OPENROUTER_API_KEY", "  ")]);
    assert_eq!(
        resolve_api_key(
            ProviderId::Openrouter,
            "https://openrouter.ai/api/v1",
            &env,
            &tmp
        ),
        Some("sk-from-file".into())
    );
}

#[test]
fn a_missing_key_yields_none_so_the_dashboard_still_runs() {
    let tmp = tempdir();
    assert_eq!(
        resolve_api_key(
            ProviderId::Openrouter,
            "https://openrouter.ai/api/v1",
            &env_from(&[]),
            &tmp
        ),
        None
    );
}

#[test]
fn an_empty_key_file_yields_none() {
    let tmp = tempdir();
    std::fs::write(tmp.join(".openrouter-key"), "\n \n").unwrap();
    assert_eq!(
        resolve_api_key(
            ProviderId::Openrouter,
            "https://openrouter.ai/api/v1",
            &env_from(&[]),
            &tmp
        ),
        None
    );
}

#[test]
fn the_dotfile_key_is_not_sent_to_a_foreign_origin() {
    // Mirrors `provider_resolution::a_vendor_key_is_not_sent_to_a_foreign_origin`,
    // which only exercises the env-var credential. The dotfile is the
    // credential that sits on disk for every existing user without a shell
    // export, and it must clear the same origin gate.
    let tmp = tempdir();
    std::fs::write(tmp.join(".openrouter-key"), "sk-from-file\n").unwrap();
    assert!(
        resolve_api_key(
            ProviderId::Openrouter,
            "http://elsewhere.test/v1",
            &env_from(&[]),
            &tmp
        )
        .is_none(),
        "the dotfile key leaked to an overridden origin"
    );
}

#[test]
fn the_no_key_message_names_the_bare_variable_when_none_was_ever_set() {
    use herdash::config::summaries_status;
    use herdash::summary::provider::{Dialect, ResolvedProvider};

    let provider = Some(ResolvedProvider {
        id: ProviderId::Openai,
        dialect: Dialect::OpenAiDirect,
        base_url: "http://gateway.internal/v1".into(),
        api_key: None,
        model: "m".into(),
    });
    let (mode, detail) = summaries_status(&provider, &env_from(&[]));
    assert_eq!(mode, herdash::app::SummariesMode::OffNoKey);
    assert_eq!(detail.as_deref(), Some("$OPENAI_API_KEY"));
}

#[test]
fn the_no_key_message_names_herdash_api_key_when_a_vendor_key_was_declined() {
    // `--provider openai --base-url http://gateway.internal/v1` with
    // $OPENAI_API_KEY exported: resolve_api_key correctly declines to
    // forward it (origin binding), so the header must not claim the
    // variable is unset — it must name it and point at $HERDASH_API_KEY.
    use herdash::config::summaries_status;
    use herdash::summary::provider::{Dialect, ResolvedProvider};

    let provider = Some(ResolvedProvider {
        id: ProviderId::Openai,
        dialect: Dialect::OpenAiDirect,
        base_url: "http://gateway.internal/v1".into(),
        api_key: None,
        model: "m".into(),
    });
    let env = env_from(&[("OPENAI_API_KEY", "sk-o")]);
    let (mode, detail) = summaries_status(&provider, &env);
    assert_eq!(mode, herdash::app::SummariesMode::OffNoKey);
    let detail = detail.expect("a declined key must still get a detail message");
    assert!(detail.contains("OPENAI_API_KEY"), "{detail}");
    assert!(detail.contains("HERDASH_API_KEY"), "{detail}");
}

#[test]
fn cli_defaults_match_the_spec() {
    // The layered flags parse to `None` absent, not their eventual default —
    // see `provider_resolution::flags_are_distinguishable_from_their_default_values`.
    let cli = Cli::parse_from(["herdash"]);
    assert!(cli.interval.is_none());
    assert!(cli.cooldown.is_none());
    assert!(cli.lines.is_none());
    assert!(cli.model.is_none());
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
    assert_eq!(cli.interval, Some(5));
    assert_eq!(cli.cooldown, Some(90));
    assert_eq!(cli.lines, Some(50));
    assert_eq!(cli.model.as_deref(), Some("other/model"));
    assert!(cli.no_summaries);
    assert_eq!(cli.socket, Some(PathBuf::from("/x.sock")));
}

#[test]
fn the_cli_definition_is_internally_valid() {
    use clap::CommandFactory;
    Cli::command().debug_assert();
}

mod provider_resolution {
    use herdash::config::{Cli, resolve_api_key, resolve_provider};
    use herdash::summary::provider::{Dialect, ProviderId};
    use std::path::Path;

    fn env_from(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let owned: Vec<(String, String)> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |k: &str| {
            owned
                .iter()
                .find(|(key, _)| key == k)
                .map(|(_, v)| v.clone())
        }
    }

    fn cli(args: &[&str]) -> Cli {
        use clap::Parser;
        let mut all = vec!["herdash"];
        all.extend_from_slice(args);
        Cli::parse_from(all)
    }

    #[test]
    fn the_default_is_openrouter_unchanged() {
        let env = env_from(&[("OPENROUTER_API_KEY", "sk-or-x")]);
        let p = resolve_provider(&cli(&[]), &env, Path::new("/nonexistent"))
            .unwrap()
            .unwrap();
        assert_eq!(p.id, ProviderId::Openrouter);
        assert_eq!(p.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(p.model, "openai/gpt-oss-120b");
        assert_eq!(p.api_key.as_deref(), Some("sk-or-x"));
    }

    #[test]
    fn a_preset_supplies_its_own_endpoint_and_dialect() {
        let env = env_from(&[("ANTHROPIC_API_KEY", "sk-ant-x")]);
        let p = resolve_provider(&cli(&["--provider", "anthropic"]), &env, Path::new("/none"))
            .unwrap()
            .unwrap();
        assert_eq!(p.base_url, "https://api.anthropic.com");
        assert_eq!(p.model, "claude-haiku-4-5");
        assert_eq!(p.dialect, Dialect::Anthropic);
    }

    #[test]
    fn a_local_preset_resolves_with_no_key_at_all() {
        let env = env_from(&[]);
        let p = resolve_provider(
            &cli(&["--provider", "ollama", "--model", "qwen3"]),
            &env,
            Path::new("/none"),
        )
        .unwrap()
        .unwrap();
        assert!(p.api_key.is_none());
        assert_eq!(p.base_url, "http://localhost:11434/v1");
        assert!(p.is_loopback());
    }

    #[test]
    fn a_local_preset_without_a_model_is_a_clear_error() {
        let env = env_from(&[]);
        let err = resolve_provider(&cli(&["--provider", "ollama"]), &env, Path::new("/none"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("--model"), "{err}");
        assert!(err.contains("ollama list"), "{err}");
    }

    #[test]
    fn a_compatible_preset_without_a_base_url_is_a_clear_error() {
        let env = env_from(&[]);
        let err = resolve_provider(
            &cli(&["--provider", "openai-compatible", "--model", "m"]),
            &env,
            Path::new("/none"),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("--base-url"), "{err}");
    }

    #[test]
    fn no_summaries_resolves_to_no_provider() {
        let env = env_from(&[("OPENROUTER_API_KEY", "sk-or-x")]);
        assert!(
            resolve_provider(&cli(&["--no-summaries"]), &env, Path::new("/none"))
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn herdash_api_key_outranks_the_vendor_variable() {
        let env = env_from(&[("HERDASH_API_KEY", "sk-h"), ("OPENAI_API_KEY", "sk-o")]);
        assert_eq!(
            resolve_api_key(
                ProviderId::Openai,
                "https://api.openai.com/v1",
                &env,
                Path::new("/none")
            )
            .as_deref(),
            Some("sk-h")
        );
    }

    #[test]
    fn a_vendor_key_is_not_sent_to_a_foreign_origin() {
        // Otherwise `--base-url http://elsewhere/v1` turns a stored key into
        // an exfiltration path.
        let env = env_from(&[("OPENAI_API_KEY", "sk-o")]);
        assert!(
            resolve_api_key(
                ProviderId::Openai,
                "http://elsewhere.test/v1",
                &env,
                Path::new("/none")
            )
            .is_none(),
            "a vendor key leaked to an overridden origin"
        );
        // ...but the explicit, per-run variable still applies.
        let env = env_from(&[("HERDASH_API_KEY", "sk-h"), ("OPENAI_API_KEY", "sk-o")]);
        assert_eq!(
            resolve_api_key(
                ProviderId::Openai,
                "http://elsewhere.test/v1",
                &env,
                Path::new("/none")
            )
            .as_deref(),
            Some("sk-h")
        );
    }

    #[test]
    fn flags_are_distinguishable_from_their_default_values() {
        // The config file (layer 2) can only override a value the user did
        // not supply, so clap must not materialize defaults.
        assert!(cli(&[]).interval.is_none());
        assert_eq!(cli(&["--interval", "1"]).interval, Some(1));
        assert!(cli(&[]).model.is_none());
    }
}
