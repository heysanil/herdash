//! Coordination between herdr polling, summarisation and app state.
//!
//! This lives in the library rather than in `main.rs` on purpose: it holds the
//! trickiest state transitions in the program — latched bypasses, forced
//! refreshes racing an in-flight call, failure backoff, fleet-summary
//! throttling — and none of that is testable from inside a binary.
//!
//! Every function here is synchronous and clock-injected. The binary supplies
//! `now`, performs the I/O the returned jobs describe, and feeds results back
//! through [`apply_update`].

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

use crate::app::{App, ConnState};
use crate::fleet::Agent;
use crate::herdr::types::Snapshot;
use crate::summary::AgentSummary;
use crate::summary::policy::{self, Cfg, Decision};

/// Minimum gap between fleet-overview attempts.
pub const FLEET_COOLDOWN: Duration = Duration::from_secs(120);

/// Results flowing back into the app from worker tasks.
#[derive(Debug)]
pub enum Update {
    Snapshot(Box<Snapshot>),
    Connection(ConnState),
    /// One-line transient message for the header.
    Notice(String),
    Summary {
        pane_id: String,
        /// Snapshot revision this attempt was dispatched for.
        revision: u64,
        /// `Ok(None)` means the pane had nothing worth summarising — a normal
        /// outcome, not a failure, so it must not trigger error backoff.
        result: Result<Option<AgentSummary>, String>,
    },
    Fleet {
        /// Hash of the headlines this call was made from.
        hash: u64,
        result: Result<String, String>,
    },
}

/// Bookkeeping for the single fleet-overview call.
#[derive(Debug, Default, Clone)]
pub struct FleetJob {
    pub in_flight: bool,
    /// Hash of the request currently in flight, so a response that arrives
    /// after the fleet has moved on can be discarded rather than displayed.
    pub active_hash: Option<u64>,
    pub last_attempt: Option<Instant>,
    /// Hash of the headlines that produced the *current* summary. Only a
    /// success commits here, so a failed call is retried rather than
    /// suppressed forever by an unchanged input.
    pub last_success_hash: Option<u64>,
}

/// An agent that should be summarised now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryJob {
    pub pane_id: String,
    /// Revision the snapshot reported, used only if the transcript read
    /// itself fails and cannot report its own.
    pub revision: u64,
}

/// A fleet overview that should be generated now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FleetRequest {
    pub hash: u64,
    pub headlines: Vec<String>,
}

/// Fold a worker result into app state.
pub fn apply_update(app: &mut App, fleet: &mut FleetJob, update: Update) {
    match update {
        Update::Snapshot(snapshot) => {
            app.conn = ConnState::Connected;
            app.notice = None;
            app.apply_snapshot(&snapshot);
        }
        Update::Connection(state) => app.conn = state,
        Update::Notice(text) => app.notice = Some(text),
        Update::Summary {
            pane_id,
            revision,
            result,
        } => {
            // Never `entry().or_default()` here. A worker can outlive the
            // agent it describes, and recreating the slot would resurrect a
            // dead agent's summary — which the next snapshot would then have
            // to prune again. (herdr never reuses a closed pane id, so a late
            // result cannot be misattributed to a different agent.)
            let Some(slot) = app.slots.get_mut(&pane_id) else {
                return;
            };
            slot.state.in_flight = false;
            // `forced` is deliberately NOT cleared here. The user may have
            // pressed `r` after this call was dispatched; clearing would
            // silently swallow that request. It is cleared on dispatch.
            match result {
                Ok(None) => {
                    // An empty pane is a fact, not a fault. Record the
                    // revision so we do not re-read it every backoff tick.
                    slot.error = None;
                    slot.state.from_revision = Some(revision);
                    slot.state.failures = 0;
                    slot.state.retry_after = None;
                }
                Ok(Some(summary)) => {
                    slot.summary = Some(summary);
                    slot.error = None;
                    slot.state.from_revision = Some(revision);
                    slot.state.generated_at = Some(Instant::now());
                    slot.state.failures = 0;
                    slot.state.retry_after = None;
                }
                Err(err) => {
                    slot.state.failures = slot.state.failures.saturating_add(1);
                    slot.state.retry_after =
                        Some(Instant::now() + policy::backoff(slot.state.failures));
                    slot.state.from_revision = Some(revision);
                    // The previous summary stays on screen; the error annotates it.
                    slot.error = Some(first_line(&err));
                }
            }
        }
        Update::Fleet { hash, result } => {
            fleet.in_flight = false;
            // Discard a response for a fleet that no longer exists.
            if fleet.active_hash != Some(hash) {
                return;
            }
            fleet.active_hash = None;
            if let Ok(text) = result {
                app.fleet_summary = Some(text);
                app.fleet_generated_at = Some(Instant::now());
                fleet.last_success_hash = Some(hash);
            }
        }
    }
}

