//! The one file in `summary` that performs I/O.
//!
//! Owns the reqwest client, per-wire headers, and the reasoning-rung
//! escalation loop. Body construction and parsing live in the pure codecs.

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::Value;

use super::provider::{ReasoningMode, ResolvedProvider, Wire};
use super::types::AgentSummary;
use super::{Summarizer, anthropic, openai};

/// True when a failure is this provider refusing the current reasoning rung
/// rather than a real error worth surfacing.
///
/// All three conditions must hold. Vendors expose no machine-readable code
/// for "you may not disable reasoning", so the field-name check stays
/// textual — but gating it on the status and on whether the current rung
/// actually sent a reasoning field is what stops an unrelated 400 from being
/// silently retried two more times at cost and then misreported. 429 is
/// excluded even though it falls in the 4xx range: a rate limit is never the
/// provider refusing the rung, and a request that merely got throttled can
/// still carry a stale error body that happens to name the reasoning field.
pub fn is_reasoning_rejection(status: u16, sent_reasoning: bool, message: &str) -> bool {
    if !(400..500).contains(&status) || status == 429 || !sent_reasoning {
        return false;
    }
    let m = message.to_ascii_lowercase();
    let names_the_field = m.contains("reasoning")
        || m.contains("reasoning_effort")
        || m.contains("thinking")
        || m.contains("effort");
    if !names_the_field {
        return false;
    }
    m.contains("mandatory")
        || m.contains("cannot be disabled")
        || m.contains("not supported")
        || m.contains("unsupported")
        || m.contains("unrecognized")
}

/// Summarizer over any configured provider.
pub struct LlmClient {
    client: reqwest::Client,
    provider: ResolvedProvider,
    /// Overridable so tests can point at a local stub — the reason the URL is
    /// stored rather than derived on every call.
    endpoint: String,
    /// Cached rung, advanced monotonically. Stored as an index so it can be
    /// shared without a lock.
    reasoning: std::sync::atomic::AtomicU8,
}

impl LlmClient {
    pub fn new(provider: ResolvedProvider) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(45))
            .build()
            .expect("reqwest client");
        let endpoint = provider.completions_url();
        let start = provider.dialect.start_rung().index();
        Self {
            client,
            provider,
            endpoint,
            reasoning: std::sync::atomic::AtomicU8::new(start),
        }
    }

    /// Point the client at a different completions endpoint.
    pub fn with_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.endpoint = endpoint.into();
        self
    }

    fn rung(&self) -> ReasoningMode {
        ReasoningMode::from_index(self.reasoning.load(std::sync::atomic::Ordering::Relaxed))
    }

    /// Monotonic on purpose — see the note in `openrouter.rs`'s original
    /// version: a late rejection must never lower the cached rung.
    fn advance(&self, mode: ReasoningMode) {
        self.reasoning
            .fetch_max(mode.index(), std::sync::atomic::Ordering::Relaxed);
    }

    fn agent_body(&self, transcript: &str, rung: ReasoningMode) -> Value {
        let d = self.provider.dialect;
        let m = &self.provider.model;
        match d.wire() {
            Wire::OpenAi => openai::agent_request_body(d, m, transcript, rung),
            Wire::Anthropic => anthropic::agent_request_body(m, transcript, rung),
        }
    }

    fn fleet_body(&self, headlines: &[String], rung: ReasoningMode) -> Value {
        let d = self.provider.dialect;
        let m = &self.provider.model;
        match d.wire() {
            Wire::OpenAi => openai::fleet_request_body(d, m, headlines, rung),
            Wire::Anthropic => anthropic::fleet_request_body(m, headlines, rung),
        }
    }

    /// Whether the given rung actually puts a reasoning field on the wire.
    fn sends_reasoning(&self, rung: ReasoningMode) -> bool {
        match self.provider.dialect.wire() {
            Wire::OpenAi => openai::reasoning_field(self.provider.dialect, rung).is_some(),
            Wire::Anthropic => rung != ReasoningMode::ProviderDefault,
        }
    }

    async fn post(&self, body: Value) -> Result<String, (u16, String)> {
        let mut req = self.client.post(&self.endpoint).json(&body);
        req = match self.provider.dialect.wire() {
            Wire::OpenAi => match &self.provider.api_key {
                Some(key) => req.bearer_auth(key),
                // Local servers accept an unauthenticated request; sending an
                // empty bearer token makes some of them 401 instead.
                None => req,
            },
            Wire::Anthropic => {
                let req = req.header("anthropic-version", anthropic::ANTHROPIC_VERSION);
                match &self.provider.api_key {
                    Some(key) => req.header("x-api-key", key),
                    None => req,
                }
            }
        };
        let resp = req
            .send()
            .await
            .map_err(|e| (0, format!("request to {} failed: {e}", self.endpoint)))?;
        let status = resp.status();
        let text = resp
            .text()
            .await
            .map_err(|e| (status.as_u16(), format!("could not read response: {e}")))?;
        if !status.is_success() {
            // Parse anyway: providers put a useful message in the error body.
            let msg = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
                .unwrap_or_else(|| text.chars().take(200).collect());
            return Err((status.as_u16(), msg));
        }
        Ok(text)
    }

    /// Run one request, escalating the rung on an explicit refusal.
    async fn call<T>(
        &self,
        build: impl Fn(ReasoningMode) -> Value,
        parse: impl Fn(&str) -> Result<T>,
    ) -> Result<T> {
        let mut rung = self.rung();
        loop {
            match self.post(build(rung)).await {
                Ok(text) => return parse(&text),
                Err((status, msg)) => {
                    let refused = is_reasoning_rejection(status, self.sends_reasoning(rung), &msg);
                    let next = rung.escalate();
                    match (refused, next) {
                        (true, Some(next)) => {
                            rung = next;
                            self.advance(next);
                        }
                        // At the top rung, or an unrelated failure: report it.
                        _ => bail!("{} {status}: {msg}", self.provider.id.as_str()),
                    }
                }
            }
        }
    }
}

#[async_trait]
impl Summarizer for LlmClient {
    async fn summarize_agent(&self, transcript: &str) -> Result<AgentSummary> {
        let wire = self.provider.dialect.wire();
        self.call(
            |rung| self.agent_body(transcript, rung),
            |text| match wire {
                Wire::OpenAi => openai::parse_agent_response(text),
                Wire::Anthropic => anthropic::parse_agent_response(text),
            },
        )
        .await
        .with_context(|| format!("summarizing with {}", self.provider.model))
    }

    async fn summarize_fleet(&self, headlines: &[String]) -> Result<String> {
        let wire = self.provider.dialect.wire();
        self.call(
            |rung| self.fleet_body(headlines, rung),
            |text| match wire {
                Wire::OpenAi => openai::parse_fleet_response(text),
                Wire::Anthropic => anthropic::parse_fleet_response(text),
            },
        )
        .await
    }
}
