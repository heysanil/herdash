//! Coordination logic: dispatch decisions, update folding, fleet throttling.
//!
//! None of this was testable while it lived in `main.rs`, and it is where the
//! subtlest bugs live — edges racing in-flight calls, forced refreshes,
//! failure retry, and a fleet summary that must not suppress its own retry.

use std::time::{Duration, Instant};

use herdash::app::{App, ConnState, SummariesMode};
use herdash::herdr::types::{AgentStatus, Snapshot};
use herdash::orchestrator::{
    self, FLEET_COOLDOWN, FleetJob, Update, apply_update, force_all, force_one, plan_fleet,
    plan_summaries,
};
use herdash::summary::AgentSummary;
use herdash::summary::policy::Cfg;

const SNAPSHOT_FIXTURE: &str = include_str!("fixtures/snapshot.json");

/// A base instant safely in the future, so tests can subtract without risking
/// an `Instant` underflow panic on a freshly booted machine.
fn base() -> Instant {
    Instant::now() + Duration::from_secs(3600)
}

fn fixture() -> Snapshot {
    let v: serde_json::Value = serde_json::from_str(SNAPSHOT_FIXTURE).unwrap();
    serde_json::from_value(v["result"]["snapshot"].clone()).unwrap()
}

fn app() -> App {
    let mut a = App::new(SummariesMode::On);
    a.apply_snapshot(&fixture());
    a
}

fn cfg() -> Cfg {
    Cfg {
        cooldown: Duration::from_secs(45),
        enabled: true,
    }
}

fn summary(headline: &str) -> AgentSummary {
    AgentSummary {
        headline: headline.into(),
        task: "task".into(),
        now: "now".into(),
        recent: vec![],
    }
}

// ------------------------------------------------------------- dispatching --

#[test]
fn every_unsummarized_agent_is_dispatched_on_the_first_pass() {
    let mut a = app();
    let jobs = plan_summaries(&mut a, base(), &cfg());
    assert_eq!(jobs.len(), 5);
    assert!(a.slots.values().all(|s| s.state.in_flight));
}

#[test]
fn a_second_pass_dispatches_nothing_while_calls_are_in_flight() {
    let mut a = app();
    plan_summaries(&mut a, base(), &cfg());
    assert!(plan_summaries(&mut a, base(), &cfg()).is_empty());
}

#[test]
fn disabled_summaries_dispatch_nothing() {
    let mut a = App::new(SummariesMode::OffByFlag);
    a.apply_snapshot(&fixture());
    let cfg = Cfg {
        cooldown: Duration::from_secs(45),
        enabled: false,
    };
    assert!(plan_summaries(&mut a, base(), &cfg).is_empty());
}

#[test]
fn a_job_carries_the_snapshot_revision_as_a_fallback() {
    let mut a = app();
    let jobs = plan_summaries(&mut a, base(), &cfg());
    let job = jobs.iter().find(|j| j.pane_id == "w1:p1").unwrap();
    assert_eq!(job.revision, 10);
}

// ------------------------------------------------------------- forced work --

/// Pressing `r` while a call is already running must not be swallowed when
/// that call completes.
#[test]
fn a_forced_refresh_pressed_during_an_in_flight_call_survives_completion() {
    let mut a = app();
    let mut fleet = FleetJob::default();
    plan_summaries(&mut a, base(), &cfg());

    force_one(&mut a, "w1:p1");
    apply_update(
        &mut a,
        &mut fleet,
        Update::Summary {
            pane_id: "w1:p1".into(),
            revision: 10,
            result: Ok(summary("done")),
        },
    );

    assert!(
        a.slots["w1:p1"].state.forced,
        "the keypress must outlive the call it raced"
    );
    let jobs = plan_summaries(&mut a, base(), &cfg());
    assert!(
        jobs.iter().any(|j| j.pane_id == "w1:p1"),
        "and must actually dispatch"
    );
}

#[test]
fn dispatching_consumes_the_forced_flag_exactly_once() {
    let mut a = app();
    plan_summaries(&mut a, base(), &cfg());
    // Clear in-flight so the next pass can dispatch.
    for slot in a.slots.values_mut() {
        slot.state.in_flight = false;
        slot.state.from_revision = Some(999);
        slot.state.generated_at = Some(base());
    }
    force_one(&mut a, "w1:p1");
    assert_eq!(plan_summaries(&mut a, base(), &cfg()).len(), 1);
    for slot in a.slots.values_mut() {
        slot.state.in_flight = false;
    }
    assert!(
        plan_summaries(&mut a, base(), &cfg()).is_empty(),
        "not forced twice"
    );
}

