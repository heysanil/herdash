//! Value types for LLM-written agent summaries.

use serde::{Deserialize, Serialize};

/// Maximum rendered length of a sidebar headline.
pub const HEADLINE_MAX: usize = 60;
/// Maximum number of "recent work" bullets kept.
pub const RECENT_MAX: usize = 6;
/// Maximum rendered length of an attention reason.
pub const ATTENTION_MAX: usize = 100;
/// Rendered in place of an empty field.
pub const DASH: &str = "—";

/// One agent's summary, as returned by the model's strict JSON schema.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSummary {
    /// One-line gist for the sidebar.
    pub headline: String,
    /// The overall objective the agent is pursuing.
    pub task: String,
    /// What the agent is doing at this moment, in a sentence or three.
    pub now: String,
    /// Recently completed steps, newest first.
    #[serde(default)]
    pub recent: Vec<String>,
    /// Whether the agent is waiting on the human for something.
    ///
    /// Judged from the transcript, not from herdr's lifecycle state: an agent
    /// can be `working` yet stuck on a question it already asked, or `idle`
    /// simply because it finished cleanly and needs nothing.
    #[serde(default)]
    pub needs_attention: bool,
    /// What it is waiting for. Empty unless `needs_attention`.
    #[serde(default)]
    pub attention_reason: String,
}

impl AgentSummary {
    /// Trim, bound and de-blank a model response.
    ///
    /// The strict JSON schema guarantees shape but not length or usefulness,
    /// so the headline is truncated on a character boundary and blank bullets
    /// are dropped rather than rendered as empty rows.
    pub fn sanitized(mut self) -> Self {
        self.headline = truncate_chars(self.headline.trim(), HEADLINE_MAX);
        self.task = self.task.trim().to_string();
        self.now = self.now.trim().to_string();
        self.recent = self
            .recent
            .into_iter()
            .map(|r| r.trim().to_string())
            .filter(|r| !r.is_empty())
            .take(RECENT_MAX)
            .collect();
        self.attention_reason = truncate_chars(self.attention_reason.trim(), ATTENTION_MAX);
        // A reason without the flag is meaningless, and a flag without a
        // reason is unactionable — keep the two consistent.
        if self.attention_reason.is_empty() {
            self.needs_attention = false;
        }
        self
    }
}

/// Truncate to at most `max` characters, appending an ellipsis when cut.
/// Operates on `char`s so multi-byte text is never split mid-codepoint.
pub fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let kept: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{}…", kept.trim_end())
}

/// Render a field, substituting an em dash when the model left it blank.
pub fn or_dash(s: &str) -> &str {
    if s.trim().is_empty() { DASH } else { s }
}
