//! The preset table, dialect properties, and URL derivation.
//!
//! These are pure lookups, but they encode every provider-specific 400 trap
//! recorded in the spec's §3 — getting one wrong is a runtime failure against
//! a real vendor that no other test can catch.

use herdash::summary::provider::{
    Dialect, KeyRequirement, ProviderId, ReasoningMode, ResolvedProvider, Wire, is_loopback_url,
    preset, same_origin,
};

fn resolved(id: ProviderId, base: &str) -> ResolvedProvider {
    ResolvedProvider {
        id,
        dialect: preset(id).dialect,
        base_url: base.to_string(),
        api_key: None,
        model: "m".into(),
    }
}

#[test]
fn openrouter_keeps_every_current_default() {
    let p = preset(ProviderId::Openrouter);
    assert_eq!(p.default_base_url, Some("https://openrouter.ai/api/v1"));
    assert_eq!(p.default_model, Some("openai/gpt-oss-120b"));
    assert_eq!(p.key, KeyRequirement::Required);
    assert_eq!(p.dialect, Dialect::OpenRouter);
}

#[test]
fn local_presets_need_no_key_but_do_need_a_model() {
    for id in [ProviderId::Ollama, ProviderId::Lmstudio] {
        let p = preset(id);
        assert_eq!(p.key, KeyRequirement::None, "{id:?} should need no key");
        assert!(
            p.default_model.is_none(),
            "{id:?} cannot have a default model — names are whatever the user pulled"
        );
        assert!(p.default_base_url.is_some());
    }
}

#[test]
fn compatible_presets_have_no_default_endpoint() {
    for id in [
        ProviderId::OpenaiCompatible,
        ProviderId::AnthropicCompatible,
    ] {
        let p = preset(id);
        assert!(p.default_base_url.is_none());
        assert!(p.default_model.is_none());
    }
}

#[test]
fn dialects_disagree_within_one_wire() {
    // This is the whole reason Dialect exists rather than Wire.
    assert_eq!(Dialect::OpenRouter.wire(), Wire::OpenAi);
    assert_eq!(Dialect::OpenAiDirect.wire(), Wire::OpenAi);
    assert_eq!(Dialect::OpenAiGeneric.wire(), Wire::OpenAi);

    // gpt-5 / o-series / gpt-4.1 reject `max_tokens` outright.
    assert_eq!(
        Dialect::OpenAiDirect.token_cap_field(),
        "max_completion_tokens"
    );
    assert_eq!(Dialect::OpenRouter.token_cap_field(), "max_tokens");
    assert_eq!(Dialect::OpenAiGeneric.token_cap_field(), "max_tokens");

    // o-series rejects any temperature but 1; current Claude models removed
    // sampling parameters entirely.
    assert!(!Dialect::OpenAiDirect.sends_temperature());
    assert!(!Dialect::Anthropic.sends_temperature());
    assert!(Dialect::OpenRouter.sends_temperature());
    assert!(Dialect::OpenAiGeneric.sends_temperature());
}

#[test]
fn start_rungs_match_what_each_dialect_can_actually_accept() {
    // `reasoning` is an OpenRouter extension; OpenAI direct rejects it as an
    // unknown argument, and llama.cpp-backed servers have no knob at all.
    assert_eq!(Dialect::OpenRouter.start_rung(), ReasoningMode::Disabled);
    assert_eq!(Dialect::Anthropic.start_rung(), ReasoningMode::Disabled);
    assert_eq!(Dialect::OpenAiDirect.start_rung(), ReasoningMode::LowEffort);
    assert_eq!(
        Dialect::OpenAiGeneric.start_rung(),
        ReasoningMode::ProviderDefault
    );
}

#[test]
fn reasons_at_reports_whether_a_rung_lets_the_model_think() {
    // Drives the token budget: a thinking rung needs headroom.
    assert!(!Dialect::OpenRouter.reasons_at(ReasoningMode::Disabled));
    assert!(Dialect::OpenRouter.reasons_at(ReasoningMode::LowEffort));
    assert!(Dialect::OpenRouter.reasons_at(ReasoningMode::ProviderDefault));
    // A generic server never renders a reasoning field, but its model may
    // still think by default, so the top rung is treated as thinking.
    assert!(!Dialect::OpenAiGeneric.reasons_at(ReasoningMode::Disabled));
}

