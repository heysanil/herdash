//! Dashboard state and input handling.
//!
//! `App` owns everything the UI renders, including the status-age [`Timings`].
//! It performs no I/O: the event loop feeds it snapshots and summaries, and
//! [`App::on_key`] returns an [`Action`] for the loop to carry out.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::fleet::{self, Agent, RepoGroup, Timings};
use crate::herdr::types::Snapshot;
use crate::summary::AgentSummary;
use crate::summary::policy::SummaryState;

/// Per-agent summary bookkeeping plus the latest result or error.
#[derive(Debug, Clone, Default)]
pub struct SummarySlot {
    pub state: SummaryState,
    pub summary: Option<AgentSummary>,
    pub error: Option<String>,
}

/// Health of the herdr socket connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connected,
    Reconnecting { since: Instant },
}

/// One rendered sidebar line group: either a repo header or an agent.
#[derive(Debug, Clone, Copy)]
pub enum Row<'a> {
    Group(&'a RepoGroup),
    Agent(&'a Agent),
}

/// Work the event loop should perform on the app's behalf.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    None,
    Quit,
    /// Focus herdr on this pane id.
    Focus(String),
    /// Force a re-summary of this pane id.
    ForceOne(String),
    /// Force a re-summary of every visible agent.
    ForceAll,
}

pub struct App {
    pub groups: Vec<RepoGroup>,
    pub slots: HashMap<String, SummarySlot>,
    pub fleet_summary: Option<String>,
    pub fleet_generated_at: Option<Instant>,
    /// Selected agent, keyed by pane id so it survives re-sorting.
    pub selected: Option<String>,
    pub active_only: bool,
    pub show_help: bool,
    /// Narrow-terminal mode: detail replaces the sidebar.
    pub detail_open: bool,
    pub conn: ConnState,
    pub summaries_enabled: bool,
    pub should_quit: bool,
    /// Transient one-line message shown in the header.
    pub notice: Option<String>,

    /// Status ages. Owned here so a filter toggle can rebuild immediately
    /// rather than waiting for the next poll.
    timings: Timings,
    /// Latest snapshot, retained for the same reason.
    last_snapshot: Option<Snapshot>,
}

impl App {
    pub fn new(summaries_enabled: bool) -> Self {
        Self {
            groups: Vec::new(),
            slots: HashMap::new(),
            fleet_summary: None,
            fleet_generated_at: None,
            selected: None,
            active_only: false,
            show_help: false,
            detail_open: false,
            conn: ConnState::Connected,
            summaries_enabled,
            should_quit: false,
            notice: None,
            timings: Timings::new(),
            last_snapshot: None,
        }
    }

    /// Fold in a fresh snapshot: advance ages, rebuild groups, prune dead
    /// agents, and keep the selection pointing at something visible.
    pub fn apply_snapshot(&mut self, snapshot: &Snapshot) {
        self.timings.observe(snapshot, Instant::now());
        self.last_snapshot = Some(snapshot.clone());
        self.rebuild();

        let live: HashSet<&str> = snapshot.agents.iter().map(|a| a.pane_id.as_str()).collect();
        self.slots.retain(|k, _| live.contains(k.as_str()));
    }

    /// Number of tracked agents. Exposed so tests can assert no leak.
    pub fn timings_len(&self) -> usize {
        self.timings.len()
    }

    /// Recompute groups from the retained snapshot, honouring the filter.
    fn rebuild(&mut self) {
        let Some(snapshot) = self.last_snapshot.take() else { return };
        let previous_index = self.selected_index();
        self.groups = fleet::build(&snapshot, &self.timings, self.active_only);
        self.last_snapshot = Some(snapshot);
        self.reconcile_selection(previous_index);
    }

    /// Keep the selection on the same agent; if it vanished, fall back to the
    /// nearest surviving row so the cursor does not jump to the top.
    fn reconcile_selection(&mut self, previous_index: Option<usize>) {
        let ids: Vec<String> = self.agents().iter().map(|a| a.pane_id.clone()).collect();
        if ids.is_empty() {
            self.selected = None;
            return;
        }
        if let Some(sel) = &self.selected
            && ids.iter().any(|id| id == sel)
        {
            return;
        }
        let idx = previous_index.unwrap_or(0).min(ids.len() - 1);
        self.selected = Some(ids[idx].clone());
    }

    fn selected_index(&self) -> Option<usize> {
        let sel = self.selected.as_deref()?;
        self.agents().iter().position(|a| a.pane_id == sel)
    }

    /// Every visible agent in render order.
    pub fn agents(&self) -> Vec<&Agent> {
        self.groups.iter().flat_map(|g| g.agents.iter()).collect()
    }

    /// Sidebar rows: a header per group, then its agents.
    pub fn rows(&self) -> Vec<Row<'_>> {
        let mut rows = Vec::new();
        for g in &self.groups {
            rows.push(Row::Group(g));
            for a in &g.agents {
                rows.push(Row::Agent(a));
            }
        }
        rows
    }

    pub fn selected_agent(&self) -> Option<&Agent> {
        let sel = self.selected.as_deref()?;
        self.agents().into_iter().find(|a| a.pane_id == sel)
    }

    pub fn counts(&self) -> fleet::Counts {
        fleet::counts(&self.groups)
    }

    fn move_selection(&mut self, delta: isize) {
        let ids: Vec<String> = self.agents().iter().map(|a| a.pane_id.clone()).collect();
        if ids.is_empty() {
            return;
        }
        let cur = self.selected_index().unwrap_or(0) as isize;
        let next = cur.saturating_add(delta).clamp(0, ids.len() as isize - 1) as usize;
        self.selected = Some(ids[next].clone());
    }

    /// Apply a keypress. Returns work for the event loop; mutations that are
    /// purely local (selection, filters, overlays) happen here.
    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        // The help overlay is modal: it swallows everything except its own
        // dismissal, so a stray `j` cannot silently move the hidden cursor.
        if self.show_help {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')) {
                self.show_help = false;
            }
            return Action::None;
        }

        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c'))
        {
            self.should_quit = true;
            return Action::Quit;
        }

        match key.code {
            KeyCode::Char('q') => {
                self.should_quit = true;
                Action::Quit
            }
            KeyCode::Char('?') => {
                self.show_help = true;
                Action::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.move_selection(1);
                Action::None
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.move_selection(-1);
                Action::None
            }
            KeyCode::Char('g') | KeyCode::Home => {
                self.move_selection(isize::MIN / 2);
                Action::None
            }
            KeyCode::Char('G') | KeyCode::End => {
                self.move_selection(isize::MAX / 2);
                Action::None
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.detail_open = true;
                Action::None
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Esc => {
                self.detail_open = false;
                Action::None
            }
            KeyCode::Char('a') => {
                self.active_only = !self.active_only;
                // Rebuild now so the filter is visible on the very next frame.
                self.rebuild();
                Action::None
            }
            KeyCode::Enter => match self.selected.clone() {
                Some(id) => Action::Focus(id),
                None => Action::None,
            },
            KeyCode::Char('r') => match self.selected.clone() {
                Some(id) => Action::ForceOne(id),
                None => Action::None,
            },
            KeyCode::Char('R') => {
                if self.agents().is_empty() {
                    Action::None
                } else {
                    Action::ForceAll
                }
            }
            _ => Action::None,
        }
    }
}
