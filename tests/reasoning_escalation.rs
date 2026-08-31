//! The reasoning-mode negotiation, exercised against a stub OpenRouter.
//!
//! This suite exists because a live run once found the escalation code was
//! never called at all: the state machine was correct and fully unit-tested,
//! but nothing wired it into the request path, so every call to a provider
//! that mandates reasoning failed. Testing the pure state machine was not
//! enough — the seam had to be exercised end to end.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use herdash::summary::Summarizer;
use herdash::summary::openrouter::OpenRouter;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// Minimal HTTP server that records each request's `reasoning` field and
/// replies according to `behavior`.
async fn stub(
    listener: TcpListener,
    seen: Arc<std::sync::Mutex<Vec<String>>>,
    reject_disabled: bool,
    calls: Arc<AtomicUsize>,
) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let seen = Arc::clone(&seen);
        let calls = Arc::clone(&calls);
        tokio::spawn(async move {
            let (r, mut w) = socket.split();
            let mut reader = BufReader::new(r);
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                let lower = line.to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    content_length = v.trim().parse().unwrap_or(0);
                }
                if line == "\r\n" || line == "\n" {
                    break;
                }
            }
            let mut body = vec![0u8; content_length];
            tokio::io::AsyncReadExt::read_exact(&mut reader, &mut body)
                .await
                .ok();
            let json: serde_json::Value =
                serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
            let mode = match json.get("reasoning") {
                None => "none".to_string(),
                Some(v) if v.get("enabled") == Some(&serde_json::json!(false)) => {
                    "disabled".to_string()
                }
                Some(v) => format!("effort:{}", v["effort"].as_str().unwrap_or("?")),
            };
            seen.lock().unwrap().push(mode.clone());
            calls.fetch_add(1, Ordering::SeqCst);

            let (status, payload) = if reject_disabled && mode == "disabled" {
                (
                    "400 Bad Request",
                    serde_json::json!({"error":{"message":
                        "Reasoning is mandatory for this endpoint and cannot be disabled."}}),
                )
            } else {
                (
                    "200 OK",
                    serde_json::json!({"choices":[{"message":{"content":
                        r#"{"headline":"h","task":"t","now":"n","recent":[],"needs_attention":false,"attention_reason":""}"#
                    }}]}),
                )
            };
            let body = payload.to_string();
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = w.write_all(resp.as_bytes()).await;
            let _ = w.flush().await;
        });
    }
}

async fn spawn_stub(
    reject_disabled: bool,
) -> (String, Arc<std::sync::Mutex<Vec<String>>>, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    tokio::spawn(stub(
        listener,
        Arc::clone(&seen),
        reject_disabled,
        Arc::clone(&calls),
    ));
    (format!("http://{addr}/v1/chat/completions"), seen, calls)
}

/// The happy path: reasoning is asked to be off, and stays off.
#[tokio::test]
async fn a_permissive_provider_is_only_ever_asked_for_no_reasoning() {
    let (endpoint, seen, calls) = spawn_stub(false).await;
    let client = OpenRouter::new("k".into(), "m".into()).with_endpoint(endpoint);

    client.summarize_agent("transcript").await.unwrap();
    client.summarize_agent("transcript").await.unwrap();

    assert_eq!(calls.load(Ordering::SeqCst), 2, "no wasted retries");
    assert_eq!(*seen.lock().unwrap(), vec!["disabled", "disabled"]);
}

/// The regression this suite exists for: a provider that mandates reasoning
/// must be retried with low effort rather than failing every call.
#[tokio::test]
async fn a_provider_that_mandates_reasoning_is_retried_with_low_effort() {
    let (endpoint, seen, _calls) = spawn_stub(true).await;
    let client = OpenRouter::new("k".into(), "m".into()).with_endpoint(endpoint);

    let summary = client
        .summarize_agent("transcript")
        .await
        .expect("must escalate rather than fail");
    assert_eq!(summary.headline, "h");
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["disabled", "effort:low"],
        "cheapest first, then escalate"
    );
}

/// And the escalation must be remembered, or every call pays the round trip.
#[tokio::test]
async fn the_escalated_mode_is_cached_for_later_calls() {
    let (endpoint, seen, calls) = spawn_stub(true).await;
    let client = OpenRouter::new("k".into(), "m".into()).with_endpoint(endpoint);

    client.summarize_agent("one").await.unwrap();
    client.summarize_agent("two").await.unwrap();
    client.summarize_agent("three").await.unwrap();

    assert_eq!(
        calls.load(Ordering::SeqCst),
        4,
        "one rejected probe, then three straight to low effort"
    );
    assert_eq!(
        *seen.lock().unwrap(),
        vec!["disabled", "effort:low", "effort:low", "effort:low"]
    );
}

/// The fleet overview goes through the same negotiation.
#[tokio::test]
async fn the_fleet_summary_negotiates_reasoning_too() {
    let (endpoint, seen, _calls) = spawn_stub(true).await;
    let client = OpenRouter::new("k".into(), "m".into()).with_endpoint(endpoint);
    let _ = client.summarize_fleet(&["a".into(), "b".into()]).await;
    assert_eq!(*seen.lock().unwrap(), vec!["disabled", "effort:low"]);
}
