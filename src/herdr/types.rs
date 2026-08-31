//! Wire types for the herdr socket API (protocol 20).
//!
//! Every struct uses `#[serde(default)]` liberally and never denies unknown
//! fields, so a herdr upgrade that adds fields cannot break the dashboard.

use serde::Deserialize;

/// Agent lifecycle state as reported by herdr.
///
/// `Done` is the same underlying idle state as `Idle`, but reached after
/// unseen background work finished — so it ranks as more urgent. `Unknown`
/// means an agent is present but unclassified; it does **not** imply
/// completion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AgentStatus {
    Blocked,
    Done,
    Working,
    Idle,
    Unknown,
}

impl AgentStatus {
    /// Map a wire string to a status. Unrecognised values become
    /// [`Self::Unknown`] rather than a deserialisation error, so new herdr
    /// states degrade safely instead of blanking the dashboard.
    pub fn from_wire(s: &str) -> Self {
        match s {
            "blocked" => Self::Blocked,
            "done" => Self::Done,
            "working" => Self::Working,
            "idle" => Self::Idle,
            _ => Self::Unknown,
        }
    }

    /// Sort key expressing "how much does this want my attention", 0 = most.
    pub fn urgency(self) -> u8 {
        match self {
            Self::Blocked => 0,
            Self::Done => 1,
            Self::Working => 2,
            Self::Idle => 3,
            Self::Unknown => 4,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blocked => "blocked",
            Self::Done => "done",
            Self::Working => "working",
            Self::Idle => "idle",
            Self::Unknown => "unknown",
        }
    }
}

impl<'de> Deserialize<'de> for AgentStatus {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Ok(Self::from_wire(&s))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AgentInfo {
    /// Agent kind, e.g. "claude", "codex".
    pub agent: String,
    pub agent_status: AgentStatus,
    pub workspace_id: String,
    pub pane_id: String,
    #[serde(default)]
    pub tab_id: String,
    #[serde(default)]
    pub terminal_title_stripped: String,
    #[serde(default)]
    pub cwd: String,
    #[serde(default)]
    pub focused: bool,
    /// Increments as pane content changes — the summarisation change signal.
    #[serde(default)]
    pub revision: u64,
    /// Increments on lifecycle changes — the status-age reset signal.
    #[serde(default)]
    pub state_change_seq: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Worktree {
    #[serde(default)]
    pub repo_name: String,
    #[serde(default)]
    pub repo_root: String,
    #[serde(default)]
    pub checkout_path: String,
    #[serde(default)]
    pub is_linked_worktree: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    #[serde(default)]
    pub label: String,
    /// Absent for workspaces with no git checkout. Never index unconditionally.
    #[serde(default)]
    pub worktree: Option<Worktree>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Snapshot {
    #[serde(default)]
    pub agents: Vec<AgentInfo>,
    #[serde(default)]
    pub workspaces: Vec<WorkspaceInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SnapshotEnvelope {
    pub snapshot: Snapshot,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReadPayload {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReadEnvelope {
    pub read: ReadPayload,
}
