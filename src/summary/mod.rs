//! LLM summarization of agent transcripts.

pub mod openrouter;
pub mod policy;
pub mod types;

use anyhow::Result;
use async_trait::async_trait;

pub use types::{AgentSummary, or_dash};

/// Produces summaries. Behind a trait so tests substitute a stub and never
/// touch the network.
#[async_trait]
pub trait Summarizer: Send + Sync {
    /// Summarize one agent's transcript.
    async fn summarize_agent(&self, transcript: &str) -> Result<AgentSummary>;
    /// Fold per-agent headlines into a one-or-two sentence fleet overview.
    async fn summarize_fleet(&self, headlines: &[String]) -> Result<String>;
}
