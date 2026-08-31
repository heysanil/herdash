//! Selection, filtering and key handling.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use herdash::app::{Action, App, Row, SummariesMode};
use herdash::herdr::types::Snapshot;

const SNAPSHOT_FIXTURE: &str = include_str!("fixtures/snapshot.json");

fn fixture() -> Snapshot {
    let v: serde_json::Value = serde_json::from_str(SNAPSHOT_FIXTURE).unwrap();
    serde_json::from_value(v["result"]["snapshot"].clone()).unwrap()
}

fn app_with_fixture() -> App {
    let mut app = App::new(SummariesMode::On);
    app.apply_snapshot(&fixture());
    app
}

fn key(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE)
}

#[test]
fn rows_interleave_group_headers_with_their_agents() {
    let app = app_with_fixture();
    let rows = app.rows();
    // The fixture holds a blocked agent, so the attention section leads.
    assert!(matches!(rows[0], Row::AttentionHeader(1)));
    assert!(
        rows.iter().any(|r| matches!(r, Row::Group(_, _))),
        "repo headers still appear"
    );
    assert_eq!(
        rows.iter().filter(|r| matches!(r, Row::Agent(_))).count(),
        5,
        "every agent appears exactly once"
    );
}

#[test]
fn the_first_agent_is_selected_on_first_snapshot() {
    // beta leads (it holds the blocked agent), and blocked sorts first inside it.
    assert_eq!(app_with_fixture().selected.as_deref(), Some("w3:p1"));
}

#[test]
fn moving_down_skips_group_headers() {
    let mut app = app_with_fixture();
    let order: Vec<String> = app.agents().iter().map(|a| a.pane_id.clone()).collect();
    app.on_key(key('j'));
    assert_eq!(app.selected.as_deref(), Some(order[1].as_str()));
    app.on_key(key('j'));
    assert_eq!(app.selected.as_deref(), Some(order[2].as_str()));
}

#[test]
fn selection_clamps_at_both_ends() {
    let mut app = app_with_fixture();
    let order: Vec<String> = app.agents().iter().map(|a| a.pane_id.clone()).collect();
    app.on_key(key('k'));
    assert_eq!(
        app.selected.as_deref(),
        Some(order[0].as_str()),
        "already at the top"
    );
    app.on_key(key('G'));
    assert_eq!(
        app.selected.as_deref(),
        Some(order.last().unwrap().as_str())
    );
    app.on_key(key('j'));
    assert_eq!(
        app.selected.as_deref(),
        Some(order.last().unwrap().as_str()),
        "already at the bottom"
    );
    app.on_key(key('g'));
    assert_eq!(app.selected.as_deref(), Some(order[0].as_str()));
}

#[test]
fn selection_survives_reordering_because_it_is_keyed_on_pane_id() {
    let mut app = app_with_fixture();
    app.on_key(key('j'));
    let chosen = app.selected.clone().unwrap();

    let mut snap = fixture();
    for a in &mut snap.agents {
        a.state_change_seq += 1;
    }
    snap.agents.reverse();
    app.apply_snapshot(&snap);

    assert_eq!(app.selected, Some(chosen), "the same agent stays selected");
}

#[test]
fn selection_moves_to_a_neighbour_when_the_selected_agent_disappears() {
    let mut app = app_with_fixture();
    let selected = app.selected.clone().unwrap();

    let mut snap = fixture();
    snap.agents.retain(|a| a.pane_id != selected);
    app.apply_snapshot(&snap);

    let survivors: Vec<String> = app.agents().iter().map(|a| a.pane_id.clone()).collect();
    assert!(app.selected.is_some());
    assert!(survivors.contains(app.selected.as_ref().unwrap()));
}

#[test]
fn selection_becomes_none_when_every_agent_disappears() {
    let mut app = app_with_fixture();
    let mut snap = fixture();
    snap.agents.clear();
    app.apply_snapshot(&snap);
    assert!(app.selected.is_none());
    assert!(app.rows().is_empty());
    assert!(app.selected_agent().is_none());
}