#[test]
fn the_rung_ladder_terminates() {
    assert_eq!(
        ReasoningMode::Disabled.escalate(),
        Some(ReasoningMode::LowEffort)
    );
    assert_eq!(
        ReasoningMode::LowEffort.escalate(),
        Some(ReasoningMode::ProviderDefault)
    );
    assert_eq!(ReasoningMode::ProviderDefault.escalate(), None);
    // Indices must be ordered for the monotonic fetch_max cache (Task 1).
    assert!(ReasoningMode::Disabled.index() < ReasoningMode::LowEffort.index());
    assert!(ReasoningMode::LowEffort.index() < ReasoningMode::ProviderDefault.index());
}

#[test]
fn each_wire_appends_its_own_path() {
    // OpenAI-style base URLs include /v1; Anthropic-style ones do not. Both
    // conventions are copied from the vendors' own docs on purpose.
    let o = resolved(ProviderId::Openrouter, "https://openrouter.ai/api/v1");
    assert_eq!(
        o.completions_url(),
        "https://openrouter.ai/api/v1/chat/completions"
    );
    assert_eq!(o.models_url(), "https://openrouter.ai/api/v1/models");

    let a = resolved(ProviderId::Anthropic, "https://api.anthropic.com");
    assert_eq!(a.completions_url(), "https://api.anthropic.com/v1/messages");
    assert_eq!(a.models_url(), "https://api.anthropic.com/v1/models");
}

#[test]
fn a_trailing_slash_does_not_double_up() {
    let o = resolved(ProviderId::Ollama, "http://localhost:11434/v1/");
    assert_eq!(
        o.completions_url(),
        "http://localhost:11434/v1/chat/completions"
    );
    let a = resolved(ProviderId::Anthropic, "https://api.anthropic.com/");
    assert_eq!(a.completions_url(), "https://api.anthropic.com/v1/messages");
}

#[test]
fn loopback_is_decided_by_host_not_by_preset_name() {
    // The (local) badge is a claim about egress. A local preset pointed at a
    // LAN box must not wear it.
    assert!(is_loopback_url("http://localhost:11434/v1"));
    assert!(is_loopback_url("http://127.0.0.1:1234/v1"));
    assert!(is_loopback_url("http://127.1.2.3:8080/v1"));
    assert!(is_loopback_url("http://[::1]:11434/v1"));
    assert!(!is_loopback_url("http://192.168.1.50:11434/v1"));
    assert!(!is_loopback_url("https://openrouter.ai/api/v1"));
    assert!(!is_loopback_url("not a url"));

    let lan = resolved(ProviderId::Ollama, "http://192.168.1.50:11434/v1");
    assert!(
        !lan.is_loopback(),
        "an ollama preset on a LAN address is not local"
    );
}

#[test]
fn a_bracketed_host_with_trailing_text_is_not_loopback() {
    // `[::1].evil.test` must not masquerade as loopback by having the
    // bracket contents read and the remainder discarded.
    assert!(!is_loopback_url("http://[::1].evil.test/v1"));
    assert!(!is_loopback_url("http://[::1]x/v1"));
    // A port after the bracket is the only thing allowed to follow it.
    assert!(is_loopback_url("http://[::1]:11434/v1"));
    assert!(is_loopback_url("http://[::1]/v1"));
}

#[test]
fn an_ipv4_mapped_loopback_address_counts_as_local() {
    assert!(is_loopback_url("http://[::ffff:127.0.0.1]:11434/v1"));
}

#[test]
fn a_gateway_base_url_with_a_path_prefix_keeps_its_prefix() {
    // `openai-compatible` exists for gateways, which commonly live under a
    // path rather than at the root.
    let p = resolved(ProviderId::OpenaiCompatible, "https://gw.test/openai/v1");
    assert_eq!(
        p.completions_url(),
        "https://gw.test/openai/v1/chat/completions"
    );
    assert_eq!(p.models_url(), "https://gw.test/openai/v1/models");
}

#[test]
fn origin_comparison_ignores_path_and_trailing_slash() {
    assert!(same_origin(
        "https://api.openai.com/v1",
        "https://api.openai.com/v1/"
    ));
    assert!(same_origin(
        "https://api.openai.com/v1",
        "https://api.openai.com/"
    ));
    assert!(!same_origin(
        "https://api.openai.com/v1",
        "http://api.openai.com/v1"
    ));
    assert!(!same_origin(
        "https://api.openai.com/v1",
        "https://evil.test/v1"
    ));
    assert!(!same_origin(
        "https://api.openai.com/v1",
        "https://api.openai.com:8443/v1"
    ));
}
