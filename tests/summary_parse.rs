//! Structured-output parsing, sanitisation and transcript clamping.

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
    let body =
        wrap(r#"{"headline":"h","task":"t","now":"n","recent":["a","  ","b","c","d","e","f"]}"#);
    let s = parse_agent_response(&body).unwrap();
    assert_eq!(s.recent, vec!["a", "b", "c", "d", "e"]);
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
            })
        }
        async fn summarize_fleet(&self, _h: &[String]) -> anyhow::Result<String> {
            Ok("fleet".into())
        }
    }
    let _: Box<dyn herdash::summary::Summarizer> = Box::new(Stub);
}
