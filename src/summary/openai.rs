//! OpenAI v1 Chat Completions codec.
//!
//! Pure: body construction and parsing only, so every shape is testable
//! without a socket. What varies between OpenAI-wire providers is carried by
//! [`Dialect`], not by this module.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use super::prompts::{
    AGENT_SYSTEM, FLEET_SYSTEM, TRANSCRIPT_MAX_BYTES, agent_max_tokens, clamp_transcript,
    fleet_max_tokens, summary_schema,
};
use super::provider::{Dialect, ReasoningMode};
use super::types::AgentSummary;

/// The reasoning field this dialect uses at this rung, if any.
///
/// OpenRouter takes a `reasoning` object; OpenAI direct takes a flat
/// `reasoning_effort` string and rejects `reasoning` as an unrecognized
/// argument; generic endpoints have no knob at all.
pub fn reasoning_field(d: Dialect, rung: ReasoningMode) -> Option<(&'static str, Value)> {
    match (d, rung) {
        (Dialect::OpenRouter, ReasoningMode::Disabled) => {
            Some(("reasoning", json!({ "enabled": false })))
        }
        (Dialect::OpenRouter, ReasoningMode::LowEffort) => {
            Some(("reasoning", json!({ "effort": "low" })))
        }
        (Dialect::OpenAiDirect, ReasoningMode::LowEffort) => {
            Some(("reasoning_effort", json!("low")))
        }
        _ => None,
    }
}

fn base_body(d: Dialect, model: &str, max_tokens: u32, messages: Value) -> Value {
    let mut body = json!({ "model": model, "messages": messages });
    body[d.token_cap_field()] = json!(max_tokens);
    if d.sends_temperature() {
        body["temperature"] = json!(0.2);
    }
    body
}

/// Request body for one agent summary, demanding a strict JSON schema.
pub fn agent_request_body(d: Dialect, model: &str, transcript: &str, rung: ReasoningMode) -> Value {
    let messages = json!([
        { "role": "system", "content": AGENT_SYSTEM },
        {
            "role": "user",
            "content": format!(
                "Transcript:\n{}",
                clamp_transcript(transcript, TRANSCRIPT_MAX_BYTES)
            )
        }
    ]);
    let mut body = base_body(d, model, agent_max_tokens(d.reasons_at(rung)), messages);
    body["response_format"] = json!({
        "type": "json_schema",
        "json_schema": {
            "name": "agent_summary",
            "strict": true,
            "schema": summary_schema()
        }
    });
    if let Some((key, value)) = reasoning_field(d, rung) {
        body[key] = value;
    }
    body
}

/// Request body for the fleet overview — plain prose, no schema.
///
/// `response_format` is omitted rather than set to `text`: LM Studio
/// historically rejected any value but `json_schema`, and omitting the field
/// is valid everywhere.
pub fn fleet_request_body(
    d: Dialect,
    model: &str,
    headlines: &[String],
    rung: ReasoningMode,
) -> Value {
    let messages = json!([
        { "role": "system", "content": FLEET_SYSTEM },
        { "role": "user", "content": headlines.join("\n") }
    ]);
    let mut body = base_body(d, model, fleet_max_tokens(d.reasons_at(rung)), messages);
    if let Some((key, value)) = reasoning_field(d, rung) {
        body[key] = value;
    }
    body
}

fn first_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Pull `choices[0].message.content`, distinguishing an empty body caused by
/// truncation or refusal from a model that ignored the schema.
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
    let choice = &v["choices"][0];
    let content = choice["message"]["content"].as_str().unwrap_or("");
    if content.trim().is_empty() {
        // Reporting this as a schema failure would misattribute a spent
        // budget to a disobedient model.
        match choice["finish_reason"].as_str() {
            Some("length") => bail!(
                "response truncated (finish_reason \"length\") — the token budget \
                 was consumed before any content was returned"
            ),
            Some(other) if other != "stop" => bail!("provider stopped early: {other}"),
            _ => bail!("provider response had no message content"),
        }
    }
    Ok(content.to_string())
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
