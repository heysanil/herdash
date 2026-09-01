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

/// A stale rejection arriving late must not lower the cached rung.
///
/// Deterministic on purpose: the interleaving this guards against does not
/// reproduce reliably through the HTTP path, so the invariant is asserted
/// directly. Swap `fetch_max` back to `store` and this test fails.
#[test]
fn a_late_rejection_never_lowers_the_cached_rung() {
    use herdash::summary::openrouter::{ReasoningMode as M, advance_rung};
    use std::sync::atomic::{AtomicU8, Ordering};

    let cache = AtomicU8::new(0);

    // A fast worker walks the ladder to the top.
    advance_rung(&cache, M::LowEffort);
    advance_rung(&cache, M::ProviderDefault);
    assert_eq!(cache.load(Ordering::Relaxed), 2);

    // A slow worker that read rung 0 long ago now reports its own rejection.
    advance_rung(&cache, M::LowEffort);
    assert_eq!(
        cache.load(Ordering::Relaxed),
        2,
        "a late rejection lowered the cached rung"
    );

    // And the bottom rung is equally inert once the ladder has moved.
    advance_rung(&cache, M::Disabled);
    assert_eq!(cache.load(Ordering::Relaxed), 2);
}

/// Six concurrent workers against one client all converge and succeed.
///
/// A liveness check, not a monotonicity check: every worker reads the cached
/// rung once at call entry, this runtime is single-threaded, and the stub
/// accepts `effort:low` unconditionally — so the sequence is always six
/// `disabled` then six `effort:low`, and no worker can return a stale rung
/// after another has passed it. The monotonic invariant is asserted directly
/// and deterministically in `a_late_rejection_never_lowers_the_cached_rung`;
/// this test guards that concurrent escalation over a real socket does not
/// deadlock or drop a worker.
#[tokio::test]
async fn concurrent_calls_converge_on_one_rung() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    tokio::spawn(stub(listener, Arc::clone(&seen), true, Arc::clone(&calls)));

    let client = Arc::new(OpenRouter::new("k".into(), "m".into()).with_endpoint(endpoint));
    let mut set = tokio::task::JoinSet::new();
    for _ in 0..6 {
        let c = Arc::clone(&client);
        set.spawn(async move { c.summarize_agent("transcript").await });
    }
    while let Some(r) = set.join_next().await {
        r.unwrap().expect("summary should succeed after escalation");
    }

    // Every worker may try `disabled` once before the cache settles, but no
    // worker may try it after the cache has moved past it.
    let modes = seen.lock().unwrap().clone();
    let last_disabled = modes.iter().rposition(|m| m == "disabled");
    let first_effort = modes.iter().position(|m| m.starts_with("effort:"));
    if let (Some(d), Some(e)) = (last_disabled, first_effort) {
        assert!(
            d < e || modes[..d].iter().all(|m| m == "disabled"),
            "a refused rung was re-sent after the cache advanced: {modes:?}"
        );
    }
}
