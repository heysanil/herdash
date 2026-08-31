//! Wire types for the herdr socket API (protocol 20).
//!
//! Every struct uses `#[serde(default)]` liberally and never denies unknown
//! fields, so a herdr upgrade that adds fields cannot break the dashboard.

use serde::{Deserialize, Deserializer};

/// Accept an explicit JSON `null` as the type's default.
///
/// This is not the same as `#[serde(default)]`, which only covers an *absent*
/// key. herdr's schema declares `agent`, `cwd`, `terminal_title_stripped`,
/// `terminal_title`, `label` and `foreground_cwd` as `["string", "null"]`, so
/// a real server can send `null` for any of them — and `null` into `String`
/// is a hard deserialisation error that would blank the whole dashboard.
fn null_to_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

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
    /// Agent kind, e.g. "claude", "codex". Nullable on the wire.
    #[serde(default, deserialize_with = "null_to_default")]
    pub agent: String,
    pub agent_status: AgentStatus,
    pub workspace_id: String,
    pub pane_id: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub tab_id: String,
    /// Nullable on the wire.
    #[serde(default, deserialize_with = "null_to_default")]
    pub terminal_title_stripped: String,
    /// Nullable on the wire.
    #[serde(default, deserialize_with = "null_to_default")]
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
    #[serde(default, deserialize_with = "null_to_default")]
    pub repo_name: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub repo_root: String,
    #[serde(default, deserialize_with = "null_to_default")]
    pub checkout_path: String,
    #[serde(default)]
    pub is_linked_worktree: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct WorkspaceInfo {
    pub workspace_id: String,
    /// Nullable on the wire.
    #[serde(default, deserialize_with = "null_to_default")]
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
    #[serde(default, deserialize_with = "null_to_default")]
    pub text: String,
    /// Which read source herdr actually served, e.g. `recent_unwrapped` or
    /// `visible`. Present so callers can tell whether the fallback fired.
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub revision: u64,
    #[serde(default)]
    pub truncated: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReadEnvelope {
    pub read: ReadPayload,
}