/// `R` must reach agents the active-only filter is hiding.
#[test]
fn force_all_covers_agents_the_filter_hides() {
    let mut a = app();
    a.active_only = true;
    a.apply_snapshot(&fixture());
    assert_eq!(a.agents().len(), 3, "two agents are filtered out");

    force_all(&mut a);
    assert_eq!(a.slots.len(), 5, "including the hidden ones");
    assert!(a.slots.values().all(|s| s.state.forced));
}

// ------------------------------------------------------- failure behaviour --

#[test]
fn a_failed_summary_records_backoff_and_keeps_the_previous_text_visible() {
    let mut a = app();
    let mut fleet = FleetJob::default();
    apply_update(
        &mut a,
        &mut fleet,
        Update::Summary {
            pane_id: "w1:p1".into(),
            revision: 10,
            result: Ok(summary("first")),
        },
    );
    apply_update(
        &mut a,
        &mut fleet,
        Update::Summary {
            pane_id: "w1:p1".into(),
            revision: 11,
            result: Err("429 rate limited\nsecond line".into()),
        },
    );

    let slot = &a.slots["w1:p1"];
    assert_eq!(slot.state.failures, 1);
    assert!(slot.state.retry_after.is_some());
    assert_eq!(
        slot.error.as_deref(),
        Some("429 rate limited"),
        "only the first line"
    );
    assert_eq!(
        slot.summary.as_ref().unwrap().headline,
        "first",
        "a failure annotates the old summary rather than erasing it"
    );
    assert!(!slot.state.in_flight);
}

#[test]
fn a_success_clears_the_failure_state() {
    let mut a = app();
    let mut fleet = FleetJob::default();
    apply_update(
        &mut a,
        &mut fleet,
        Update::Summary {
            pane_id: "w1:p1".into(),
            revision: 10,
            result: Err("boom".into()),
        },
    );
    apply_update(
        &mut a,
        &mut fleet,
        Update::Summary {
            pane_id: "w1:p1".into(),
            revision: 11,
            result: Ok(summary("ok")),
        },
    );
    let slot = &a.slots["w1:p1"];
    assert_eq!(slot.state.failures, 0);
    assert!(slot.state.retry_after.is_none());
    assert!(slot.error.is_none());
    assert_eq!(slot.state.from_revision, Some(11));
}

// ----------------------------------------------------------------- updates --

#[test]
fn a_snapshot_marks_the_connection_healthy_and_clears_any_notice() {
    let mut a = app();
    let mut fleet = FleetJob::default();
    a.conn = ConnState::Reconnecting { since: base() };
    a.notice = Some("herdr: something".into());
    apply_update(&mut a, &mut fleet, Update::Snapshot(Box::new(fixture())));
    assert_eq!(a.conn, ConnState::Connected);
    assert!(a.notice.is_none());
}

#[test]
fn a_notice_does_not_flip_the_connection_state() {
    let mut a = app();
    let mut fleet = FleetJob::default();
    apply_update(
        &mut a,
        &mut fleet,
        Update::Notice("herdr: odd payload".into()),
    );
    assert_eq!(
        a.conn,
        ConnState::Connected,
        "herdr answered, so it is not down"
    );
    assert!(a.notice.is_some());
}

// ------------------------------------------------------------------- fleet --

fn app_with_two_summaries() -> App {
    let mut a = app();
    let mut fleet = FleetJob::default();
    for (pane, head) in [("w1:p1", "one"), ("w3:p1", "two")] {
        apply_update(
            &mut a,
            &mut fleet,
            Update::Summary {
                pane_id: pane.into(),
                revision: 1,
                result: Ok(summary(head)),
            },
        );
    }
    a
}

#[test]
fn the_fleet_summary_needs_at_least_two_agent_summaries() {
    let mut a = app();
    let mut fleet = FleetJob::default();
    let mut job = FleetJob::default();
    apply_update(
        &mut a,
        &mut fleet,
        Update::Summary {
            pane_id: "w1:p1".into(),
            revision: 1,
            result: Ok(summary("only")),
        },
    );
    assert!(plan_fleet(&a, &mut job, base(), FLEET_COOLDOWN).is_none());
}

#[test]
fn the_fleet_summary_is_requested_once_per_change() {
    let a = app_with_two_summaries();
    let mut job = FleetJob::default();
    let req = plan_fleet(&a, &mut job, base(), FLEET_COOLDOWN).expect("first request");
    assert_eq!(req.headlines.len(), 2);
    assert!(job.in_flight);
    assert!(
        plan_fleet(&a, &mut job, base(), FLEET_COOLDOWN).is_none(),
        "no second call while one is in flight"
    );
}

