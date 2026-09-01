//! Anthropic Messages request shaping and response parsing.

use herdash::summary::anthropic::{
    agent_request_body, fleet_request_body, parse_agent_response, parse_fleet_response,
};
use herdash::summary::provider::ReasoningMode;

#[test]
fn the_system_prompt_is_top_level_not_a_message() {
    let b = agent_request_body("m", "transcript", ReasoningMode::Disabled);
    assert!(b["system"].is_string());
    assert_eq!(b["messages"].as_array().unwrap().len(), 1);
    assert_eq!(b["messages"][0]["role"], "user");
}

#[test]
fn sampling_parameters_are_never_sent() {
    // temperature, top_p and top_k were removed on Opus 5, Sonnet 5 and
    // Opus 4.7/4.8 — sending any of them is a 400.
    let b = agent_request_body("m", "t", ReasoningMode::Disabled);
    for field in ["temperature", "top_p", "top_k"] {
        assert!(b.get(field).is_none(), "{field} must not be sent");
    }
}

#[test]
fn max_tokens_is_always_present() {
    let b = agent_request_body("m", "t", ReasoningMode::Disabled);
    assert!(b["max_tokens"].is_number());
    assert!(b.get("max_completion_tokens").is_none());
}

#[test]
fn the_schema_goes_in_output_config_format_without_name_or_strict() {
    let b = agent_request_body("m", "t", ReasoningMode::Disabled);
    let f = &b["output_config"]["format"];
    assert_eq!(f["type"], "json_schema");
    assert_eq!(f["schema"]["additionalProperties"], false);
    assert!(f.get("name").is_none(), "Anthropic takes no schema name");
    assert!(f.get("strict").is_none(), "Anthropic takes no strict flag");
    assert!(b.get("response_format").is_none());
}

#[test]
fn rungs_render_as_thinking_then_effort_then_nothing() {
    let b = agent_request_body("m", "t", ReasoningMode::Disabled);
    assert_eq!(b["thinking"], serde_json::json!({ "type": "disabled" }));
    assert!(b["output_config"].get("effort").is_none());

    let b = agent_request_body("m", "t", ReasoningMode::LowEffort);
    assert!(b.get("thinking").is_none());
    // effort merges into the same output_config that carries the schema.
    assert_eq!(b["output_config"]["effort"], "low");
    assert_eq!(b["output_config"]["format"]["type"], "json_schema");

    let b = agent_request_body("m", "t", ReasoningMode::ProviderDefault);
    assert!(b.get("thinking").is_none());
    assert!(b["output_config"].get("effort").is_none());
}

#[test]
fn the_fleet_body_carries_no_schema() {
    let b = fleet_request_body("m", &["a".into()], ReasoningMode::Disabled);
    assert!(b.get("output_config").is_none() || b["output_config"].get("format").is_none());
    assert!(b["max_tokens"].is_number());
}

#[test]
fn a_thinking_rung_gets_a_larger_budget() {
    let quiet = agent_request_body("m", "t", ReasoningMode::Disabled);
    let thinking = agent_request_body("m", "t", ReasoningMode::LowEffort);
    assert!(thinking["max_tokens"].as_u64() > quiet["max_tokens"].as_u64());
}

#[test]
fn text_is_taken_from_the_first_text_block() {
    // A response may lead with a thinking block; the text block is what counts.
    let body = serde_json::json!({
        "stop_reason": "end_turn",
        "content": [
            { "type": "thinking", "thinking": "" },
            { "type": "text", "text": r#"{"headline":"h","task":"t","now":"n","recent":[],"needs_attention":false,"attention_reason":""}"# }
        ]
    })
    .to_string();
    let s = parse_agent_response(&body).unwrap();
    assert_eq!(s.headline, "h");
}

#[test]
fn a_max_tokens_stop_is_reported_as_truncation() {
    let body = serde_json::json!({
        "stop_reason": "max_tokens",
        "content": [{ "type": "text", "text": "" }]
    })
    .to_string();
    let err = parse_agent_response(&body).unwrap_err().to_string();
    assert!(
        err.contains("truncated") || err.contains("max_tokens"),
        "{err}"
    );
    assert!(
        !err.contains("did not return the requested schema"),
        "{err}"
    );
}

#[test]
fn a_refusal_is_reported_as_itself() {
    let body = serde_json::json!({
        "stop_reason": "refusal",
        "content": []
    })
    .to_string();
    let err = parse_agent_response(&body).unwrap_err().to_string();
    assert!(err.contains("refusal"), "{err}");
}

#[test]
fn the_error_envelope_surfaces_the_message() {
    let body = r#"{"type":"error","error":{"type":"invalid_request_error","message":"thinking.type: disabled is not supported"}}"#;
    let err = parse_agent_response(body).unwrap_err().to_string();
    assert!(err.contains("disabled is not supported"), "{err}");
}

#[test]
fn the_fleet_response_is_trimmed_prose() {
    let body = serde_json::json!({
        "stop_reason": "end_turn",
        "content": [{ "type": "text", "text": "  two agents are busy.\n" }]
    })
    .to_string();
    assert_eq!(parse_fleet_response(&body).unwrap(), "two agents are busy.");
}
