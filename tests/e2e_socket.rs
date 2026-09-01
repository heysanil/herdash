//! Exercises the real client against a stub herdr server over a real Unix
//! socket, so framing, request shape and the error path are all covered
//! without a herdr installation.

use std::path::PathBuf;
use std::time::Duration;
use std::sync::atomic::{AtomicU32, Ordering};

use herdash::herdr::client::Client;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

const SNAPSHOT_FIXTURE: &str = include_str!("fixtures/snapshot.json");
const READ_FIXTURE: &str = include_str!("fixtures/agent_read.json");

static COUNTER: AtomicU32 = AtomicU32::new(0);

fn socket_path(tag: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    // Unix socket paths are length-limited, so keep this short.
    let p = std::env::temp_dir().join(format!("hd-{}-{n}-{tag}.sock", std::process::id()));
    let _ = std::fs::remove_file(&p);
    p
}

/// A faithful herdr stub: accepts connections forever, and for each one reads
/// exactly one request, writes one response, then **closes**.
///
/// That last part is the important bit. Real herdr 0.8.2 does this — a second
/// request on the same stream fails with `EPIPE` — and an earlier version of
/// this client assumed a long-lived multiplexed connection, which worked
/// against a more forgiving stub and failed instantly against the real server.
async fn serve(listener: UnixListener, tx: UnboundedSender<String>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let tx = tx.clone();
        tokio::spawn(async move {
            let (read_half, mut write_half) = stream.into_split();
            let mut lines = BufReader::new(read_half).lines();
            let Ok(Some(line)) = lines.next_line().await else {
                return;
            };
            let Ok(req) = serde_json::from_str::<serde_json::Value>(&line) else {
                return;
            };
            let _ = tx.send(line.clone());
            let id = req["id"].as_str().unwrap_or_default().to_string();
            let method = req["method"].as_str().unwrap_or_default();
            let response = match method {
                "session.snapshot" => {
                    let mut v: serde_json::Value = serde_json::from_str(SNAPSHOT_FIXTURE).unwrap();
                    v["id"] = serde_json::Value::String(id);
                    v
                }
                "agent.read" => {
                    // Mirror herdr: a working agent on the alternate screen
                    // refuses any read larger than its viewport.
                    let source = req["params"]["source"].as_str().unwrap_or_default();
                    let lines = req["params"]["lines"].as_u64().unwrap_or(0);
                    let alt_screen_rows = 40;
                    if req["params"]["target"] == "busy:p1"
                        && source != "visible"
                        && lines > alt_screen_rows
                    {
                        serde_json::json!({
                            "id": id,
                            "error": {
                                "code": "agent_not_idle",
                                "message": format!(
                                    "cannot read {lines} lines while busy:p1 is working"
                                )
                            }
                        })
                    } else {
                        let mut v: serde_json::Value = serde_json::from_str(READ_FIXTURE).unwrap();
                        v["id"] = serde_json::Value::String(id);
                        v["result"]["read"]["source"] =
                            serde_json::Value::String(source.to_string());
                        v
                    }
                }
                "agent.focus" => {
                    serde_json::json!({"id": id, "result": {"type": "agent_focus"}})
                }
                "workspace.report_metadata" => {
                    serde_json::json!({"id": id, "result": {"type": "ok"}})
                }
                other => serde_json::json!({
                    "id": id,
                    "error": {
                        "code": "unknown_method",
                        "message": format!("no such method: {other}")
                    }
                }),
            };
            let _ = write_half.write_all(response.to_string().as_bytes()).await;
            let _ = write_half.write_all(b"\n").await;
            // Close, exactly as herdr does.
        });
    }
}