/// Committing the hash before the call would suppress every future attempt.
#[test]
fn a_failed_fleet_summary_is_retried_once_the_cooldown_elapses() {
    let mut a = app_with_two_summaries();
    let mut job = FleetJob::default();
    let req = plan_fleet(&a, &mut job, base(), FLEET_COOLDOWN).unwrap();

    let mut discard = FleetJob::default();
    let _ = &mut discard;
    apply_update(
        &mut a,
        &mut job,
        Update::Fleet {
            hash: req.hash,
            result: Err("503".into()),
        },
    );
    assert!(!job.in_flight);
    assert!(
        job.last_success_hash.is_none(),
        "a failure must not commit the hash"
    );

    let later = base() + FLEET_COOLDOWN + Duration::from_secs(1);
    assert!(
        plan_fleet(&a, &mut job, later, FLEET_COOLDOWN).is_some(),
        "the same unchanged input must be retried after a failure"
    );
}

#[test]
fn an_unchanged_fleet_is_not_re_summarised_after_success() {
    let mut a = app_with_two_summaries();
    let mut job = FleetJob::default();
    let req = plan_fleet(&a, &mut job, base(), FLEET_COOLDOWN).unwrap();
    apply_update(
        &mut a,
        &mut job,
        Update::Fleet {
            hash: req.hash,
            result: Ok("overview".into()),
        },
    );
    assert_eq!(a.fleet_summary.as_deref(), Some("overview"));
    assert!(a.fleet_generated_at.is_some());

    let later = base() + FLEET_COOLDOWN * 10;
    assert!(
        plan_fleet(&a, &mut job, later, FLEET_COOLDOWN).is_none(),
        "nothing changed, so there is nothing new to say"
    );
}

#[test]
fn a_changed_fleet_is_re_summarised_after_the_cooldown() {
    let mut a = app_with_two_summaries();
    let mut job = FleetJob::default();
    let req = plan_fleet(&a, &mut job, base(), FLEET_COOLDOWN).unwrap();
    apply_update(
        &mut a,
        &mut job,
        Update::Fleet {
            hash: req.hash,
            result: Ok("v1".into()),
        },
    );

    // A new headline changes the hash.
    let mut throwaway = FleetJob::default();
    apply_update(
        &mut a,
        &mut throwaway,
        Update::Summary {
            pane_id: "w1:p1".into(),
            revision: 2,
            result: Ok(summary("changed")),
        },
    );

    let later = base() + FLEET_COOLDOWN + Duration::from_secs(1);
    assert!(plan_fleet(&a, &mut job, later, FLEET_COOLDOWN).is_some());
}

#[test]
fn a_changed_fleet_still_respects_the_cooldown() {
    let mut a = app_with_two_summaries();
    let mut job = FleetJob::default();
    let req = plan_fleet(&a, &mut job, base(), FLEET_COOLDOWN).unwrap();
    apply_update(
        &mut a,
        &mut job,
        Update::Fleet {
            hash: req.hash,
            result: Ok("v1".into()),
        },
    );

    let mut throwaway = FleetJob::default();
    apply_update(
        &mut a,
        &mut throwaway,
        Update::Summary {
            pane_id: "w1:p1".into(),
            revision: 2,
            result: Ok(summary("changed")),
        },
    );

    let soon = base() + Duration::from_secs(5);
    assert!(plan_fleet(&a, &mut job, soon, FLEET_COOLDOWN).is_none());
}

// ------------------------------------------------------ latched transitions --

/// The whole point of latching: an agent that becomes blocked while its own
/// summary call is running must still be re-summarised afterwards.
#[test]
fn an_agent_that_blocks_during_its_own_call_is_resummarised_afterwards() {
    let mut a = app();
    let mut fleet = FleetJob::default();
    let now = base();

    plan_summaries(&mut a, now, &cfg());
    apply_update(
        &mut a,
        &mut fleet,
        Update::Summary {
            pane_id: "w1:p1".into(),
            revision: 10,
            result: Ok(summary("working")),
        },
    );

    // While that call was in flight the agent became blocked and produced output.
    let mut snap = fixture();
    for agent in &mut snap.agents {
        if agent.pane_id == "w1:p1" {
            agent.agent_status = AgentStatus::from_wire("blocked");
            agent.revision = 11;
            agent.state_change_seq += 1;
        }
    }
    a.apply_snapshot(&snap);

    // Well inside the cooldown, so only a latched bypass can explain dispatch.
    let jobs = plan_summaries(&mut a, now + Duration::from_secs(1), &cfg());
    assert!(
        jobs.iter().any(|j| j.pane_id == "w1:p1"),
        "the blocked transition must survive the call it raced"
    );
}

#[test]
fn plan_summaries_observes_status_for_agents_it_skips() {
    let mut a = app();
    let now = base();
    plan_summaries(&mut a, now, &cfg());
    assert_eq!(
        a.slots["w1:p1"].state.last_status,
        Some(AgentStatus::Working),
        "status is recorded even for agents that were dispatched or skipped"
    );
}

#[test]
fn orchestrator_module_is_reachable() {
    // Guards against the module being made private again by accident.
    let _ = orchestrator::FLEET_COOLDOWN;
}
