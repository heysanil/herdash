//! OpenAI v1 request shaping and response parsing, per dialect.

use herdash::summary::openai::{
    agent_request_body, fleet_request_body, parse_agent_response, parse_fleet_response,
};
use herdash::summary::provider::{Dialect, ReasoningMode};

fn agent(d: Dialect, rung: ReasoningMode) -> serde_json::Value {
    agent_request_body(d, "m", "transcript", rung)
}

#[test]
fn openrouter_sends_its_reasoning_object() {
    let b = agent(Dialect::OpenRouter, ReasoningMode::Disabled);
    assert_eq!(b["reasoning"], serde_json::json!({ "enabled": false }));
    let b = agent(Dialect::OpenRouter, ReasoningMode::LowEffort);
    assert_eq!(b["reasoning"], serde_json::json!({ "effort": "low" }));
    let b = agent(Dialect::OpenRouter, ReasoningMode::ProviderDefault);
    assert!(b.get("reasoning").is_none());
}

#[test]
fn openai_direct_sends_reasoning_effort_not_reasoning() {
    // `reasoning` is an OpenRouter extension; OpenAI rejects it as an
    // unrecognized argument.
    let b = agent(Dialect::OpenAiDirect, ReasoningMode::LowEffort);
    assert_eq!(b["reasoning_effort"], "low");
    assert!(b.get("reasoning").is_none());
}

#[test]
fn a_generic_endpoint_sends_no_reasoning_field_at_any_rung() {
    for rung in [
        ReasoningMode::Disabled,
        ReasoningMode::LowEffort,
        ReasoningMode::ProviderDefault,
    ] {
        let b = agent(Dialect::OpenAiGeneric, rung);
        assert!(b.get("reasoning").is_none(), "{rung:?}");
        assert!(b.get("reasoning_effort").is_none(), "{rung:?}");
    }
}

#[test]
fn openai_direct_uses_max_completion_tokens_and_omits_temperature() {
    let b = agent(Dialect::OpenAiDirect, ReasoningMode::LowEffort);
    assert!(
        b.get("max_tokens").is_none(),
        "gpt-5 and the o-series reject max_tokens outright"
    );
    assert!(b["max_completion_tokens"].is_number());
    assert!(
        b.get("temperature").is_none(),
        "the o-series rejects any temperature but 1"
    );
}

#[test]
fn other_openai_wire_dialects_keep_max_tokens_and_temperature() {
    for d in [Dialect::OpenRouter, Dialect::OpenAiGeneric] {
        let b = agent(d, ReasoningMode::Disabled);
        assert!(b["max_tokens"].is_number(), "{d:?}");
        assert!(b.get("max_completion_tokens").is_none(), "{d:?}");
        assert_eq!(b["temperature"], 0.2, "{d:?}");
    }
}

/// Agent summarization is extraction and wants determinism; the fleet
/// overview is a sentence or two of prose and gets more room. A shared
/// `base_body` must not collapse that distinction back to one value.
#[test]
fn the_agent_and_fleet_bodies_keep_their_own_temperatures() {
    let b = agent(Dialect::OpenRouter, ReasoningMode::Disabled);
    assert_eq!(b["temperature"], 0.2, "agent summaries want determinism");

    let f = fleet_request_body(
        Dialect::OpenRouter,
        "m",
        &["a".into(), "b".into()],
        ReasoningMode::Disabled,
    );
    assert_eq!(f["temperature"], 0.3, "the fleet overview is prose");
}

#[test]
fn a_thinking_rung_gets_a_larger_budget() {
    // Reasoning tokens are billed against the output cap, so a rung that
    // thinks must not be given the reasoning-disabled budget.
    let quiet = agent(Dialect::OpenRouter, ReasoningMode::Disabled);
    let thinking = agent(Dialect::OpenRouter, ReasoningMode::LowEffort);
    assert!(
        thinking["max_tokens"].as_u64() > quiet["max_tokens"].as_u64(),
        "a thinking rung reused the reasoning-disabled budget"
    );

    let quiet = fleet_request_body(Dialect::OpenRouter, "m", &[], ReasoningMode::Disabled);
    let thinking = fleet_request_body(Dialect::OpenRouter, "m", &[], ReasoningMode::LowEffort);
    assert!(thinking["max_tokens"].as_u64() > quiet["max_tokens"].as_u64());
}

#[test]
fn the_agent_body_demands_a_strict_schema() {
    let b = agent(Dialect::OpenRouter, ReasoningMode::Disabled);
    assert_eq!(b["response_format"]["type"], "json_schema");
    assert_eq!(b["response_format"]["json_schema"]["strict"], true);
    assert_eq!(b["response_format"]["json_schema"]["name"], "agent_summary");
    assert_eq!(
        b["response_format"]["json_schema"]["schema"]["additionalProperties"],
        false
    );
    assert_eq!(b["messages"][0]["role"], "system");
    assert_eq!(b["messages"][1]["role"], "user");
}

#[test]
fn the_fleet_body_omits_response_format_entirely() {
    // LM Studio historically rejected any response_format.type that was not
    // json_schema. Omitting the field never trips that.
    let b = fleet_request_body(
        Dialect::OpenAiGeneric,
        "m",
        &["a".into()],
        ReasoningMode::Disabled,
    );
    assert!(b.get("response_format").is_none());
}

#[test]
fn a_well_formed_response_parses() {
    let body = serde_json::json!({
        "choices": [{
            "finish_reason": "stop",
            "message": { "content": r#"{"headline":"h","task":"t","now":"n","recent":[],"needs_attention":false,"attention_reason":""}"# }
        }]
    })
    .to_string();
    let s = parse_agent_response(&body).unwrap();
    assert_eq!(s.headline, "h");
    assert!(!s.needs_attention);
}

#[test]
fn a_length_stop_is_reported_as_truncation_not_a_schema_failure() {
    // Misattributing this sends the reader looking in the wrong place: the
    // model obeyed the schema, the budget ran out mid-object.
    let body = serde_json::json!({
        "choices": [{ "finish_reason": "length", "message": { "content": "" } }]
    })
    .to_string();
    let err = parse_agent_response(&body).unwrap_err().to_string();
    assert!(
        err.contains("truncated") || err.contains("length"),
        "unhelpful error: {err}"
    );
    assert!(
        !err.contains("did not return the requested schema"),
        "{err}"
    );
}

#[test]
fn a_refusal_is_reported_as_itself() {
    let body = serde_json::json!({
        "choices": [{ "finish_reason": "content_filter", "message": { "content": "" } }]
    })
    .to_string();
    let err = parse_agent_response(&body).unwrap_err().to_string();
    assert!(err.contains("content_filter"), "{err}");
}

#[test]
fn an_error_envelope_surfaces_the_provider_message() {
    let body = r#"{"error":{"message":"Reasoning is mandatory for this endpoint"}}"#;
    let err = parse_agent_response(body).unwrap_err().to_string();
    assert!(err.contains("Reasoning is mandatory"), "{err}");
}

#[test]
fn non_json_is_reported_with_a_prefix_of_the_body() {
    let err = parse_agent_response("<html>502 Bad Gateway</html>")
        .unwrap_err()
        .to_string();
    assert!(err.contains("502"), "{err}");
}

#[test]
fn the_fleet_response_is_trimmed_prose() {
    let body = serde_json::json!({
        "choices": [{ "finish_reason": "stop", "message": { "content": "  two agents are busy.\n" } }]
    })
    .to_string();
    assert_eq!(parse_fleet_response(&body).unwrap(), "two agents are busy.");
}