fn first_line(msg: &str) -> String {
    msg.lines()
        .next()
        .unwrap_or(msg)
        .chars()
        .take(120)
        .collect()
}

/// Observe every agent's status and return up to `capacity` summaries to start.
///
/// Status observation happens for *every* agent, including ones we skip and
/// ones beyond the capacity limit, so urgent transitions are latched even
/// while a call is in flight or a worker slot is unavailable.
///
/// Iterates the unfiltered fleet: hiding idle agents is a viewing preference,
/// not an instruction to stop describing them.
pub fn plan_summaries(app: &mut App, now: Instant, cfg: &Cfg, capacity: usize) -> Vec<SummaryJob> {
    // Unfiltered: a display filter must not silently stop summarisation.
    let agents: Vec<Agent> = app.all_agents();
    let mut jobs = Vec::new();
    for agent in agents {
        let slot = app.slots.entry(agent.pane_id.clone()).or_default();
        policy::observe_status(&mut slot.state, agent.status);
        if policy::decide(&agent, &slot.state, now, cfg) == Decision::Skip {
            continue;
        }
        // Out of worker slots. Leave `forced` and `pending_bypass` intact so
        // this agent is picked up on a later pass rather than losing its
        // trigger — status is already observed above, so no edge is missed.
        if jobs.len() >= capacity {
            continue;
        }
        // Consume the one-shot triggers only when work actually starts.
        slot.state.in_flight = true;
        slot.state.forced = false;
        slot.state.pending_bypass = false;
        jobs.push(SummaryJob {
            pane_id: agent.pane_id.clone(),
            revision: agent.revision,
        });
    }
    jobs
}

/// Mark one agent for an unconditional re-summary.
pub fn force_one(app: &mut App, pane_id: &str) {
    app.slots
        .entry(pane_id.to_string())
        .or_default()
        .state
        .forced = true;
}

/// Mark every agent in the snapshot — including ones the filter hides.
pub fn force_all(app: &mut App) {
    for id in app.all_agent_ids() {
        app.slots.entry(id).or_default().state.forced = true;
    }
}

/// Decide whether to regenerate the fleet overview.
///
/// Skipped below two summaries: with one agent the header would merely
/// restate the detail pane.
pub fn plan_fleet(
    app: &mut App,
    job: &mut FleetJob,
    now: Instant,
    cooldown: Duration,
) -> Option<FleetRequest> {
    if job.in_flight {
        return None;
    }
    let headlines: Vec<String> = app
        .all_agents()
        .iter()
        .filter_map(|a| {
            app.slots
                .get(&a.pane_id)
                .and_then(|s| s.summary.as_ref())
                .map(|s| {
                    format!(
                        "{} [{}] {} — {}",
                        a.label,
                        a.status.as_str(),
                        s.headline,
                        s.task
                    )
                })
        })
        .collect();
    if headlines.len() < 2 {
        // The fleet shrank below the threshold. An overview describing agents
        // that have since departed is worse than no overview at all, so drop
        // it rather than leaving stale prose in the header forever.
        app.fleet_summary = None;
        app.fleet_generated_at = None;
        job.last_success_hash = None;
        job.active_hash = None;
        return None;
    }
    let hash = hash_of(&headlines);
    // Nothing new to say.
    if job.last_success_hash == Some(hash) {
        return None;
    }
    if let Some(at) = job.last_attempt
        && now.duration_since(at) < cooldown
    {
        return None;
    }
    job.in_flight = true;
    job.active_hash = Some(hash);
    job.last_attempt = Some(now);
    Some(FleetRequest { hash, headlines })
}

fn hash_of(headlines: &[String]) -> u64 {
    let mut h = DefaultHasher::new();
    headlines.hash(&mut h);
    h.finish()
}
