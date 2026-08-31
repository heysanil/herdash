//! Wire-level tests for the herdr NDJSON client.

use herdash::herdr::client::Connection;
use herdash::herdr::types::{AgentStatus, ReadEnvelope, Snapshot};
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const SNAPSHOT_FIXTURE: &str = include_str!("fixtures/snapshot.json");
const READ_FIXTURE: &str = include_str!("fixtures/agent_read.json");

#[test]
fn unknown_status_strings_degrade_instead_of_erroring() {
    assert_eq!(AgentStatus::from_wire("working"), AgentStatus::Working);
    assert_eq!(AgentStatus::from_wire("idle"), AgentStatus::Idle);
    assert_eq!(AgentStatus::from_wire("blocked"), AgentStatus::Blocked);
    assert_eq!(AgentStatus::from_wire("done"), AgentStatus::Done);
    assert_eq!(AgentStatus::from_wire("unknown"), AgentStatus::Unknown);
    assert_eq!(AgentStatus::from_wire("levitating"), AgentStatus::Unknown);
}

#[test]
fn urgency_orders_blocked_first_and_unknown_last() {
    let mut order = vec![
        AgentStatus::Idle,
        AgentStatus::Unknown,
        AgentStatus::Blocked,
        AgentStatus::Working,
        AgentStatus::Done,
    ];
    order.sort_by_key(|s| s.urgency());
    assert_eq!(
        order,
        vec![
            AgentStatus::Blocked,
            AgentStatus::Done,
            AgentStatus::Working,
            AgentStatus::Idle,
            AgentStatus::Unknown
        ]
    );
}

#[test]
fn snapshot_fixture_deserializes_including_missing_worktree() {
    let v: serde_json::Value = serde_json::from_str(SNAPSHOT_FIXTURE).unwrap();
    let snap: Snapshot = serde_json::from_value(v["result"]["snapshot"].clone()).unwrap();
    assert_eq!(snap.agents.len(), 5);
    assert_eq!(snap.workspaces.len(), 5);
    let w4 = snap
        .workspaces
        .iter()
        .find(|w| w.workspace_id == "w4")
        .unwrap();
    assert!(
        w4.worktree.is_none(),
        "workspaces with no checkout omit `worktree`"
    );
    let w1 = snap
        .workspaces
        .iter()
        .find(|w| w.workspace_id == "w1")
        .unwrap();
    assert_eq!(w1.worktree.as_ref().unwrap().repo_name, "alpha");
    let mystery = snap.agents.iter().find(|a| a.pane_id == "w5:p1").unwrap();
    assert_eq!(mystery.agent_status, AgentStatus::Unknown);
}

/// The server writes a response one byte at a time; framing must still work.
#[tokio::test]
async fn request_reassembles_a_response_split_across_reads() {
    let (client_side, mut server_side) = tokio::io::duplex(64);
    let mut conn = Connection::new(client_side);

    let payload = json!({ "id": "1", "result": { "type": "pong" } }).to_string();

    tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        let _ = server_side.read(&mut buf).await;
        for byte in payload.as_bytes() {
            server_side.write_all(&[*byte]).await.unwrap();
            tokio::task::yield_now().await;
        }
        server_side.write_all(b"\n").await.unwrap();
    });

    let v: serde_json::Value = conn.request("ping", json!({})).await.unwrap();
    assert_eq!(v["type"], "pong");
}

#[tokio::test]
async fn request_skips_lines_that_are_not_its_response() {
    let (client_side, mut server_side) = tokio::io::duplex(1024);
    let mut conn = Connection::new(client_side);

    tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        let _ = server_side.read(&mut buf).await;
        // An unsolicited event, then a blank line, then the real answer.
        server_side
            .write_all(b"{\"event\":\"pane.updated\"}\n\n")
            .await
            .unwrap();
        server_side
            .write_all(b"{\"id\":\"1\",\"result\":{\"type\":\"pong\"}}\n")
            .await
            .unwrap();
    });

    let v: serde_json::Value = conn.request("ping", json!({})).await.unwrap();
    assert_eq!(v["type"], "pong");
}

