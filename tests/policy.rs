//! The summarization cadence decision table.
//!
//! This is the highest-value suite in the project: `policy::decide` is pure,
//! so every branch is reachable without a clock, a socket or HTTP.

use std::time::{Duration, Instant};

use herdash::fleet::Agent;
use herdash::herdr::types::AgentStatus;
use herdash::summary::policy::{Cfg, Decision, SummaryState, backoff, decide, observe_status};

/// A base instant safely in the future, so subtracting test durations can
/// never underflow `Instant` on a machine that booted moments ago.
fn base() -> Instant {
    Instant::now() + Duration::from_secs(3600)
}

fn cfg() -> Cfg {
    Cfg {
        cooldown: Duration::from_secs(45),
        enabled: true,
    }
}

fn agent(status: AgentStatus, revision: u64) -> Agent {
    let now = base();
    Agent {
        pane_id: "w1:p1".into(),
        workspace_id: "w1".into(),
        kind: "claude".into(),
        title: "t".into(),
        label: "feat".into(),
        repo: Some("alpha".into()),
        cwd: "/repos/alpha".into(),
        status,
        revision,
        state_change_seq: 1,
        status_since: now,
        age_is_lower_bound: false,
    }
}

/// A summarized state: revision 10 captured 60s ago, cooldown already elapsed.
fn settled(now: Instant, status: AgentStatus) -> SummaryState {
    SummaryState {
        in_flight: false,
        from_revision: Some(10),
        generated_at: Some(now - Duration::from_secs(60)),
        last_status: Some(status),
        pending_bypass: false,
        failures: 0,
        retry_after: None,
        forced: false,
    }
}

#[test]
fn a_never_summarized_agent_is_summarized_immediately() {
    let now = base();
    let st = SummaryState::default();
    assert_eq!(
        decide(&agent(AgentStatus::Working, 1), &st, now, &cfg()),
        Decision::Summarize
    );
}

#[test]
fn disabled_summaries_never_call_out() {
    let now = base();
    let cfg = Cfg {
        cooldown: Duration::from_secs(45),
        enabled: false,
    };
    assert_eq!(
        decide(
            &agent(AgentStatus::Working, 1),
            &SummaryState::default(),
            now,
            &cfg
        ),
        Decision::Skip
    );
}

#[test]
fn an_in_flight_call_blocks_a_second_one() {
    let now = base();
    let mut st = settled(now, AgentStatus::Working);
    st.in_flight = true;
    assert_eq!(
        decide(&agent(AgentStatus::Working, 99), &st, now, &cfg()),
        Decision::Skip
    );
}

#[test]
fn an_unchanged_revision_is_skipped_even_after_the_cooldown() {
    let now = base();
    let st = settled(now, AgentStatus::Working);
    assert_eq!(
        decide(&agent(AgentStatus::Working, 10), &st, now, &cfg()),
        Decision::Skip,
        "no new output means nothing new to say"
    );
}

#[test]
fn a_changed_revision_within_the_cooldown_is_skipped() {
    let now = base();
    let mut st = settled(now, AgentStatus::Working);
    st.generated_at = Some(now - Duration::from_secs(10));
    assert_eq!(
        decide(&agent(AgentStatus::Working, 11), &st, now, &cfg()),
        Decision::Skip
    );
}

#[test]
fn a_changed_revision_after_the_cooldown_is_summarized() {
    let now = base();
    let st = settled(now, AgentStatus::Working);
    assert_eq!(
        decide(&agent(AgentStatus::Working, 11), &st, now, &cfg()),
        Decision::Summarize
    );
}

#[test]
fn the_cooldown_boundary_is_inclusive() {
    let now = base();
    let mut st = settled(now, AgentStatus::Working);
    st.generated_at = Some(now - Duration::from_secs(45));
    assert_eq!(
        decide(&agent(AgentStatus::Working, 11), &st, now, &cfg()),
        Decision::Summarize
    );
}

#[test]
fn becoming_blocked_bypasses_the_cooldown() {
    let now = base();
    let mut st = settled(now, AgentStatus::Working);
    st.generated_at = Some(now - Duration::from_secs(1));
    observe_status(&mut st, AgentStatus::Blocked);
    assert!(st.pending_bypass);
    assert_eq!(
        decide(&agent(AgentStatus::Blocked, 11), &st, now, &cfg()),
        Decision::Summarize,
        "it is waiting on the user right now"
    );
}

#[test]
fn finishing_work_bypasses_the_cooldown() {
    let now = base();
    for finished in [AgentStatus::Done, AgentStatus::Idle] {
        let mut st = settled(now, AgentStatus::Working);
        st.generated_at = Some(now - Duration::from_secs(1));
        observe_status(&mut st, finished);
        assert_eq!(
            decide(&agent(finished, 11), &st, now, &cfg()),
            Decision::Summarize,
            "working -> {finished:?} just finished"
        );
    }
}

#[test]
fn staying_blocked_does_not_re_bypass_every_tick() {
    let now = base();
    let mut st = settled(now, AgentStatus::Blocked);
    st.generated_at = Some(now - Duration::from_secs(1));
    observe_status(&mut st, AgentStatus::Blocked);
    assert!(!st.pending_bypass, "no edge, so nothing to latch");
    assert_eq!(
        decide(&agent(AgentStatus::Blocked, 11), &st, now, &cfg()),
        Decision::Skip,
        "a bypass fires on the transition, not for as long as the state lasts"
    );
}

