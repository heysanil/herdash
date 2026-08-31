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
        /// Revision the transcript was actually read at.
        revision: u64,
        result: Result<AgentSummary, String>,
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
            let slot = app.slots.entry(pane_id).or_default();
            slot.state.in_flight = false;
            // `forced` is deliberately NOT cleared here. The user may have
            // pressed `r` after this call was dispatched; clearing would
            // silently swallow that request. It is cleared on dispatch.
            match result {
                Ok(summary) => {
                    slot.summary = Some(summary);
                    slot.error = None;
                    slot.state.from_revision = Some(revision);
                    slot.state.generated_at = Some(Instant::now());
                    slot.state.failures = 0;
                    slot.state.retry_after = None;
                }
                Err(err) => {
                    slot.state.failures += 1;
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

/// Observe every visible agent's status and return the summaries to start.
///
/// Status observation happens for *all* agents, including ones we skip, so
/// urgent transitions are latched even while a call is in flight.
pub fn plan_summaries(app: &mut App, now: Instant, cfg: &Cfg) -> Vec<SummaryJob> {
    let agents: Vec<Agent> = app.agents().into_iter().cloned().collect();
    let mut jobs = Vec::new();
    for agent in agents {
        let slot = app.slots.entry(agent.pane_id.clone()).or_default();
        policy::observe_status(&mut slot.state, agent.status);
        if policy::decide(&agent, &slot.state, now, cfg) == Decision::Skip {
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
    app: &App,
    job: &mut FleetJob,
    now: Instant,
    cooldown: Duration,
) -> Option<FleetRequest> {
    if job.in_flight {
        return None;
    }
    let headlines: Vec<String> = app
        .agents()
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
    job.last_attempt = Some(now);
    Some(FleetRequest { hash, headlines })
}

fn hash_of(headlines: &[String]) -> u64 {
    let mut h = DefaultHasher::new();
    headlines.hash(&mut h);
    h.finish()
}