#[test]
fn stale_summary_slots_are_pruned_when_agents_disappear() {
    let mut app = app_with_fixture();
    app.slots.entry("w1:p1".into()).or_default();
    app.slots.entry("w3:p1".into()).or_default();

    let mut snap = fixture();
    snap.agents.retain(|a| a.pane_id == "w1:p1");
    app.apply_snapshot(&snap);

    assert!(app.slots.contains_key("w1:p1"));
    assert!(
        !app.slots.contains_key("w3:p1"),
        "no unbounded growth over a long session"
    );
}

#[test]
fn timings_are_pruned_with_the_agents_that_owned_them() {
    let mut app = app_with_fixture();
    assert_eq!(app.timings_len(), 5);
    let mut snap = fixture();
    snap.agents.retain(|a| a.pane_id == "w1:p1");
    app.apply_snapshot(&snap);
    assert_eq!(app.timings_len(), 1);
}

#[test]
fn a_toggles_the_active_only_filter_and_takes_effect_immediately() {
    let mut app = app_with_fixture();
    assert_eq!(app.agents().len(), 5);
    app.on_key(key('a'));
    assert!(app.active_only);
    assert_eq!(
        app.agents().len(),
        3,
        "the filter applies without waiting for a poll"
    );
    app.on_key(key('a'));
    assert_eq!(app.agents().len(), 5);
}

#[test]
fn filtering_out_the_selected_agent_reselects_a_visible_one() {
    let mut app = app_with_fixture();
    // Select the idle agent, which the filter will hide.
    while app.selected_agent().map(|a| a.pane_id.as_str()) != Some("w2:p1") {
        app.on_key(key('j'));
    }
    app.on_key(key('a'));
    let visible: Vec<String> = app.agents().iter().map(|a| a.pane_id.clone()).collect();
    assert!(visible.contains(app.selected.as_ref().unwrap()));
}

#[test]
fn counts_reflect_the_active_filter() {
    let mut app = app_with_fixture();
    assert_eq!(app.counts().total, 5);
    app.on_key(key('a'));
    assert_eq!(app.counts().total, 3);
    assert_eq!(app.counts().idle, 0);
}

#[test]
fn enter_asks_to_focus_the_selected_pane() {
    let mut app = app_with_fixture();
    let want = app.selected.clone().unwrap();
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Action::Focus(want)
    );
}

#[test]
fn r_forces_one_and_shift_r_forces_all() {
    let mut app = app_with_fixture();
    let want = app.selected.clone().unwrap();
    assert_eq!(app.on_key(key('r')), Action::ForceOne(want));
    assert_eq!(app.on_key(key('R')), Action::ForceAll);
}

#[test]
fn q_and_ctrl_c_quit() {
    let mut app = app_with_fixture();
    assert_eq!(app.on_key(key('q')), Action::Quit);
    assert!(app.should_quit);

    let mut app = app_with_fixture();
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        Action::Quit
    );
    assert!(app.should_quit);
}

#[test]
fn question_mark_toggles_help_and_escape_closes_it() {
    let mut app = app_with_fixture();
    app.on_key(key('?'));
    assert!(app.show_help);
    app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app.show_help);
}

#[test]
fn help_swallows_navigation_keys_while_open() {
    let mut app = app_with_fixture();
    let before = app.selected.clone();
    app.on_key(key('?'));
    app.on_key(key('j'));
    assert_eq!(
        app.selected, before,
        "navigation must not happen behind the overlay"
    );
    assert!(app.show_help, "and the overlay stays open");
}

#[test]
fn quitting_from_the_help_overlay_only_closes_it() {
    let mut app = app_with_fixture();
    app.on_key(key('?'));
    assert_eq!(app.on_key(key('q')), Action::None);
    assert!(!app.show_help);
    assert!(
        !app.should_quit,
        "the first q dismisses help rather than exiting"
    );
}