#[test]
fn going_idle_from_idle_is_not_a_completion_bypass() {
    let now = base();
    let mut st = settled(now, AgentStatus::Idle);
    st.generated_at = Some(now - Duration::from_secs(1));
    assert_eq!(
        decide(&agent(AgentStatus::Idle, 11), &st, now, &cfg()),
        Decision::Skip
    );
}

#[test]
fn a_bypass_still_requires_new_output() {
    let now = base();
    let mut st = settled(now, AgentStatus::Working);
    st.generated_at = Some(now - Duration::from_secs(1));
    observe_status(&mut st, AgentStatus::Blocked);
    assert_eq!(
        decide(&agent(AgentStatus::Blocked, 10), &st, now, &cfg()),
        Decision::Skip,
        "same revision means the transcript is unchanged, so there is nothing to re-read"
    );
}

#[test]
fn a_forced_refresh_ignores_cooldown_revision_and_backoff() {
    let now = base();
    let mut st = settled(now, AgentStatus::Working);
    st.generated_at = Some(now);
    st.forced = true;
    st.retry_after = Some(now + Duration::from_secs(300));
    assert_eq!(
        decide(&agent(AgentStatus::Working, 10), &st, now, &cfg()),
        Decision::Summarize,
        "the user asked explicitly"
    );
}

#[test]
fn a_forced_refresh_still_waits_for_an_in_flight_call() {
    let now = base();
    let mut st = settled(now, AgentStatus::Working);
    st.forced = true;
    st.in_flight = true;
    assert_eq!(
        decide(&agent(AgentStatus::Working, 10), &st, now, &cfg()),
        Decision::Skip
    );
}

#[test]
fn a_forced_refresh_is_still_refused_when_summaries_are_disabled() {
    let now = base();
    let cfg = Cfg {
        cooldown: Duration::from_secs(45),
        enabled: false,
    };
    let mut st = settled(now, AgentStatus::Working);
    st.forced = true;
    assert_eq!(
        decide(&agent(AgentStatus::Working, 11), &st, now, &cfg),
        Decision::Skip
    );
}

#[test]
fn a_pending_backoff_suppresses_retries() {
    let now = base();
    let mut st = settled(now, AgentStatus::Working);
    st.failures = 2;
    st.retry_after = Some(now + Duration::from_secs(10));
    assert_eq!(
        decide(&agent(AgentStatus::Working, 11), &st, now, &cfg()),
        Decision::Skip
    );
}

/// The revision is deliberately unchanged and the summary freshly generated,
/// so only the elapsed backoff can explain a retry. The earlier version of
/// this test passed even when backoff did nothing, because a 60-second-old
/// `generated_at` satisfied the cooldown on its own.
#[test]
fn an_elapsed_backoff_retries_even_with_an_unchanged_revision() {
    let now = base();
    let mut st = settled(now, AgentStatus::Working);
    st.generated_at = Some(now);
    st.failures = 2;
    st.retry_after = Some(now - Duration::from_secs(1));
    assert_eq!(
        decide(&agent(AgentStatus::Working, 10), &st, now, &cfg()),
        Decision::Summarize
    );
}

/// A failure records the revision it attempted, so without the failure branch
/// the revision gate would strand it permanently.
#[test]
fn a_failed_summary_is_never_stranded_by_the_revision_gate() {
    let now = base();
    let st = SummaryState {
        from_revision: Some(10),
        generated_at: None,
        failures: 1,
        retry_after: Some(now - Duration::from_millis(1)),
        last_status: Some(AgentStatus::Working),
        ..Default::default()
    };
    assert_eq!(
        decide(&agent(AgentStatus::Working, 10), &st, now, &cfg()),
        Decision::Summarize
    );
}

/// An urgent transition that lands while a call is in flight must survive it.
#[test]
fn a_bypass_latched_during_an_in_flight_call_survives_until_a_worker_is_free() {
    let now = base();
    let mut st = settled(now, AgentStatus::Working);
    st.generated_at = Some(now);
    st.in_flight = true;

    observe_status(&mut st, AgentStatus::Blocked);
    assert_eq!(
        decide(&agent(AgentStatus::Blocked, 11), &st, now, &cfg()),
        Decision::Skip,
        "still busy"
    );
    assert!(st.pending_bypass, "the edge must be latched, not consumed");

    st.in_flight = false;
    // A later observation of the same status must not clear the latch.
    observe_status(&mut st, AgentStatus::Blocked);
    assert_eq!(
        decide(&agent(AgentStatus::Blocked, 11), &st, now, &cfg()),
        Decision::Summarize,
        "the latched edge fires as soon as a worker frees up"
    );
}

#[test]
fn backoff_triples_from_five_seconds_and_caps_at_five_minutes() {
    assert_eq!(backoff(0), Duration::ZERO);
    assert_eq!(backoff(1), Duration::from_secs(5));
    assert_eq!(backoff(2), Duration::from_secs(15));
    assert_eq!(backoff(3), Duration::from_secs(45));
    assert_eq!(backoff(4), Duration::from_secs(135));
    assert_eq!(backoff(5), Duration::from_secs(300));
    assert_eq!(
        backoff(50),
        Duration::from_secs(300),
        "no overflow at high failure counts"
    );
    assert_eq!(backoff(u32::MAX), Duration::from_secs(300));
}