#[tokio::test]
async fn snapshot_round_trips_over_a_real_socket() {
    let path = socket_path("snap");
    let listener = UnixListener::bind(&path).unwrap();
    let (tx, _rx) = unbounded_channel();
    tokio::spawn(serve(listener, tx));

    let client = Client::new(&path);
    let snap = client.snapshot().await.unwrap();
    assert_eq!(snap.agents.len(), 5);
    assert_eq!(snap.workspaces.len(), 5);
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn agent_read_sends_the_underscored_source_and_returns_the_nested_text() {
    let path = socket_path("read");
    let listener = UnixListener::bind(&path).unwrap();
    let (tx, mut rx) = unbounded_channel();
    tokio::spawn(serve(listener, tx));

    let client = Client::new(&path);
    let read = client.read_agent("w1:p1", 200).await.unwrap();
    assert!(read.text.contains("Now editing proration()"));
    // herdr always reports 0 here. The field exists but carries no
    // information, which is why change detection uses the snapshot revision.
    assert_eq!(read.revision, 0);

    let sent: serde_json::Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert_eq!(sent["method"], "agent.read");
    assert_eq!(
        sent["params"]["source"], "recent_unwrapped",
        "the hyphenated CLI spelling is rejected on the wire"
    );
    assert_eq!(sent["params"]["target"], "w1:p1");
    assert_eq!(sent["params"]["lines"], 200);
    assert_eq!(sent["params"]["strip_ansi"], true);
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn every_request_carries_a_params_object_even_when_empty() {
    let path = socket_path("params");
    let listener = UnixListener::bind(&path).unwrap();
    let (tx, mut rx) = unbounded_channel();
    tokio::spawn(serve(listener, tx));

    let client = Client::new(&path);
    client.snapshot().await.unwrap();
    let sent: serde_json::Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert!(
        sent["params"].is_object(),
        "herdr rejects a request with no params"
    );
    let _ = std::fs::remove_file(&path);
}

/// Regression test for the design's original mistake: herdr closes the socket
/// after a single response, so a client that keeps one connection open dies on
/// its second request.
#[tokio::test]
async fn repeated_calls_succeed_against_a_server_that_closes_after_each_response() {
    let path = socket_path("seq");
    let listener = UnixListener::bind(&path).unwrap();
    let (tx, mut rx) = unbounded_channel();
    tokio::spawn(serve(listener, tx));

    let client = Client::new(&path);
    for i in 0..5 {
        let snap = client
            .snapshot()
            .await
            .unwrap_or_else(|e| panic!("call {i} failed: {e}"));
        assert_eq!(snap.agents.len(), 5);
    }
    client.focus_agent("w1:p1").await.unwrap();
    client.read_agent("w1:p1", 10).await.unwrap();

    let mut seen = 0;
    while rx.try_recv().is_ok() {
        seen += 1;
    }
    assert_eq!(
        seen, 7,
        "every call is its own connection and its own request"
    );
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn a_server_error_becomes_a_rust_error_rather_than_a_hang() {
    let path = socket_path("err");
    let listener = UnixListener::bind(&path).unwrap();
    let (tx, _rx) = unbounded_channel();
    tokio::spawn(serve(listener, tx));

    let client = Client::new(&path);
    let err = client
        .request::<_, serde_json::Value>("bogus.method", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("unknown_method"), "got: {err}");
    assert!(
        !err.is_transport(),
        "a rejected method means herdr is alive; it must not read as a lost connection"
    );
    let _ = std::fs::remove_file(&path);
}

#[tokio::test]
async fn connecting_to_a_missing_socket_fails_with_the_path_in_the_message() {
    let path = std::env::temp_dir().join("herdash-definitely-not-here.sock");
    let _ = std::fs::remove_file(&path);
    let err = Client::new(&path).snapshot().await.unwrap_err().to_string();
    assert!(
        err.contains("herdash-definitely-not-here.sock"),
        "got: {err}"
    );
}

/// Regression test for the constraint that broke summarization of every
/// *working* agent: herdr refuses a read larger than the alternate-screen
/// viewport, so the client must fall back to `visible` rather than give up.
#[tokio::test]
async fn an_oversized_read_on_a_working_agent_falls_back_to_the_visible_viewport() {
    let path = socket_path("altscreen");
    let listener = UnixListener::bind(&path).unwrap();
    let (tx, mut rx) = unbounded_channel();
    tokio::spawn(serve(listener, tx));

    let client = Client::new(&path);
    let read = client
        .read_agent("busy:p1", 200)
        .await
        .expect("must not give up");
    assert!(read.text.contains("Now editing proration()"));
    assert_eq!(
        read.source.as_deref(),
        Some("visible"),
        "fell back to the viewport source"
    );

    let first: serde_json::Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert_eq!(
        first["params"]["source"], "recent_unwrapped",
        "full history is tried first"
    );
    let second: serde_json::Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert_eq!(second["params"]["source"], "visible");
    assert!(
        second["params"]["lines"].is_null(),
        "the viewport read is unbounded"
    );
    let _ = std::fs::remove_file(&path);
}

/// An idle agent must not pay for the fallback.
#[tokio::test]
async fn a_normal_read_uses_full_history_and_makes_one_call() {
    let path = socket_path("idle-read");
    let listener = UnixListener::bind(&path).unwrap();
    let (tx, mut rx) = unbounded_channel();
    tokio::spawn(serve(listener, tx));

    let client = Client::new(&path);
    client.read_agent("w1:p1", 200).await.unwrap();

    let first: serde_json::Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert_eq!(first["params"]["source"], "recent_unwrapped");
    assert!(
        rx.try_recv().is_err(),
        "no fallback call for an agent that answered"
    );
    let _ = std::fs::remove_file(&path);
}

/// herdash names itself in herdr's sidebar by publishing a `$herdash`
/// metadata token, rather than renaming the workspace — a rename would
/// relabel every neighbouring pane's space too. The TTL is what makes the
/// token safe: herdr expires it, so a killed herdash leaves nothing behind.
#[tokio::test]
async fn the_sidebar_token_is_published_with_a_ttl() {
    let path = socket_path("token");
    let listener = UnixListener::bind(&path).unwrap();
    let (tx, mut rx) = unbounded_channel();
    tokio::spawn(serve(listener, tx));

    let client = Client::new(&path);
    client
        .report_workspace_token("w1", "herdash", Some("herdash"), Duration::from_secs(30))
        .await
        .unwrap();

    let sent: serde_json::Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert_eq!(sent["method"], "workspace.report_metadata");
    assert_eq!(sent["params"]["workspace_id"], "w1");
    assert_eq!(sent["params"]["source"], "herdash", "attributed, so herdr can scope it");
    assert_eq!(sent["params"]["tokens"]["herdash"], "herdash");
    assert_eq!(sent["params"]["ttl_ms"], 30_000);
    let _ = std::fs::remove_file(&path);
}

/// Clearing sends an explicit null, which is how herdr removes a token.
#[tokio::test]
async fn the_sidebar_token_is_cleared_with_a_null_value() {
    let path = socket_path("token-clear");
    let listener = UnixListener::bind(&path).unwrap();
    let (tx, mut rx) = unbounded_channel();
    tokio::spawn(serve(listener, tx));

    Client::new(&path)
        .report_workspace_token("w1", "herdash", None, Duration::from_secs(1))
        .await
        .unwrap();

    let sent: serde_json::Value = serde_json::from_str(&rx.recv().await.unwrap()).unwrap();
    assert!(sent["params"]["tokens"]["herdash"].is_null());
    let _ = std::fs::remove_file(&path);
}
