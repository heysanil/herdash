//! Dashboard state and input handling.
//!
//! `App` owns everything the UI renders, including the status-age [`Timings`].
//! It performs no I/O: the event loop feeds it snapshots and summaries, and
//! [`App::on_key`] returns an [`Action`] for the loop to carry out.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::fleet::{self, Agent, RepoGroup, Timings};
use crate::herdr::types::{AgentStatus, Snapshot};
use crate::summary::AgentSummary;
use crate::summary::policy::SummaryState;

/// Per-agent summary bookkeeping plus the latest result or error.
#[derive(Debug, Clone, Default)]
pub struct SummarySlot {
    pub state: SummaryState,
    pub summary: Option<AgentSummary>,
    pub error: Option<String>,
}

/// Why summaries are or are not running.
///
/// A boolean would collapse two cases the user needs told apart: "you asked
/// for this" and "I could not find a key".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummariesMode {
    On,
    /// Running against a loopback endpoint — transcripts never leave the
    /// machine. Kept distinct from `On` so the header can say so.
    OnLocal,
    /// No API key resolved.
    OffNoKey,
    /// `--no-summaries` was passed.
    OffByFlag,
}

impl SummariesMode {
    pub fn enabled(self) -> bool {
        matches!(self, Self::On | Self::OnLocal)
    }

    /// Header annotation, or `None` when summaries are running.
    pub fn note(self) -> Option<&'static str> {
        match self {
            Self::On | Self::OnLocal => None,
            Self::OffNoKey => Some("summaries off (no key)"),
            Self::OffByFlag => Some("summaries off"),
        }
    }
}

/// Health of the herdr socket connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnState {
    Connected,
    Reconnecting { since: Instant },
}

/// One rendered sidebar line: a section header or an agent.
#[derive(Debug, Clone, Copy)]
pub enum Row<'a> {
    /// The pinned "waiting on you" section, carrying how many agents it holds.
    AttentionHeader(usize),
    /// A repository section, carrying how many of its agents are shown here
    /// (agents lifted into the attention section are not counted twice).
    Group(&'a RepoGroup, usize),
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
    pub summaries: SummariesMode,
    /// Provider name, or the variable a missing key was looked for in.
    /// Owned because it names a specific provider, unlike the `&'static str`
    /// notes above.
    pub summaries_detail: Option<String>,
    pub should_quit: bool,
    /// Redraw counter, used to animate the in-flight indicator. Incremented
    /// by the event loop so rendering stays a pure function of state.
    pub tick: u64,
    /// Transient one-line message shown in the header.
    pub notice: Option<String>,

    /// Status ages. Owned here so a filter toggle can rebuild immediately
    /// rather than waiting for the next poll.
    timings: Timings,
    /// Latest snapshot, retained for the same reason.
    last_snapshot: Option<Snapshot>,
}