#[tokio::test]
async fn request_surfaces_error_envelopes_as_errors() {
    let (client_side, mut server_side) = tokio::io::duplex(1024);
    let mut conn = Connection::new(client_side);

    tokio::spawn(async move {
        let mut buf = [0u8; 1024];
        let _ = server_side.read(&mut buf).await;
        server_side
            .write_all(
                b"{\"id\":\"1\",\"error\":{\"code\":\"invalid_request\",\"message\":\"boom\"}}\n",
            )
            .await
            .unwrap();
    });

    let err = conn
        .request::<_, serde_json::Value>("nope", json!({}))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("invalid_request"), "got: {err}");
    assert!(err.contains("boom"), "got: {err}");
}

#[tokio::test]
async fn a_closed_connection_is_an_error_not_a_hang() {
    let (client_side, server_side) = tokio::io::duplex(64);
    let mut conn = Connection::new(client_side);
    drop(server_side);
    let err = conn
        .request::<_, serde_json::Value>("ping", json!({}))
        .await
        .unwrap_err()
        .to_string();
    let lower = err.to_lowercase();
    assert!(
        lower.contains("closed") || lower.contains("broken pipe"),
        "got: {err}"
    );
}

#[test]
fn read_result_extracts_nested_text() {
    let v: serde_json::Value = serde_json::from_str(READ_FIXTURE).unwrap();
    let env: ReadEnvelope = serde_json::from_value(v["result"].clone()).unwrap();
    assert!(env.read.text.contains("Now editing proration()"));
    // herdr always reports 0 for a read's revision — the field exists but
    // carries no information. Change detection uses the snapshot revision.
    assert_eq!(env.read.revision, 0);
}

/// herdr's schema declares `agent`, `cwd` and `terminal_title_stripped` as
/// `["string", "null"]`. `#[serde(default)]` only covers an *absent* key, so
/// an explicit `null` would otherwise be a hard error that blanks the whole
/// dashboard.
#[test]
fn explicit_nulls_deserialize_to_defaults_rather_than_failing() {
    let v: serde_json::Value = serde_json::from_str(SNAPSHOT_FIXTURE).unwrap();
    let snap: Snapshot = serde_json::from_value(v["result"]["snapshot"].clone()).unwrap();
    let nulled = snap.agents.iter().find(|a| a.pane_id == "w5:p1").unwrap();
    assert_eq!(nulled.agent, "");
    assert_eq!(nulled.cwd, "");
    assert_eq!(nulled.terminal_title_stripped, "");
}

#[test]
fn a_null_heavy_payload_never_fails_to_parse() {
    let raw = serde_json::json!({
        "agents": [{
            "agent": null,
            "agent_status": "working",
            "workspace_id": "w9",
            "pane_id": "w9:p1",
            "tab_id": null,
            "terminal_title_stripped": null,
            "cwd": null
        }],
        "workspaces": [{ "workspace_id": "w9", "label": null, "worktree": null }]
    });
    let snap: Snapshot = serde_json::from_value(raw).unwrap();
    assert_eq!(snap.agents.len(), 1);
    assert!(snap.workspaces[0].worktree.is_none());
    assert_eq!(snap.workspaces[0].label, "");
}

/// The fixture must be a shape herdr could actually produce, or it silently
/// stops guarding anything.
#[test]
fn the_fixture_matches_the_real_wire_shape() {
    let v: serde_json::Value = serde_json::from_str(SNAPSHOT_FIXTURE).unwrap();
    let snap = &v["result"]["snapshot"];
    assert!(
        snap["version"].is_string(),
        "herdr reports version as a string, e.g. \"0.8.2\""
    );
    assert_eq!(snap["protocol"], 20);
    for w in snap["workspaces"].as_array().unwrap() {
        assert!(
            w["active_tab_id"].is_string(),
            "every workspace carries active_tab_id"
        );
    }
    let read: serde_json::Value = serde_json::from_str(READ_FIXTURE).unwrap();
    assert_eq!(
        read["result"]["type"], "pane_read",
        "the discriminator is pane_read"
    );
}
