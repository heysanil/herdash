//! Grouping, ordering and status-age tracking.

use std::time::{Duration, Instant};

use herdash::fleet::{self, Timings};
use herdash::herdr::types::{AgentStatus, Snapshot};

const SNAPSHOT_FIXTURE: &str = include_str!("fixtures/snapshot.json");

/// A base instant safely in the future, so subtracting test durations can
/// never underflow `Instant` on a machine that booted moments ago.
fn base() -> Instant {
    Instant::now() + Duration::from_secs(3600)
}

fn fixture() -> Snapshot {
    let v: serde_json::Value = serde_json::from_str(SNAPSHOT_FIXTURE).unwrap();
    serde_json::from_value(v["result"]["snapshot"].clone()).unwrap()
}

fn built(active_only: bool) -> Vec<fleet::RepoGroup> {
    let snap = fixture();
    let mut t = Timings::new();
    t.observe(&snap, Instant::now());
    fleet::build(&snap, &t, active_only)
}

fn find<'a>(groups: &'a [fleet::RepoGroup], pane: &str) -> &'a fleet::Agent {
    groups
        .iter()
        .flat_map(|g| g.agents.iter())
        .find(|a| a.pane_id == pane)
        .unwrap()
}

#[test]
fn agents_are_grouped_by_repo_name() {
    let groups = built(false);
    let alpha = groups.iter().find(|g| g.name() == "alpha").unwrap();
    assert_eq!(alpha.agents.len(), 2, "both alpha worktrees group together");
    let beta = groups.iter().find(|g| g.name() == "beta").unwrap();
    assert_eq!(beta.agents.len(), 2);
}

#[test]
fn workspaces_without_a_worktree_fall_back_to_ungrouped() {
    let groups = built(false);
    let ungrouped = groups.iter().find(|g| g.repo.is_none()).unwrap();
    assert_eq!(ungrouped.name(), "ungrouped");
    assert_eq!(ungrouped.agents.len(), 1);
    assert_eq!(ungrouped.agents[0].pane_id, "w4:p1");
}

#[test]
fn the_workspace_label_becomes_the_display_name() {
    let groups = built(false);
    let alpha = groups.iter().find(|g| g.name() == "alpha").unwrap();
    let labels: Vec<&str> = alpha.agents.iter().map(|a| a.label.as_str()).collect();
    assert!(labels.contains(&"feat-alpha"));
    assert!(labels.contains(&"feat-alpha-docs"));
}

#[test]
fn agents_sort_by_urgency_within_a_group() {
    let groups = built(false);
    let beta = groups.iter().find(|g| g.name() == "beta").unwrap();
    assert_eq!(beta.agents[0].status, AgentStatus::Blocked);
    assert_eq!(beta.agents[1].status, AgentStatus::Unknown);
}

#[test]
fn groups_sort_by_their_most_urgent_member() {
    let groups = built(false);
    assert_eq!(
        groups[0].name(),
        "beta",
        "beta holds the only blocked agent"
    );
}

/// Regression guard for spec §8.2: `ungrouped` loses ties but must never
/// outrank urgency, or the one agent needing attention gets buried.
#[test]
fn ungrouped_sorts_last_only_among_equally_urgent_groups() {
    let groups = built(false);
    let ungrouped_pos = groups.iter().position(|g| g.repo.is_none()).unwrap();
    let alpha_pos = groups.iter().position(|g| g.name() == "alpha").unwrap();
    // ungrouped holds a `done` agent (urgency 1); alpha's best is `working` (2).
    assert!(
        ungrouped_pos < alpha_pos,
        "a done agent in ungrouped must outrank a working agent in a named repo"
    );
    assert_eq!(groups.last().unwrap().name(), "alpha");
}

#[test]
fn ungrouped_loses_a_tie_against_an_equally_urgent_named_repo() {
    // Give alpha a `done` agent too, so both groups tie at urgency 1.
    let mut snap = fixture();
    snap.agents
        .retain(|a| a.workspace_id == "w1" || a.workspace_id == "w4");
    snap.agents[0].agent_status = AgentStatus::from_wire("done");
    let mut t = Timings::new();
    t.observe(&snap, Instant::now());
    let groups = fleet::build(&snap, &t, false);
    assert_eq!(groups.len(), 2);
    assert_eq!(groups[0].name(), "alpha", "named repo wins the tie");
    assert_eq!(groups[1].name(), "ungrouped");
}

#[test]
fn active_only_hides_idle_and_unknown_agents() {
    let groups = built(true);
    let all: Vec<&fleet::Agent> = groups.iter().flat_map(|g| g.agents.iter()).collect();
    assert_eq!(all.len(), 3, "working + blocked + done survive");
    assert!(all.iter().all(|a| a.status != AgentStatus::Idle));
    assert!(all.iter().all(|a| a.status != AgentStatus::Unknown));
}

#[test]
fn empty_groups_are_dropped_when_filtering() {
    let groups = built(true);
    assert!(groups.iter().all(|g| !g.agents.is_empty()));
}

#[test]
fn an_empty_snapshot_yields_no_groups() {
    let snap = Snapshot::default();
    let t = Timings::new();
    assert!(fleet::build(&snap, &t, false).is_empty());
}

#[test]
fn timings_reset_status_age_when_state_change_seq_moves() {
    let mut snap = fixture();
    let mut t = Timings::new();
    let t0 = base() - Duration::from_secs(600);
    t.observe(&snap, t0);
    assert_eq!(
        find(&fleet::build(&snap, &t, false), "w1:p1").status_since,
        t0
    );

    // Same seq: age must not move.
    t.observe(&snap, t0 + Duration::from_secs(60));
    assert_eq!(
        find(&fleet::build(&snap, &t, false), "w1:p1").status_since,
        t0,
        "unchanged seq must not reset the clock"
    );

    // Bumped seq: age resets.
    snap.agents[0].state_change_seq = 101;
    let t2 = t0 + Duration::from_secs(120);
    t.observe(&snap, t2);
    assert_eq!(
        find(&fleet::build(&snap, &t, false), "w1:p1").status_since,
        t2
    );
}

#[test]
fn ages_are_lower_bounds_until_a_state_change_is_observed() {
    let mut snap = fixture();
    let mut t = Timings::new();
    let t0 = Instant::now();
    t.observe(&snap, t0);
    assert!(
        find(&fleet::build(&snap, &t, false), "w1:p1").age_is_lower_bound,
        "agent existed before we started watching"
    );

    snap.agents[0].state_change_seq = 101;
    t.observe(&snap, t0 + Duration::from_secs(5));
    assert!(
        !find(&fleet::build(&snap, &t, false), "w1:p1").age_is_lower_bound,
        "we have now seen a transition, so the age is exact"
    );
}

#[test]
fn timings_forget_agents_that_disappear() {
    let mut snap = fixture();
    let mut t = Timings::new();
    t.observe(&snap, Instant::now());
    assert_eq!(t.len(), 5);
    snap.agents.retain(|a| a.pane_id == "w1:p1");
    t.observe(&snap, Instant::now());
    assert_eq!(
        t.len(),
        1,
        "stale entries must not leak for the process lifetime"
    );
}

#[test]
fn counts_summarize_the_fleet() {
    let c = fleet::counts(&built(false));
    assert_eq!(c.total, 5);
    assert_eq!(c.working, 1);
    assert_eq!(c.idle, 1);
    assert_eq!(c.blocked, 1);
    assert_eq!(c.done, 1);
    assert_eq!(c.unknown, 1);
}
