//! OpenRouter client using strict structured outputs.
//!
//! Body construction and parsing are free functions so they can be tested
//! without HTTP; only `post` touches the network.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::Summarizer;
use super::prompts::{AGENT_SYSTEM, FLEET_SYSTEM, summary_schema};
use super::types::AgentSummary;

const ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";

/// Re-exported so existing callers and tests keep their import path until
/// Task 7 removes this module.
pub use super::prompts::{TRANSCRIPT_MAX_BYTES, clamp_transcript};

/// How much chain-of-thought to ask a provider for.
///
/// Summarization is extraction, not deduction, so reasoning buys nothing here
/// and costs a great deal. But no single setting works everywhere, measured
/// across seven providers:
///
/// - `Disabled` is cheapest and is the only thing that makes kimi-k2.6 and
///   qwen3.5-35b work at all — without it they spend the entire token budget
///   thinking and return `finish_reason: "length"` with empty content, a call
///   you still pay for.
/// - The OpenAI and Gemini endpoints *reject* `Disabled` outright with
///   "Reasoning is mandatory for this endpoint", but accept `LowEffort`.
/// - `ProviderDefault` sends no field at all, for anything that rejects both.
///
/// So the client starts at `Disabled` and escalates on rejection, caching the
/// answer for the process lifetime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReasoningMode {
    Disabled,
    LowEffort,
    ProviderDefault,
}

impl ReasoningMode {
    /// The next mode to try when a provider rejects this one.
    pub fn escalate(self) -> Option<Self> {
        match self {
            Self::Disabled => Some(Self::LowEffort),
            Self::LowEffort => Some(Self::ProviderDefault),
            Self::ProviderDefault => None,
        }
    }

    fn field(self) -> Option<Value> {
        match self {
            Self::Disabled => Some(json!({ "enabled": false })),
            Self::LowEffort => Some(json!({ "effort": "low" })),
            Self::ProviderDefault => None,
        }
    }
}

/// The `reasoning` request field for a mode, if any.
pub fn reasoning_field(mode: ReasoningMode) -> Option<Value> {
    mode.field()
}

/// True when the failure is a provider refusing this reasoning mode, rather
/// than a real error worth surfacing.
pub fn is_reasoning_rejection(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    m.contains("reasoning is mandatory")
        || (m.contains("reasoning")
            && (m.contains("cannot be disabled") || m.contains("not supported")))
}

/// Request body for one agent summary, demanding a strict JSON schema.
pub fn agent_request_body(model: &str, transcript: &str) -> Value {
    agent_request_body_with(model, transcript, ReasoningMode::Disabled)
}

/// As [`agent_request_body`], with an explicit reasoning mode.
pub fn agent_request_body_with(model: &str, transcript: &str, reasoning: ReasoningMode) -> Value {
    let mut body = agent_body_inner(model, transcript);
    if let Some(field) = reasoning.field() {
        body["reasoning"] = field;
    }
    body
}

fn agent_body_inner(model: &str, transcript: &str) -> Value {
    json!({
        "model": model,
        "temperature": 0.2,
        "max_tokens": 900,
        "messages": [
            { "role": "system", "content": AGENT_SYSTEM },
            { "role": "user", "content": format!("Transcript:\n{}", clamp_transcript(transcript, TRANSCRIPT_MAX_BYTES)) }
        ],
        "response_format": {
            "type": "json_schema",
            "json_schema": {
                "name": "agent_summary",
                "strict": true,
                "schema": summary_schema()
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

/// Advance a cached reasoning rung. **Monotonic on purpose.**
///
/// Up to `MAX_SUMMARY_TASKS` workers share one cached rung. A worker that
/// started at a low rung, was refused, and returns after another worker
/// already escalated past it must not write its stale rung back — that makes
/// the ladder oscillate and re-pay for a refused call on every lap. A late
/// rejection is a no-op instead.
///
/// A free function so the invariant is testable without driving a race that
/// only reproduces under specific interleavings.
pub fn advance_rung(cache: &std::sync::atomic::AtomicU8, mode: ReasoningMode) {
    let index = match mode {
        ReasoningMode::Disabled => 0,
        ReasoningMode::LowEffort => 1,
        ReasoningMode::ProviderDefault => 2,
    };
    cache.fetch_max(index, std::sync::atomic::Ordering::Relaxed);
}

/// OpenRouter-backed [`Summarizer`].
pub struct OpenRouter {
    client: reqwest::Client,
    api_key: String,
    model: String,
    /// Overridable so tests can point at a local stub and assert the
    /// escalation sequence — the reason this is not a `const`.
    endpoint: String,
    /// Cached [`ReasoningMode`], escalated once if the provider rejects the
    /// cheapest form. Stored as an index so it can be shared without a lock.
    reasoning: std::sync::atomic::AtomicU8,
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
            endpoint: ENDPOINT.to_string(),
            reasoning: std::sync::atomic::AtomicU8::new(0),
        }
    }

    /// Point the client at a different completions endpoint.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    fn reasoning_mode(&self) -> ReasoningMode {
        match self.reasoning.load(std::sync::atomic::Ordering::Relaxed) {
            0 => ReasoningMode::Disabled,
            1 => ReasoningMode::LowEffort,
            _ => ReasoningMode::ProviderDefault,
        }
    }

    fn set_reasoning_mode(&self, mode: ReasoningMode) {
        advance_rung(&self.reasoning, mode);
    }

    async fn post(&self, body: Value) -> Result<String> {
        let resp = self
            .client
            .post(&self.endpoint)
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
        // Start at the cheapest reasoning mode and escalate only if this
        // provider explicitly refuses it, then remember the answer so the rest
        // of the session pays for one extra round trip rather than every call.
        let mut mode = self.reasoning_mode();
        loop {
            let body = agent_request_body_with(&self.model, transcript, mode);
            match self.post(body).await {
                Ok(text) => return parse_agent_response(&text),
                Err(err) if is_reasoning_rejection(&err.to_string()) => {
                    let Some(next) = mode.escalate() else {
                        return Err(err);
                    };
                    mode = next;
                    self.set_reasoning_mode(next);
                }
                Err(err) => return Err(err),
            }
        }
    }

    async fn summarize_fleet(&self, headlines: &[String]) -> Result<String> {
        let mut mode = self.reasoning_mode();
        loop {
            let mut body = fleet_request_body(&self.model, headlines);
            if let Some(field) = reasoning_field(mode) {
                body["reasoning"] = field;
            }
            match self.post(body).await {
                Ok(text) => return parse_fleet_response(&text),
                Err(err) if is_reasoning_rejection(&err.to_string()) => {
                    let Some(next) = mode.escalate() else {
                        return Err(err);
                    };
                    mode = next;
                    self.set_reasoning_mode(next);
                }
                Err(err) => return Err(err),
            }
        }
    }
}
