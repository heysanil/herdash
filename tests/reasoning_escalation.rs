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
use herdash::summary::client::LlmClient;
use herdash::summary::provider::{Dialect, ProviderId, ResolvedProvider, preset};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

/// An OpenRouter-shaped provider pointed at a local stub.
fn stub_provider(endpoint: &str) -> ResolvedProvider {
    let _ = endpoint; // the URL is applied via with_endpoint
    ResolvedProvider {
        id: ProviderId::Openrouter,
        dialect: preset(ProviderId::Openrouter).dialect,
        base_url: "http://unused".into(),
        api_key: Some("k".into()),
        model: "m".into(),
    }
}

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
    let client = LlmClient::new(stub_provider(&endpoint)).with_endpoint(endpoint);

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
    let client = LlmClient::new(stub_provider(&endpoint)).with_endpoint(endpoint);

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
    let client = LlmClient::new(stub_provider(&endpoint)).with_endpoint(endpoint);

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
    let client = LlmClient::new(stub_provider(&endpoint)).with_endpoint(endpoint);
    let _ = client.summarize_fleet(&["a".into(), "b".into()]).await;
    assert_eq!(*seen.lock().unwrap(), vec!["disabled", "effort:low"]);
}

/// A generic OpenAI-compatible endpoint (e.g. llama.cpp-backed) has no
/// reasoning knob, so it must start at the dialect's own default rung rather
/// than probing a lower one first.
#[tokio::test]
async fn a_generic_endpoint_starts_at_provider_default() {
    // llama.cpp-backed servers have no reasoning knob; starting lower only
    // buys a wasted round trip.
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let calls = Arc::new(AtomicUsize::new(0));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    tokio::spawn(stub(listener, Arc::clone(&seen), false, Arc::clone(&calls)));

    let mut p = stub_provider(&endpoint);
    p.id = ProviderId::Ollama;
    p.dialect = Dialect::OpenAiGeneric;
    p.api_key = None;
    let client = LlmClient::new(p).with_endpoint(endpoint);
    client.summarize_agent("t").await.unwrap();

    assert_eq!(seen.lock().unwrap().as_slice(), ["none"]);
    assert_eq!(calls.load(Ordering::Relaxed), 1, "no wasted round trip");
}

/// Minimal HTTP server that records the request's headers and JSON body,
/// then always replies 200 with `payload`. Kept separate from [`stub`]
/// (which parses `reasoning` out of the body for the escalation tests
/// above) because contorting one helper to serve both purposes would only
/// obscure what each test is actually asserting.
async fn header_stub(
    listener: TcpListener,
    headers: Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>,
    body_out: Arc<std::sync::Mutex<serde_json::Value>>,
    payload: serde_json::Value,
) {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let headers = Arc::clone(&headers);
        let body_out = Arc::clone(&body_out);
        let payload = payload.clone();
        tokio::spawn(async move {
            let (r, mut w) = socket.split();
            let mut reader = BufReader::new(r);
            let mut content_length = 0usize;
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                if line == "\r\n" || line == "\n" {
                    break;
                }
                if let Some((k, v)) = line.trim_end().split_once(':') {
                    let k = k.trim().to_ascii_lowercase();
                    let v = v.trim().to_string();
                    if k == "content-length" {
                        content_length = v.parse().unwrap_or(0);
                    }
                    headers.lock().unwrap().insert(k, v);
                }
            }
            let mut body = vec![0u8; content_length];
            tokio::io::AsyncReadExt::read_exact(&mut reader, &mut body)
                .await
                .ok();
            let json: serde_json::Value =
                serde_json::from_slice(&body).unwrap_or(serde_json::Value::Null);
            *body_out.lock().unwrap() = json;

            let body = payload.to_string();
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = w.write_all(resp.as_bytes()).await;
            let _ = w.flush().await;
        });
    }
}

/// No test drove `LlmClient` in Anthropic dialect at all: every stub above
/// is OpenAI-wire. This is the one that exercises `x-api-key` +
/// `anthropic-version`, the absence of a bearer token, and that the
/// starting rung actually disables thinking on the wire.
#[tokio::test]
async fn the_anthropic_wire_sends_x_api_key_and_no_bearer_token() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1/messages", listener.local_addr().unwrap());
    let headers = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let body_out = Arc::new(std::sync::Mutex::new(serde_json::Value::Null));
    let payload = serde_json::json!({
        "stop_reason": "end_turn",
        "content": [{
            "type": "text",
            "text": r#"{"headline":"h","task":"t","now":"n","recent":[],"needs_attention":false,"attention_reason":""}"#
        }]
    });
    tokio::spawn(header_stub(
        listener,
        Arc::clone(&headers),
        Arc::clone(&body_out),
        payload,
    ));

    let provider = ResolvedProvider {
        id: ProviderId::Anthropic,
        dialect: preset(ProviderId::Anthropic).dialect,
        base_url: "http://unused".into(),
        api_key: Some("k".into()),
        model: "m".into(),
    };
    let client = LlmClient::new(provider).with_endpoint(endpoint);
    client
        .summarize_agent("transcript")
        .await
        .expect("the Anthropic-shaped stub response must parse");

    let seen = headers.lock().unwrap();
    assert_eq!(seen.get("x-api-key").map(String::as_str), Some("k"));
    assert_eq!(
        seen.get("anthropic-version").map(String::as_str),
        Some(herdash::summary::anthropic::ANTHROPIC_VERSION)
    );
    assert!(
        seen.get("authorization").is_none(),
        "the Anthropic wire must never send a bearer token: {seen:?}"
    );
    drop(seen);

    let body = body_out.lock().unwrap();
    assert_eq!(
        body["thinking"],
        serde_json::json!({"type": "disabled"}),
        "the starting rung must disable thinking on the wire"
    );
}

/// The sibling check on the OpenAI wire, so the header gap is closed on
/// both sides in the same pass.
#[tokio::test]
async fn the_openai_wire_sends_a_bearer_token_and_no_x_api_key() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().unwrap()
    );
    let headers = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
    let body_out = Arc::new(std::sync::Mutex::new(serde_json::Value::Null));
    let payload = serde_json::json!({"choices":[{"message":{"content":
        r#"{"headline":"h","task":"t","now":"n","recent":[],"needs_attention":false,"attention_reason":""}"#
    }}]});
    tokio::spawn(header_stub(
        listener,
        Arc::clone(&headers),
        Arc::clone(&body_out),
        payload,
    ));

    let client = LlmClient::new(stub_provider(&endpoint)).with_endpoint(endpoint);
    client.summarize_agent("transcript").await.unwrap();

    let seen = headers.lock().unwrap();
    assert_eq!(
        seen.get("authorization").map(String::as_str),
        Some("Bearer k")
    );
    assert!(
        seen.get("x-api-key").is_none(),
        "the OpenAI wire must never send x-api-key: {seen:?}"
    );
}

/// A stale rejection arriving late must not lower the cached rung.
///
/// Deterministic on purpose: the interleaving this guards against does not
/// reproduce reliably through the HTTP path, so the invariant is asserted
/// directly. Swap `fetch_max` back to `store` and this test fails.
#[test]
fn a_late_rejection_never_lowers_the_cached_rung() {
    use herdash::summary::client::advance_rung;
    use herdash::summary::provider::ReasoningMode as M;
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

    let client = Arc::new(LlmClient::new(stub_provider(&endpoint)).with_endpoint(endpoint));
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
