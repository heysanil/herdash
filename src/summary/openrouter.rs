//! OpenRouter client using strict structured outputs.
//!
//! Body construction and parsing are free functions so they can be tested
//! without HTTP; only `post` touches the network.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::Summarizer;
use super::types::AgentSummary;

const ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";

/// Transcripts are clamped before sending. Generous enough for 200 lines of
/// agent output, small enough that a runaway pane cannot blow up a request.
pub const TRANSCRIPT_MAX_BYTES: usize = 12 * 1024;

const AGENT_SYSTEM: &str = "\
You summarize a coding agent's terminal transcript for a status dashboard. \
The transcript contains terminal chrome — status lines, progress bars, token \
counters, spinners, box-drawing characters. Ignore all of it and describe only \
the actual work. Write plainly, in the third person, with no preamble. \
`headline` must be at most 60 characters. `task` is the overall objective. \
`now` is what the agent is doing at this moment. `recent` lists at most five \
recently completed steps, newest first. If the transcript shows the agent \
waiting on a question or approval, say so explicitly in `now`. \
Respond only with the JSON object.";

const FLEET_SYSTEM: &str = "\
You summarize a fleet of coding agents for a dashboard header. Given one line \
per agent, write one or two sentences describing what the fleet is collectively \
doing and which agents need attention. Be specific and terse. No preamble, no \
bullet points, no markdown.";

/// Keep the tail of an over-long transcript, cut on a UTF-8 boundary and
/// preferably at a line break. Recent output is what matters.
pub fn clamp_transcript(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut start = text.len() - max_bytes;
    while start < text.len() && !text.is_char_boundary(start) {
        start += 1;
    }
    let tail = &text[start..];
    match tail.find('\n') {
        Some(i) if tail.len() > i + 1 => &tail[i + 1..],
        _ => tail,
    }
}

/// Request body for one agent summary, demanding a strict JSON schema.
pub fn agent_request_body(model: &str, transcript: &str) -> Value {
    json!({
        "model": model,
        "temperature": 0.2,
        "max_tokens": 500,
        "messages": [
            { "role": "system", "content": AGENT_SYSTEM },
            { "role": "user", "content": format!("Transcript:\n{}", clamp_transcript(transcript, TRANSCRIPT_MAX_BYTES)) }
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "agent_summary",
                "strict": true,
                "schema": {
                    "type": "object",
                    "properties": {
                        "headline": { "type": "string" },
                        "task": { "type": "string" },
                        "now": { "type": "string" },
                        "recent": { "type": "array", "items": { "type": "string" } }
                    },
                    "required": ["headline", "task", "now", "recent"],
                    "additionalProperties": false
                }
            }
        }
    })
}

/// Request body for the fleet overview — plain prose, no schema.
pub fn fleet_request_body(model: &str, headlines: &[String]) -> Value {
    json!({
        "model": model,
        "temperature": 0.3,
        "max_tokens": 200,
        "messages": [
            { "role": "system", "content": FLEET_SYSTEM },
            { "role": "user", "content": headlines.join("\n") }
        ]
    })
}

fn first_chars(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// Pull `choices[0].message.content` out of a completion response.
fn content_of(body: &str) -> Result<String> {
    let v: Value = serde_json::from_str(body)
        .with_context(|| format!("OpenRouter returned non-JSON: {}", first_chars(body, 200)))?;
    if let Some(err) = v.get("error") {
        let msg = err
            .get("message")
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        bail!("OpenRouter error: {msg}");
    }
    v["choices"][0]["message"]["content"]
        .as_str()
        .map(str::to_owned)
        .context("OpenRouter response had no message content")
}

/// Parse and sanitise an agent summary response.
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

/// OpenRouter-backed [`Summarizer`].
pub struct OpenRouter {
    client: reqwest::Client,
    api_key: String,
    model: String,
}

impl OpenRouter {
    pub fn new(api_key: String, model: String) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(45))
            .build()
            .expect("reqwest client");
        Self {
            client,
            api_key,
            model,
        }
    }

    async fn post(&self, body: Value) -> Result<String> {
        let resp = self
            .client
            .post(ENDPOINT)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send()
            .await
            .context("OpenRouter request failed")?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .context("could not read OpenRouter response")?;
        if !status.is_success() {
            // Parse anyway: OpenRouter puts a useful message in the error body.
            if let Ok(v) = serde_json::from_str::<Value>(&text)
                && let Some(msg) = v["error"]["message"].as_str()
            {
                bail!("OpenRouter {status}: {msg}");
            }
            bail!("OpenRouter {status}: {}", first_chars(&text, 200));
        }
        Ok(text)
    }
}

#[async_trait]
impl Summarizer for OpenRouter {
    async fn summarize_agent(&self, transcript: &str) -> Result<AgentSummary> {
        let body = self
            .post(agent_request_body(&self.model, transcript))
            .await?;
        parse_agent_response(&body)
    }

    async fn summarize_fleet(&self, headlines: &[String]) -> Result<String> {
        let body = self
            .post(fleet_request_body(&self.model, headlines))
            .await?;
        parse_fleet_response(&body)
    }
}
