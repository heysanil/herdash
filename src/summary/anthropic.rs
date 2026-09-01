//! Anthropic Messages codec.
//!
//! Pure, like [`super::openai`]. Both Anthropic-wire presets share one
//! dialect, so nothing here is parameterized by it.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::prompts::{
    AGENT_SYSTEM, FLEET_SYSTEM, TRANSCRIPT_MAX_BYTES, agent_max_tokens, clamp_transcript,
    fleet_max_tokens, summary_schema,
};
use super::provider::ReasoningMode;
use super::types::AgentSummary;

/// Required on every request. Not a beta header — structured output at
/// `output_config.format` is GA.
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

fn reasons(rung: ReasoningMode) -> bool {
    rung != ReasoningMode::Disabled
}

/// Apply the rung. `thinking` and `output_config.effort` are different
/// mechanisms: `{type: "disabled"}` is accepted on Opus 5 / Sonnet 5 /
/// Opus 4.7-4.8 but rejected on Fable 5, while `effort` is rejected on
/// Haiku 4.5 and Sonnet 4.5. The ladder negotiates between them.
fn apply_rung(body: &mut Value, rung: ReasoningMode) {
    match rung {
        ReasoningMode::Disabled => body["thinking"] = json!({ "type": "disabled" }),
        // `output_config` may already carry `format`; merge rather than replace.
        ReasoningMode::LowEffort => body["output_config"]["effort"] = json!("low"),
        ReasoningMode::ProviderDefault => {}
    }
}

/// Request body for one agent summary.
///
/// No `temperature`, `top_p` or `top_k`: sampling parameters were removed on
/// current Claude models and sending one is a 400.
pub fn agent_request_body(model: &str, transcript: &str, rung: ReasoningMode) -> Value {
    let mut body = json!({
        "model": model,
        "max_tokens": agent_max_tokens(reasons(rung)),
        "system": AGENT_SYSTEM,
        "messages": [{
            "role": "user",
            "content": format!(
                "Transcript:\n{}",
                clamp_transcript(transcript, TRANSCRIPT_MAX_BYTES)
            )
        }],
        // No `name`, no `strict` — unlike OpenAI's response_format.
        "output_config": { "format": { "type": "json_schema", "schema": summary_schema() } }
    });
    apply_rung(&mut body, rung);
    body
}

/// Request body for the fleet overview — plain prose, no schema.
pub fn fleet_request_body(model: &str, headlines: &[String], rung: ReasoningMode) -> Value {
    let mut body = json!({
        "model": model,
        "max_tokens": fleet_max_tokens(reasons(rung)),
        "system": FLEET_SYSTEM,
        "messages": [{ "role": "user", "content": headlines.join("\n") }]
    });
    apply_rung(&mut body, rung);
    body
}

fn first_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Pull the first `text` block, distinguishing truncation and refusal from a
/// model that ignored the schema.
fn content_of(body: &str) -> Result<String> {
    let v: Value = serde_json::from_str(body)
        .with_context(|| format!("provider returned non-JSON: {}", first_chars(body, 200)))?;
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        bail!("provider error: {msg}");
    }
    // A response may lead with a thinking block, so scan rather than index.
    let text = v["content"]
        .as_array()
        .and_then(|blocks| {
            blocks
                .iter()
                .find(|b| b["type"] == "text")
                .and_then(|b| b["text"].as_str())
        })
        .unwrap_or("");
    if text.trim().is_empty() {
        match v["stop_reason"].as_str() {
            Some("max_tokens") => bail!(
                "response truncated (stop_reason \"max_tokens\") — the token budget \
                 was consumed before any content was returned"
            ),
            Some(other) if other != "end_turn" => bail!("provider stopped early: {other}"),
            _ => bail!("provider response had no text content"),
        }
    }
    Ok(text.to_string())
}

/// Parse and sanitize an agent summary response.
pub fn parse_agent_response(body: &str) -> Result<AgentSummary> {
    let content = content_of(body)?;
    let summary: AgentSummary = serde_json::from_str(content.trim()).with_context(|| {
        format!(
            "model did not return the requested schema: {}",
            first_chars(&content, 200)
        )
    })?;
    Ok(summary.sanitized())
}

/// Parse a fleet overview response.
pub fn parse_fleet_response(body: &str) -> Result<String> {
    Ok(content_of(body)?.trim().to_string())
}
