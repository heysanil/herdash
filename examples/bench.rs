//! Model benchmark for herdash summarisation.
//!
//! Deliberately reuses [`agent_request_body`] — the exact request the
//! dashboard sends — so the benchmark measures the shipped prompt and schema
//! rather than a re-implementation that could drift from it.
//!
//! Usage:
//!   cargo run --release --example bench -- <transcript-dir> <out.jsonl> [runs]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use herdash::summary::openrouter::{
    ReasoningMode, agent_request_body_with, is_reasoning_rejection,
};
use serde_json::{Value, json};

const MODELS: &[&str] = &[
    "meta-llama/llama-4-scout:nitro",
    "openai/gpt-oss-120b",
    "openai/gpt-oss-20b:nitro",
    "nvidia/nemotron-3.5-lightning",
    "qwen/qwen3.5-35b-a3b:nitro",
    "moonshotai/kimi-k2.6:nitro",
    "google/gemini-3.5-flash-lite",
];

const ENDPOINT: &str = "https://openrouter.ai/api/v1/chat/completions";

#[tokio::main(flavor = "multi_thread")]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(args.get(1).map(String::as_str).unwrap_or("transcripts"));
    let out_path = PathBuf::from(args.get(2).map(String::as_str).unwrap_or("bench.jsonl"));
    let runs: usize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(5);

    let key = std::env::var("OPENROUTER_API_KEY").ok().or_else(|| {
        std::fs::read_to_string(
            std::env::var("HOME").map(PathBuf::from).unwrap().join(".openrouter-key"),
        )
        .ok()
        .map(|s| s.trim().to_string())
    });
    let key = key.expect("no OpenRouter key");

    let mut transcripts: Vec<(String, String)> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let path = entry?.path();
        if path.extension().and_then(|e| e.to_str()) == Some("txt") {
            let name = path.file_stem().unwrap().to_string_lossy().to_string();
            transcripts.push((name, std::fs::read_to_string(&path)?));
        }
    }
    transcripts.sort();
    eprintln!("{} transcripts, {} models, {runs} runs each", transcripts.len(), MODELS.len());

    let client = reqwest::Client::builder().timeout(Duration::from_secs(180)).build()?;

    // Models run concurrently, transcripts sequentially within a model, so a
    // slow provider cannot distort another model's latency measurements.
    let mut set = tokio::task::JoinSet::new();
    for model in MODELS {
        let client = client.clone();
        let key = key.clone();
        let transcripts = transcripts.clone();
        set.spawn(async move {
            let mut records = Vec::new();
            for run in 1..=runs {
                for (name, text) in &transcripts {
                    records.push(one_call(&client, &key, model, name, run, text).await);
                    tokio::time::sleep(Duration::from_millis(150)).await;
                }
            }
            eprintln!("done: {model}");
            records
        });
    }

    let mut all = Vec::new();
    while let Some(res) = set.join_next().await {
        all.extend(res?);
    }
    let body: String =
        all.iter().map(|r| format!("{}\n", serde_json::to_string(r).unwrap())).collect();
    std::fs::write(&out_path, body)?;
    eprintln!("wrote {} records to {}", all.len(), out_path.display());
    Ok(())
}

async fn one_call(
    client: &reqwest::Client,
    key: &str,
    model: &str,
    transcript_name: &str,
    run: usize,
    text: &str,
) -> Value {
    // Mirror the shipped client: start at the cheapest reasoning mode and
    // escalate only when the provider explicitly refuses it. Benchmarking a
    // single fixed mode would measure a request herdash never sends.
    let mut mode = ReasoningMode::Disabled;
    let mut total_latency = 0u64;
    let mut attempts = 0u32;
    loop {
        attempts += 1;
        let body = agent_request_body_with(model, text, mode);
        let started = Instant::now();
        let resp = client.post(ENDPOINT).bearer_auth(key).json(&body).send().await;
        total_latency += started.elapsed().as_millis() as u64;

        let mut rec = json!({
            "model": model,
            "transcript": transcript_name,
            "run": run,
            "latency_ms": total_latency,
            "attempts": attempts,
            "reasoning_mode": format!("{mode:?}"),
        });

        let (status, text_body) = match resp {
            Ok(r) => {
                let s = r.status().as_u16();
                (s, r.text().await.unwrap_or_default())
            }
            Err(e) => {
                rec["ok"] = json!(false);
                rec["error"] = json!(format!("transport: {e}"));
                return rec;
            }
        };
        rec["http_status"] = json!(status);

        let parsed: Value = match serde_json::from_str(&text_body) {
            Ok(v) => v,
            Err(_) => {
                rec["ok"] = json!(false);
                rec["error"] = json!("non-JSON response");
                return rec;
            }
        };

        if let Some(err) = parsed.get("error") {
            let message = err.get("message").and_then(|m| m.as_str()).unwrap_or("error");
            if is_reasoning_rejection(message)
                && let Some(next) = mode.escalate()
            {
                mode = next;
                continue;
            }
            rec["ok"] = json!(false);
            rec["error"] = json!(message);
            return rec;
        }

        rec["provider"] = parsed.get("provider").cloned().unwrap_or(Value::Null);
        rec["finish_reason"] = parsed["choices"][0]["finish_reason"].clone();
        if let Some(usage) = parsed.get("usage") {
            rec["prompt_tokens"] = usage.get("prompt_tokens").cloned().unwrap_or(Value::Null);
            rec["completion_tokens"] =
                usage.get("completion_tokens").cloned().unwrap_or(Value::Null);
            rec["cost"] = usage.get("cost").cloned().unwrap_or(Value::Null);
        }
        rec["reasoning_chars"] =
            json!(parsed["choices"][0]["message"]["reasoning"].as_str().unwrap_or("").len());

        let content = parsed["choices"][0]["message"]["content"].as_str().unwrap_or_default();
        rec["raw"] = json!(content);

        // Compliance is judged by the shipped parser, not a looser one.
        match herdash::summary::openrouter::parse_agent_response(&text_body) {
            Ok(summary) => {
                rec["ok"] = json!(true);
                rec["summary"] = serde_json::to_value(&summary).unwrap();
            }
            Err(e) => {
                rec["ok"] = json!(false);
                rec["error"] = json!(format!("parse: {e}"));
            }
        }
        return rec;
    }
}