#[test]
fn arrows_open_and_close_detail_for_narrow_terminals() {
    let mut app = app_with_fixture();
    app.on_key(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert!(app.detail_open);
    app.on_key(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    assert!(!app.detail_open);
}

#[test]
fn keys_are_inert_with_no_agents() {
    let mut app = App::new(SummariesMode::On);
    assert_eq!(app.on_key(key('j')), Action::None);
    assert_eq!(
        app.on_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE)),
        Action::None
    );
    assert_eq!(app.on_key(key('r')), Action::None);
    assert_eq!(app.on_key(key('R')), Action::None);
}

/// Index reuse is not "nearest": agents can be inserted and reordered in the
/// same poll, so the old index may now hold an agent the user never chose.
#[test]
fn selection_falls_back_to_a_real_neighbour_not_whatever_took_the_index() {
    let mut app = app_with_fixture();
    let order: Vec<String> = app.agents().iter().map(|a| a.pane_id.clone()).collect();
    let selected = order[2].clone();
    while app.selected.as_ref() != Some(&selected) {
        app.on_key(key('j'));
    }

    let mut snap = fixture();
    snap.agents.retain(|a| a.pane_id != selected);
    // Insert two blocked agents in a repo that sorts before every existing
    // group, shifting every index by two.
    let template = snap
        .agents
        .iter()
        .find(|a| a.pane_id == "w3:p1")
        .unwrap()
        .clone();
    for n in 0..2 {
        let mut extra = template.clone();
        extra.pane_id = format!("wA{n}:p1");
        extra.workspace_id = format!("wA{n}");
        snap.agents.push(extra);
        let mut ws = snap.workspaces[2].clone();
        ws.workspace_id = format!("wA{n}");
        ws.label = format!("aaa-{n}");
        if let Some(wt) = ws.worktree.as_mut() {
            wt.repo_name = "aaa".into();
        }
        snap.workspaces.push(ws);
    }
    app.apply_snapshot(&snap);

    let new_order: Vec<String> = app.agents().iter().map(|a| a.pane_id.clone()).collect();
    assert_ne!(
        new_order[2], selected,
        "the removed agent is gone and its index now holds someone else"
    );
    assert_eq!(
        app.selected.as_deref(),
        Some(order[3].as_str()),
        "selection must land on the removed agent's real neighbour ({}), \
         not on whoever now occupies index 2 ({})",
        order[3],
        new_order[2]
    );
}

/// An escape hatch a modal can swallow is not an escape hatch.
#[test]
fn ctrl_c_quits_even_with_the_help_overlay_open() {
    let mut app = app_with_fixture();
    app.on_key(key('?'));
    assert!(app.show_help);
    let action = app.on_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(action, Action::Quit);
    assert!(app.should_quit);
}

#[test]
fn all_agent_ids_includes_agents_the_filter_hides() {
    let mut app = app_with_fixture();
    app.on_key(key('a'));
    assert_eq!(app.agents().len(), 3);
    assert_eq!(app.all_agent_ids().len(), 5);
}

#[test]
fn all_agent_ids_is_empty_before_the_first_snapshot() {
    assert!(App::new(SummariesMode::On).all_agent_ids().is_empty());
}

#[test]
fn the_summaries_mode_distinguishes_no_key_from_the_flag() {
    use herdash::app::SummariesMode as M;
    assert!(M::On.enabled());
    assert!(!M::OffNoKey.enabled());
    assert!(!M::OffByFlag.enabled());
    assert_eq!(M::On.note(), None);
    assert_eq!(M::OffNoKey.note(), Some("summaries off (no key)"));
    assert_eq!(M::OffByFlag.note(), Some("summaries off"));
}

fn summary_needing(reason: &str) -> herdash::summary::AgentSummary {
    herdash::summary::AgentSummary {
        headline: "h".into(),
        task: "t".into(),
        now: "n".into(),
        recent: vec![],
        needs_attention: true,
        attention_reason: reason.into(),
    }
}

fn summary_clear() -> herdash::summary::AgentSummary {
    herdash::summary::AgentSummary {
        headline: "h".into(),
        task: "t".into(),
        now: "n".into(),
        recent: vec![],
        needs_attention: false,
        attention_reason: String::new(),
    }
}