impl App {
    pub fn new(summaries: SummariesMode) -> Self {
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
            summaries,
            summaries_detail: None,
            should_quit: false,
            tick: 0,
            notice: None,
            timings: Timings::new(),
            last_snapshot: None,
        }
    }

    /// Attach the provider detail rendered beside the summaries state.
    pub fn with_summaries_detail(mut self, detail: String) -> Self {
        self.summaries_detail = Some(detail);
        self
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

    pub fn summaries_enabled(&self) -> bool {
        self.summaries.enabled()
    }

    /// Every agent in the last snapshot, unfiltered.
    ///
    /// Summarization must never be scoped by a display filter: hiding idle
    /// agents is a viewing preference, not an instruction to stop describing
    /// them, and `R` has to reach agents the filter is hiding.
    pub fn all_agents(&self) -> Vec<Agent> {
        let Some(snapshot) = &self.last_snapshot else {
            return Vec::new();
        };
        fleet::build(snapshot, &self.timings, false)
            .into_iter()
            .flat_map(|group| group.agents)
            .collect()
    }

    /// Every agent in the last snapshot, including ones the filter hides.
    pub fn all_agent_ids(&self) -> Vec<String> {
        self.last_snapshot
            .as_ref()
            .map(|s| s.agents.iter().map(|a| a.pane_id.clone()).collect())
            .unwrap_or_default()
    }

    /// Number of tracked agents. Exposed so tests can assert no leak.
    pub fn timings_len(&self) -> usize {
        self.timings.len()
    }

    /// Recompute groups from the retained snapshot, honouring the filter.
    fn rebuild(&mut self) {
        // Capture the pre-rebuild order so a vanished selection can fall back
        // to a real neighbour rather than to whatever now sits at its index.
        let old_ids: Vec<String> = self.agents().iter().map(|a| a.pane_id.clone()).collect();
        let old_index = self
            .selected
            .as_ref()
            .and_then(|id| old_ids.iter().position(|old| old == id));

        let Some(snapshot) = self.last_snapshot.take() else {
            return;
        };
        self.groups = fleet::build(&snapshot, &self.timings, self.active_only);
        self.last_snapshot = Some(snapshot);
        self.reconcile_selection(&old_ids, old_index);
    }

    /// Keep the selection on the same agent; if it vanished, walk outward
    /// through the *previous* ordering for the closest survivor.
    ///
    /// Reusing the old numeric index would be wrong: agents can be inserted
    /// and reordered in the same poll, so index `n` may now hold an unrelated
    /// agent the user never selected.
    fn reconcile_selection(&mut self, old_ids: &[String], old_index: Option<usize>) {
        let visible: Vec<String> = self.agents().iter().map(|a| a.pane_id.clone()).collect();
        if visible.is_empty() {
            self.selected = None;
            return;
        }
        if self
            .selected
            .as_ref()
            .is_some_and(|id| visible.contains(id))
        {
            return;
        }
        if let Some(center) = old_index {
            for distance in 1..=old_ids.len() {
                for candidate in [center.checked_add(distance), center.checked_sub(distance)] {
                    if let Some(i) = candidate.filter(|&i| i < old_ids.len())
                        && visible.contains(&old_ids[i])
                    {
                        self.selected = Some(old_ids[i].clone());
                        return;
                    }
                }
            }
        }
        self.selected = Some(visible[0].clone());
    }

    fn selected_index(&self) -> Option<usize> {
        let sel = self.selected.as_deref()?;
        self.agents().iter().position(|a| a.pane_id == sel)
    }

    /// Every visible agent, in the order the sidebar renders them.
    ///
    /// Derived from [`Self::rows`] so selection can never disagree with what
    /// is on screen.
    pub fn agents(&self) -> Vec<&Agent> {
        self.rows()
            .into_iter()
            .filter_map(|r| match r {
                Row::Agent(a) => Some(a),
                _ => None,
            })
            .collect()
    }

    /// Whether this agent is waiting on the human for something.
    ///
    /// Judged from the model's reading of the transcript, **not** from herdr's
    /// lifecycle state: an agent can be `working` while sitting on a question
    /// it already asked, and `idle` simply because it finished cleanly and
    /// needs nothing. Before a summary exists we fall back to herdr's
    /// `blocked`, which is the best signal available until then.
    pub fn needs_attention(&self, agent: &Agent) -> bool {
        match self
            .slots
            .get(&agent.pane_id)
            .and_then(|s| s.summary.as_ref())
        {
            Some(summary) => summary.needs_attention,
            None => agent.status == AgentStatus::Blocked,
        }
    }

    /// What the agent is waiting for, when the model said so.
    pub fn attention_reason(&self, agent: &Agent) -> Option<&str> {
        let reason = self
            .slots
            .get(&agent.pane_id)
            .and_then(|s| s.summary.as_ref())
            .map(|s| s.attention_reason.as_str())
            .filter(|r| !r.is_empty())?;
        Some(reason)
    }

    /// Sidebar rows: the pinned attention section, then a header per repo.
    ///
    /// Agents needing attention are *lifted out* of their repo group rather
    /// than duplicated, so `j`/`k` never lands on the same agent twice.
    pub fn rows(&self) -> Vec<Row<'_>> {
        let mut rows = Vec::new();

        let waiting: Vec<&Agent> = self
            .groups
            .iter()
            .flat_map(|g| g.agents.iter())
            .filter(|a| self.needs_attention(a))
            .collect();
        if !waiting.is_empty() {
            rows.push(Row::AttentionHeader(waiting.len()));
            for a in waiting {
                rows.push(Row::Agent(a));
            }
        }

        for g in &self.groups {
            let remaining: Vec<&Agent> = g
                .agents
                .iter()
                .filter(|a| !self.needs_attention(a))
                .collect();
            if remaining.is_empty() {
                continue;
            }
            rows.push(Row::Group(g, remaining.len()));
            for a in remaining {
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

    /// Select a specific agent, e.g. from a mouse click. Returns whether the
    /// pane id was actually present.
    pub fn select(&mut self, pane_id: &str) -> bool {
        if self.agents().iter().any(|a| a.pane_id == pane_id) {
            self.selected = Some(pane_id.to_string());
            true
        } else {
            false
        }
    }

    /// Move the selection by `delta` rows, e.g. from a scroll wheel.
    pub fn scroll_selection(&mut self, delta: isize) {
        self.move_selection(delta);
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
        // Ctrl-C is checked first and unconditionally: an escape hatch that a
        // modal can swallow is not an escape hatch.
        if key.modifiers.contains(KeyModifiers::CONTROL) && matches!(key.code, KeyCode::Char('c')) {
            self.should_quit = true;
            return Action::Quit;
        }

        // The help overlay is otherwise modal: it swallows everything except
        // its own dismissal, so a stray `j` cannot move the hidden cursor.
        if self.show_help {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q')
            ) {
                self.show_help = false;
            }
            return Action::None;
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
                if self.all_agent_ids().is_empty() {
                    Action::None
                } else {
                    Action::ForceAll
                }
            }
            _ => Action::None,
        }
    }
}
