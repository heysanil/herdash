//! Structured-output parsing, sanitization and transcript clamping.

use herdash::summary::AgentSummary;
use herdash::summary::openrouter::{
    TRANSCRIPT_MAX_BYTES, agent_request_body, clamp_transcript, fleet_request_body,
    parse_agent_response, parse_fleet_response,
};
use herdash::summary::types::{DASH, or_dash, truncate_chars};

fn wrap(content: &str) -> String {
    serde_json::json!({ "choices": [ { "message": { "content": content } } ] }).to_string()
}

#[test]
fn a_valid_structured_response_parses() {
    let body = wrap(
        r#"{"headline":"Fixing proration rounding","task":"Fix the billing bug","now":"Editing proration()","recent":["Read billing.py","41 passed, 2 failed"]}"#,
    );
    let s = parse_agent_response(&body).unwrap();
    assert_eq!(s.headline, "Fixing proration rounding");
    assert_eq!(s.task, "Fix the billing bug");
    assert_eq!(s.now, "Editing proration()");
    assert_eq!(s.recent.len(), 2);
}

#[test]
fn an_absent_recent_array_defaults_to_empty() {
    let s = parse_agent_response(&wrap(r#"{"headline":"h","task":"t","now":"n"}"#)).unwrap();
    assert!(s.recent.is_empty());
}

#[test]
fn blank_recent_entries_are_dropped_and_the_list_is_capped() {
    let body = wrap(
        r#"{"headline":"h","task":"t","now":"n","recent":["a","  ","b","c","d","e","f","g"]}"#,
    );
    let s = parse_agent_response(&body).unwrap();
    assert_eq!(
        s.recent,
        vec!["a", "b", "c", "d", "e", "f"],
        "blank entries dropped, list capped at six"
    );
}

#[test]
fn an_overlong_headline_is_truncated_on_a_character_boundary() {
    let long = "é".repeat(200);
    let body =
        wrap(&serde_json::json!({"headline": long, "task":"t","now":"n","recent":[]}).to_string());
    let s = parse_agent_response(&body).unwrap();
    assert!(s.headline.chars().count() <= 60);
    assert!(s.headline.ends_with('…'));
    assert!(std::str::from_utf8(s.headline.as_bytes()).is_ok());
}

#[test]
fn surrounding_whitespace_is_trimmed_from_every_field() {
    let body = wrap(r#"{"headline":"  h  ","task":"  t  ","now":"  n  ","recent":["  a  "]}"#);
    let s = parse_agent_response(&body).unwrap();
    assert_eq!(s.headline, "h");
    assert_eq!(s.task, "t");
    assert_eq!(s.now, "n");
    assert_eq!(s.recent, vec!["a"]);
}

#[test]
fn a_missing_required_field_is_an_error_not_a_panic() {
    assert!(parse_agent_response(&wrap(r#"{"headline":"h","task":"t"}"#)).is_err());
}

#[test]
fn a_non_json_content_payload_is_an_error() {
    assert!(parse_agent_response(&wrap("I'm sorry, I can't do that.")).is_err());
}

#[test]
fn an_api_error_envelope_surfaces_its_message() {
    let body = serde_json::json!({"error": {"code": 429, "message": "rate limited"}}).to_string();
    let err = parse_agent_response(&body).unwrap_err().to_string();
    assert!(err.contains("rate limited"), "got: {err}");
}

#[test]
fn a_response_with_no_choices_is_an_error() {
    let body = serde_json::json!({"choices": []}).to_string();
    assert!(parse_agent_response(&body).is_err());
}

#[test]
fn empty_fields_render_as_an_em_dash() {
    assert_eq!(or_dash(""), DASH);
    assert_eq!(or_dash("   "), DASH);
    assert_eq!(or_dash("real"), "real");
}

#[test]
fn truncate_leaves_short_strings_untouched() {
    assert_eq!(truncate_chars("short", 60), "short");
    assert_eq!(truncate_chars("", 60), "");
}

#[test]
fn a_short_transcript_is_untouched() {
    let t = "line one\nline two\n";
    assert_eq!(clamp_transcript(t, TRANSCRIPT_MAX_BYTES), t);
}

#[test]
fn a_long_transcript_keeps_its_tail() {
    let t = format!("{}\nTHE VERY LAST LINE\n", "x".repeat(50_000));
    let clamped = clamp_transcript(&t, 1024);
    assert!(clamped.len() <= 1024);
    assert!(
        clamped.contains("THE VERY LAST LINE"),
        "recent output matters most"
    );
}

/// `clamp_transcript` returns `&str`, so asserting the result is valid UTF-8
/// proves nothing — it could not be otherwise. Assert instead that the result
/// is an exact suffix of the input, which is what "cut on a boundary" means.
#[test]
fn clamping_cuts_on_a_character_boundary_and_keeps_a_true_suffix() {
    let t = "日本語のテキスト".repeat(500);
    let clamped = clamp_transcript(&t, 100);
    assert!(clamped.len() <= 100);
    assert!(!clamped.is_empty());
    assert!(
        t.ends_with(clamped),
        "the clamped text must be a real suffix of the input"
    );
    // A boundary-safe cut means the character count is exactly len/3 here.
    assert!(clamped.chars().count() > 0);
}

#[test]
fn clamping_a_transcript_with_no_newlines_still_returns_the_tail() {
    let t = "z".repeat(5000);
    let clamped = clamp_transcript(&t, 100);
    assert_eq!(clamped.len(), 100);
}

#[test]
fn the_request_body_demands_strict_structured_output() {
    let b = agent_request_body("meta-llama/llama-4-scout:nitro", "transcript here");
    assert_eq!(b["model"], "meta-llama/llama-4-scout:nitro");
    assert_eq!(b["response_format"]["type"], "json_schema");
    assert_eq!(b["response_format"]["json_schema"]["strict"], true);
    let required = b["response_format"]["json_schema"]["schema"]["required"]
        .as_array()
        .unwrap();
    for field in ["headline", "task", "now", "recent"] {
        assert!(required.iter().any(|r| r == field), "missing {field}");
    }
    assert_eq!(
        b["response_format"]["json_schema"]["schema"]["additionalProperties"],
        false
    );
    let user = b["messages"][1]["content"].as_str().unwrap();
    assert!(user.contains("transcript here"));
}

#[test]
fn the_fleet_request_carries_every_headline_and_no_schema() {
    let b = fleet_request_body("m", &["one".to_string(), "two".to_string()]);
    let user = b["messages"][1]["content"].as_str().unwrap();
    assert!(user.contains("one") && user.contains("two"));
    assert!(
        b.get("response_format").is_none(),
        "the overview is prose, not JSON"
    );
}

#[test]
fn the_fleet_response_is_plain_prose() {
    let body = wrap("Two agents are converging on billing; one needs approval.");
    assert!(
        parse_fleet_response(&body)
            .unwrap()
            .starts_with("Two agents")
    );
}

#[test]
fn a_stub_summarizer_satisfies_the_trait() {
    // Compile-time proof that consumers can be tested without the network.
    struct Stub;
    #[async_trait::async_trait]
    impl herdash::summary::Summarizer for Stub {
        async fn summarize_agent(&self, _t: &str) -> anyhow::Result<AgentSummary> {
            Ok(AgentSummary {
                headline: "h".into(),
                task: "t".into(),
                now: "n".into(),
                recent: vec![],
                needs_attention: false,
                attention_reason: String::new(),
            })
        }
        async fn summarize_fleet(&self, _h: &[String]) -> anyhow::Result<String> {
            Ok("fleet".into())
        }
    }
    let _: Box<dyn herdash::summary::Summarizer> = Box::new(Stub);
}

#[test]
fn the_schema_requires_the_attention_classification() {
    let b = agent_request_body("m", "t");
    let schema = &b["response_format"]["json_schema"]["schema"];
    let required = schema["required"].as_array().unwrap();
    for field in [
        "headline",
        "task",
        "now",
        "recent",
        "needs_attention",
        "attention_reason",
    ] {
        assert!(required.iter().any(|r| r == field), "missing {field}");
    }
    assert_eq!(schema["properties"]["needs_attention"]["type"], "boolean");
}

#[test]
fn an_attention_flag_and_reason_are_parsed() {
    let body = wrap(
        r#"{"headline":"h","task":"t","now":"n","recent":[],"needs_attention":true,"attention_reason":"Approve the migration"}"#,
    );
    let s = parse_agent_response(&body).unwrap();
    assert!(s.needs_attention);
    assert_eq!(s.attention_reason, "Approve the migration");
}

/// A flag with no reason is unactionable, so it is downgraded rather than
/// shown as a mystery alert the reader cannot resolve.
#[test]
fn a_flag_without_a_reason_is_cleared() {
    let body = wrap(
        r#"{"headline":"h","task":"t","now":"n","recent":[],"needs_attention":true,"attention_reason":"   "}"#,
    );
    let s = parse_agent_response(&body).unwrap();
    assert!(!s.needs_attention);
    assert!(s.attention_reason.is_empty());
}

#[test]
fn an_overlong_attention_reason_is_truncated() {
    let long = "x".repeat(400);
    let body = wrap(
        &serde_json::json!({
            "headline":"h","task":"t","now":"n","recent":[],
            "needs_attention": true, "attention_reason": long
        })
        .to_string(),
    );
    let s = parse_agent_response(&body).unwrap();
    assert!(s.attention_reason.chars().count() <= 80);
    assert!(s.needs_attention);
}

/// Older payloads without the new fields must still parse.
#[test]
fn a_response_without_attention_fields_defaults_to_no_attention() {
    let body = wrap(r#"{"headline":"h","task":"t","now":"n","recent":[]}"#);
    let s = parse_agent_response(&body).unwrap();
    assert!(!s.needs_attention);
}

#[test]
fn reasoning_modes_escalate_in_cost_order() {
    use herdash::summary::openrouter::ReasoningMode as M;
    assert_eq!(M::Disabled.escalate(), Some(M::LowEffort));
    assert_eq!(M::LowEffort.escalate(), Some(M::ProviderDefault));
    assert_eq!(M::ProviderDefault.escalate(), None, "nothing left to try");
}

#[test]
fn the_default_body_asks_for_no_reasoning() {
    let b = agent_request_body("m", "t");
    assert_eq!(b["reasoning"]["enabled"], false);
}

#[test]
fn each_reasoning_mode_produces_the_right_field() {
    use herdash::summary::openrouter::{ReasoningMode as M, agent_request_body_with};
    assert_eq!(
        agent_request_body_with("m", "t", M::Disabled)["reasoning"]["enabled"],
        false
    );
    assert_eq!(
        agent_request_body_with("m", "t", M::LowEffort)["reasoning"]["effort"],
        "low"
    );
    assert!(
        agent_request_body_with("m", "t", M::ProviderDefault)
            .get("reasoning")
            .is_none(),
        "the default sends no field at all"
    );
}

/// Only a provider refusing the mode should trigger escalation — a rate limit
/// or a schema error must surface as a real failure.
#[test]
fn only_reasoning_refusals_are_treated_as_escalation_signals() {
    use herdash::summary::openrouter::is_reasoning_rejection;
    assert!(is_reasoning_rejection(
        "Reasoning is mandatory for this endpoint and cannot be disabled."
    ));
    assert!(is_reasoning_rejection(
        "reasoning cannot be disabled for this model"
    ));
    assert!(!is_reasoning_rejection("rate limited"));
    assert!(!is_reasoning_rejection(
        "model did not return the requested schema"
    ));
}
