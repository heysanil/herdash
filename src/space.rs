//! Naming herdash's space in herdr's sidebar.
//!
//! # Why a rename
//!
//! With no user configuration, herdr renders a space row as
//! `state_icon, workspace, branch, git_status`. Of those, `branch` and
//! `git_status` come from git and the icons come from agent state, so the
//! **label is the only part a program can influence without the user editing
//! `ui.sidebar.spaces.rows`**. Publishing a `$herdash` metadata token is the
//! tidier mechanism, but it renders only for users who have added the token to
//! their template — so it cannot be the default.
//!
//! # Why it is safe anyway
//!
//! `workspace.rename` has no "reset to derived" — an empty label sets an empty
//! label, verified against herdr 0.8.2. Restoring therefore means remembering
//! the original string. A clean exit restores it directly; a crash is covered
//! by a small state file that the next run reads.
//!
//! The restore is conditional: it only puts the old label back if the space is
//! *still* named what herdash set it to. If the user renamed the space
//! themselves in the meantime, their choice wins.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// What herdash renamed, and what it was called before.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claims {
    /// workspace id -> `(label herdash set, label it had before)`
    #[serde(default)]
    pub spaces: HashMap<String, Claim>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Claim {
    /// The label herdash applied, used to detect a later user rename.
    pub applied: String,
    /// The label to put back.
    pub original: String,
}

impl Claims {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    /// Persist, creating the parent directory. Failure is not fatal: losing
    /// crash recovery is better than refusing to start.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::to_string_pretty(self)?)
    }

    pub fn claim(&mut self, workspace_id: &str, applied: &str, original: &str) {
        self.spaces.insert(
            workspace_id.to_string(),
            Claim {
                applied: applied.to_string(),
                original: original.to_string(),
            },
        );
    }

    pub fn release(&mut self, workspace_id: &str) -> Option<Claim> {
        self.spaces.remove(workspace_id)
    }
}

/// Where crash-recovery state lives, honoring `XDG_STATE_HOME`.
pub fn state_path(home: &Path) -> PathBuf {
    std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local").join("state"))
        .join("herdash")
        .join("spaces.json")
}

/// Decide what to restore, given a claim and the space's current label.
///
/// Returns `None` when the user has since renamed the space themselves — their
/// rename outranks our bookkeeping.
pub fn restore_target(claim: &Claim, current_label: &str) -> Option<String> {
    if current_label == claim.applied {
        Some(claim.original.clone())
    } else {
        None
    }
}
