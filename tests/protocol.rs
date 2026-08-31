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
    let w4 = snap.workspaces.iter().find(|w| w.workspace_id == "w4").unwrap();
    assert!(w4.worktree.is_none(), "workspaces with no checkout omit `worktree`");
    let w1 = snap.workspaces.iter().find(|w| w.workspace_id == "w1").unwrap();
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
        server_side.write_all(b"{\"event\":\"pane.updated\"}\n\n").await.unwrap();
        server_side.write_all(b"{\"id\":\"1\",\"result\":{\"type\":\"pong\"}}\n").await.unwrap();
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
            .write_all(b"{\"id\":\"1\",\"error\":{\"code\":\"invalid_request\",\"message\":\"boom\"}}\n")
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
    assert_eq!(env.read.revision, 10);
}
