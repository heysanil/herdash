//! Domain model: herdr's flat agent list merged with workspace metadata,
//! grouped by repository and ordered by how much each agent wants attention.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::herdr::types::{AgentStatus, Snapshot, WorkspaceInfo};

/// A single agent, ready to render.
#[derive(Debug, Clone)]
pub struct Agent {
    /// Stable identity across re-sorts and list churn.
    pub pane_id: String,
    pub workspace_id: String,
    /// Agent kind, e.g. "claude".
    pub kind: String,
    /// Terminal title, used when no summary is available.
    pub title: String,
    /// Workspace label — the preferred display name.
    pub label: String,
    /// `None` for workspaces with no git checkout.
    pub repo: Option<String>,
    pub cwd: String,
    pub status: AgentStatus,
    pub revision: u64,
    pub state_change_seq: u64,
    /// When this status was first observed. See [`Timings`].
    pub status_since: Instant,
    /// True while the age is only a lower bound (agent predates herdash).
    pub age_is_lower_bound: bool,
}

/// Agents sharing a repository, or the `ungrouped` bucket.
#[derive(Debug, Clone)]
pub struct RepoGroup {
    pub repo: Option<String>,
    pub agents: Vec<Agent>,
}

impl RepoGroup {
    pub fn name(&self) -> &str {
        self.repo.as_deref().unwrap_or("ungrouped")
    }

    /// Urgency of the most urgent member; empty groups rank last.
    fn urgency(&self) -> u8 {
        self.agents
            .iter()
            .map(|a| a.status.urgency())
            .min()
            .unwrap_or(u8::MAX)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub total: usize,
    pub blocked: usize,
    pub done: usize,
    pub working: usize,
    pub idle: usize,
    pub unknown: usize,
}

#[derive(Debug, Clone, Copy)]
struct Timing {
    status_since: Instant,
    last_seq: u64,
    /// False until we observe this agent actually change state.
    seen_transition: bool,
}

/// Tracks how long each agent has held its current status.
///
/// herdr exposes no timestamp for a status, only a monotonic
/// `state_change_seq` per agent. We watch that counter and stamp our own
/// clock when it moves, which makes ages exact from the first observed
/// transition onward and a lower bound before that.
#[derive(Debug, Default)]
pub struct Timings {
    map: HashMap<String, Timing>,
}

impl Timings {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// Fold a fresh snapshot in, resetting clocks for agents that changed
    /// state and dropping agents that no longer exist.
    pub fn observe(&mut self, snapshot: &Snapshot, now: Instant) {
        for a in &snapshot.agents {
            match self.map.get_mut(&a.pane_id) {
                Some(t) if t.last_seq != a.state_change_seq => {
                    t.status_since = now;
                    t.last_seq = a.state_change_seq;
                    t.seen_transition = true;
                }
                Some(_) => {}
                None => {
                    self.map.insert(
                        a.pane_id.clone(),
                        Timing {
                            status_since: now,
                            last_seq: a.state_change_seq,
                            seen_transition: false,
                        },
                    );
                }
            }
        }
        let live: HashSet<&str> = snapshot.agents.iter().map(|a| a.pane_id.as_str()).collect();
        self.map.retain(|k, _| live.contains(k.as_str()));
    }
}

/// Merge agents with their workspace metadata, filter, group and sort.
pub fn build(snapshot: &Snapshot, timings: &Timings, active_only: bool) -> Vec<RepoGroup> {
    let workspaces: HashMap<&str, &WorkspaceInfo> = snapshot
        .workspaces
        .iter()
        .map(|w| (w.workspace_id.as_str(), w))
        .collect();

    let fallback = Instant::now();
    let mut by_repo: HashMap<Option<String>, Vec<Agent>> = HashMap::new();

    for info in &snapshot.agents {
        if active_only && matches!(info.agent_status, AgentStatus::Idle | AgentStatus::Unknown) {
            continue;
        }
        let ws = workspaces.get(info.workspace_id.as_str());
        let repo = ws
            .and_then(|w| w.worktree.as_ref())
            .map(|wt| wt.repo_name.clone())
            .filter(|r| !r.is_empty());
        // Every one of these can be null on the wire, so fall back down a
        // chain that ends at the pane id — a row must always be identifiable.
        let label = ws
            .map(|w| w.label.clone())
            .filter(|l| !l.is_empty())
            .or_else(|| Some(info.terminal_title_stripped.clone()).filter(|t| !t.is_empty()))
            .unwrap_or_else(|| info.pane_id.clone());
        let kind = if info.agent.is_empty() {
            "unknown".to_string()
        } else {
            info.agent.clone()
        };
        let t = timings.map.get(&info.pane_id);

        by_repo.entry(repo.clone()).or_default().push(Agent {
            pane_id: info.pane_id.clone(),
            workspace_id: info.workspace_id.clone(),
            kind,
            title: info.terminal_title_stripped.clone(),
            label,
            repo,
            cwd: info.cwd.clone(),
            status: info.agent_status,
            revision: info.revision,
            state_change_seq: info.state_change_seq,
            status_since: t.map(|t| t.status_since).unwrap_or(fallback),
            age_is_lower_bound: t.map(|t| !t.seen_transition).unwrap_or(true),
        });
    }

    let mut groups: Vec<RepoGroup> = by_repo
        .into_iter()
        .map(|(repo, mut agents)| {
            // Urgency, then longest-in-state first, then label for stability.
            agents.sort_by(|a, b| {
                a.status
                    .urgency()
                    .cmp(&b.status.urgency())
                    .then(a.status_since.cmp(&b.status_since))
                    .then(a.label.cmp(&b.label))
            });
            RepoGroup { repo, agents }
        })
        .filter(|g| !g.agents.is_empty())
        .collect();

    // Groups rank by their most urgent member. `ungrouped` loses ties, but
    // never outranks urgency — burying a blocked agent because its workspace
    // has no git checkout would defeat the dashboard's purpose.
    groups.sort_by(|a, b| {
        a.urgency()
            .cmp(&b.urgency())
            .then(a.repo.is_none().cmp(&b.repo.is_none()))
            .then(a.name().cmp(b.name()))
    });
    groups
}

/// Tally statuses across every group, for the header line.
pub fn counts(groups: &[RepoGroup]) -> Counts {
    let mut c = Counts::default();
    for a in groups.iter().flat_map(|g| g.agents.iter()) {
        c.total += 1;
        match a.status {
            AgentStatus::Blocked => c.blocked += 1,
            AgentStatus::Done => c.done += 1,
            AgentStatus::Working => c.working += 1,
            AgentStatus::Idle => c.idle += 1,
            AgentStatus::Unknown => c.unknown += 1,
        }
    }
    c
}
