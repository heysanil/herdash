//! When to spend an OpenRouter call on an agent.
//!
//! Deliberately pure: no clock, no I/O, no async. The caller supplies `now`,
//! so every branch is directly testable.

use std::time::{Duration, Instant};

use crate::fleet::Agent;
use crate::herdr::types::AgentStatus;

/// Base backoff after the first failure.
const BACKOFF_BASE: u64 = 5;
/// Backoff ceiling.
const BACKOFF_CAP: u64 = 300;

#[derive(Debug, Clone, Copy)]
pub struct Cfg {
    pub cooldown: Duration,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Skip,
    Summarize,
}

/// Per-agent bookkeeping the policy reads.
#[derive(Debug, Clone, Default)]
pub struct SummaryState {
    /// A call is outstanding; never start a second.
    pub in_flight: bool,
    /// The `revision` the current summary was generated from.
    pub from_revision: Option<u64>,
    /// When the current summary was produced.
    pub generated_at: Option<Instant>,
    /// Status at the time of the last observation, for edge detection.
    pub last_status: Option<AgentStatus>,
    /// An urgent transition was seen but no call could start yet.
    ///
    /// Edges are latched rather than consumed on sight: a `Working → Blocked`
    /// that lands while a call is in flight, or during backoff, must still
    /// force a summary once a worker is free. Without this the edge is lost
    /// and a blocked agent silently keeps a stale summary.
    pub pending_bypass: bool,
    /// Consecutive failures, driving [`backoff`].
    pub failures: u32,
    /// Earliest time a retry may be attempted.
    pub retry_after: Option<Instant>,
    /// Set by an explicit user refresh; cleared once consumed.
    pub forced: bool,
}

/// Exponential backoff: 5s, 15s, 45s, 135s, then capped at 300s.
pub fn backoff(failures: u32) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    let exp = failures.saturating_sub(1).min(10);
    let secs = BACKOFF_BASE.saturating_mul(3u64.saturating_pow(exp));
    Duration::from_secs(secs.min(BACKOFF_CAP))
}

/// Record the agent's current status, latching any urgent transition.
///
/// Must be called once per observation, *before* [`decide`]. Keeping the edge
/// in `pending_bypass` rather than acting on it immediately is what makes a
/// transition survive an in-flight call or an active backoff.
pub fn observe_status(st: &mut SummaryState, new: AgentStatus) {
    if is_bypass(new, st.last_status) {
        st.pending_bypass = true;
    }
    st.last_status = Some(new);
}

/// Decide whether to summarise `agent` right now.
///
/// Order matters: `in_flight` outranks even a forced refresh (we would only
/// duplicate work), and a forced refresh outranks backoff (the user asked).
pub fn decide(agent: &Agent, st: &SummaryState, now: Instant, cfg: &Cfg) -> Decision {
    if !cfg.enabled {
        return Decision::Skip;
    }
    if st.in_flight {
        return Decision::Skip;
    }
    if st.forced {
        return Decision::Summarize;
    }
    // A previous attempt failed. Retry purely on the backoff schedule — the
    // revision and cooldown gates below would otherwise strand a failed
    // summary forever, since a failure records the revision it attempted.
    if st.failures > 0 {
        return match st.retry_after {
            Some(at) if now < at => Decision::Skip,
            _ => Decision::Summarize,
        };
    }
    // Never summarised: a newly appeared agent always gets one immediately.
    let Some(from_revision) = st.from_revision else {
        return Decision::Summarize;
    };
    // No new output means nothing new to say.
    if agent.revision == from_revision {
        return Decision::Skip;
    }
    if st.pending_bypass {
        return Decision::Summarize;
    }
    match st.generated_at {
        Some(at) if now.duration_since(at) >= cfg.cooldown => Decision::Summarize,
        Some(_) => Decision::Skip,
        None => Decision::Summarize,
    }
}

/// Transitions urgent enough to ignore the cooldown.
///
/// Fires on the *edge*, not the level — a long-blocked agent must not
/// re-summarise on every tick.
fn is_bypass(new: AgentStatus, old: Option<AgentStatus>) -> bool {
    let Some(old) = old else { return false };
    if new == old {
        return false;
    }
    match (old, new) {
        // It is waiting on the user right now.
        (_, AgentStatus::Blocked) => true,
        // It just finished.
        (AgentStatus::Working, AgentStatus::Done | AgentStatus::Idle) => true,
        _ => false,
    }
}