/// Attention is judged from the model's reading of the transcript, not from
/// herdr's lifecycle state — an agent can be `working` and still stuck.
#[test]
fn a_working_agent_the_model_flags_is_lifted_into_the_attention_section() {
    let mut app = app_with_fixture();
    app.slots.entry("w1:p1".into()).or_default().summary =
        Some(summary_needing("Approve writing to the seeded branch"));

    let rows = app.rows();
    assert!(
        matches!(rows[0], Row::AttentionHeader(2)),
        "the blocked agent plus the flagged working one"
    );

    let ids: Vec<String> = app.agents().iter().map(|a| a.pane_id.clone()).collect();
    assert_eq!(
        ids.iter().filter(|id| *id == "w1:p1").count(),
        1,
        "lifted, not duplicated — otherwise j/k lands on it twice"
    );
    assert!(
        ids[0..2].contains(&"w1:p1".to_string()),
        "and it sits in the attention block"
    );
}

/// The converse: herdr says blocked, but the model reads the transcript and
/// says nothing is actually wanted.
#[test]
fn a_blocked_agent_the_model_clears_is_not_in_the_attention_section() {
    let mut app = app_with_fixture();
    app.slots.entry("w3:p1".into()).or_default().summary = Some(summary_clear());
    assert!(
        !matches!(app.rows()[0], Row::AttentionHeader(_)),
        "classification overrides herdr's status"
    );
}

/// Before any summary exists there is nothing to classify, so herdr's
/// `blocked` is the best signal available.
#[test]
fn herdr_blocked_is_the_fallback_until_a_summary_arrives() {
    let app = app_with_fixture();
    let agents = app.agents();
    let blocked = agents.iter().find(|a| a.pane_id == "w3:p1").unwrap();
    assert!(app.needs_attention(blocked));
    let working = agents.iter().find(|a| a.pane_id == "w1:p1").unwrap();
    assert!(!app.needs_attention(working));
}

#[test]
fn the_attention_reason_is_exposed_for_rendering() {
    let mut app = app_with_fixture();
    app.slots.entry("w1:p1".into()).or_default().summary =
        Some(summary_needing("Decide which rounding mode to use"));
    let agents = app.agents();
    let agent = agents.iter().find(|a| a.pane_id == "w1:p1").unwrap();
    assert_eq!(
        app.attention_reason(agent),
        Some("Decide which rounding mode to use")
    );
    let other = agents.iter().find(|a| a.pane_id == "w2:p1").unwrap();
    assert_eq!(app.attention_reason(other), None);
}

#[test]
fn selection_order_follows_the_rendered_rows_exactly() {
    let mut app = app_with_fixture();
    app.slots.entry("w1:p1".into()).or_default().summary =
        Some(summary_needing("Needs a decision"));
    let from_rows: Vec<String> = app
        .rows()
        .into_iter()
        .filter_map(|r| match r {
            Row::Agent(a) => Some(a.pane_id.clone()),
            _ => None,
        })
        .collect();
    let from_agents: Vec<String> = app.agents().iter().map(|a| a.pane_id.clone()).collect();
    assert_eq!(
        from_rows, from_agents,
        "selection must never disagree with the screen"
    );
}

#[test]
fn clicking_selects_and_a_second_click_is_what_focuses() {
    let mut app = app_with_fixture();
    let target = app.agents()[2].pane_id.clone();
    assert!(app.select(&target));
    assert_eq!(app.selected.as_deref(), Some(target.as_str()));
    assert!(!app.select("nope:p9"), "an unknown pane id changes nothing");
    assert_eq!(app.selected.as_deref(), Some(target.as_str()));
}

#[test]
fn scrolling_moves_the_selection_and_clamps() {
    let mut app = app_with_fixture();
    let order: Vec<String> = app.agents().iter().map(|a| a.pane_id.clone()).collect();
    app.scroll_selection(2);
    assert_eq!(app.selected.as_deref(), Some(order[2].as_str()));
    app.scroll_selection(-100);
    assert_eq!(app.selected.as_deref(), Some(order[0].as_str()));
    app.scroll_selection(100);
    assert_eq!(
        app.selected.as_deref(),
        Some(order.last().unwrap().as_str())
    );
}
