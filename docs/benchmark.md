# Summarization model benchmark

herdash sends one structured-output request per agent summary. This measures
which model to send it to.

Run it yourself:

```bash
mise exec -- cargo run --release --example bench -- <transcript-dir> out.jsonl 5
```

`examples/bench.rs` deliberately calls `agent_request_body_with` — the exact
request the dashboard sends — so it measures the shipped prompt and schema
rather than a re-implementation that could drift.

## Method

- **7 models × 6 transcripts × 5 runs = 210 calls.** Transcripts were captured
  live from real herdr agents (1.8 KB–19 KB), spanning `working` and `idle`
  states. They contain real source code and are **not** committed; only the
  metrics are, in `benchmark-results.jsonl`.
- **Success** means the shipped parser accepted the response. A looser parser
  would flatter the models without helping the dashboard.
- **Attention accuracy** is scored against hand-labelled ground truth. Exactly
  one of the six transcripts contains an unanswered question to the human
  (`"Next, tell me whether to write the expiry-mismatch guard"`); the other
  five do not, including two idle agents that had finished cleanly and one
  where the *human* was mid-sentence in the prompt box.
- **Quality** is scored blind by `anthropic/claude-opus-4.8` — deliberately not
  one of the candidates — on shuffled, anonymised summaries, 1–5 for
  faithfulness and usefulness.
- **Cost** averages over *all* calls including failures, because a failed call
  still bills.

## Results

| model | ok | attention | faithful | useful | best/worst | p50 | $/1k calls |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `openai/gpt-oss-120b` | 100% | **100%** | 4.17 | 4.33 | 0 / 0 | 265 ms | $0.88 |
| `openai/gpt-oss-20b:nitro` | 97% | **100%** | 3.17 | 3.67 | 0 / 2 | 199 ms | **$0.19** |
| `google/gemini-3.5-flash-lite` | 100% | 97% | 4.33 | 3.83 | 0 / 1 | 506 ms | $0.97 |
| `qwen/qwen3.5-35b-a3b:nitro` | 100% | 90% | 4.17 | 4.17 | 0 / 0 | 211 ms | $0.77 |
| `meta-llama/llama-4-scout:nitro` | 97% | 79% | 3.17 | 2.83 | 0 / 2 | **134 ms** | $0.28 |
| `nvidia/nemotron-3.5-lightning` | 100% | 77% | 4.33 | 4.17 | 1 / 1 | 126 ms | $0.22 |
| `moonshotai/kimi-k2.6:nitro` | 100% | 50% | **4.67** | **4.83** | **5** / 0 | 217 ms | $1.88 |

## What this changed

**The default moved from `meta-llama/llama-4-scout:nitro` to
`openai/gpt-oss-120b`.** Scout was strictly dominated: `gpt-oss-20b:nitro` beat
it on price, attention accuracy and prose simultaneously.

The two axes disagree sharply, which is the interesting part:

- `kimi-k2.6` writes by far the best prose — the blind judge picked it best in
  five of six comparisons — but classifies attention at chance (50%), flagging
  ten agents that wanted nothing. For a panel whose entire job is "who needs
  me", crying wolf is worse than saying nothing, and it costs the most.
- `nemotron-3.5-lightning` writes well and is cheap, but **never once**
  detected the one agent that was genuinely blocked (0 true positives across
  five runs). A detector that misses the only real case is not a detector.
- `gpt-oss-120b` is the only model that is both perfectly accurate on attention
  and top-tier on prose. At roughly $0.001 per summary, a heavy session of
  fifty summaries an hour costs about four cents.

Use `--model openai/gpt-oss-20b:nitro` to cut cost 4.6× with the same attention
accuracy, accepting noticeably plainer writing.

## A defect this found

Every model was initially recorded at 0–37% success. That was not the models:
`kimi-k2.6` and `qwen3.5-35b-a3b` are reasoning models that spent the whole
`max_tokens` budget thinking and returned `finish_reason: "length"` with empty
content — **calls that still bill**. One such kimi call cost $0.0034 and
produced nothing.

No single setting fixes this. `reasoning: {enabled: false}` is cheapest and is
the only thing that makes kimi and qwen work, but the OpenAI and Gemini
endpoints reject it outright with *"Reasoning is mandatory for this endpoint"*.
`{effort: "low"}` satisfies those but does not help kimi or qwen.

So `OpenRouter` starts at `ReasoningMode::Disabled` and escalates only when a
provider explicitly refuses, caching the answer for the process lifetime. That
took every model from partial failure to 100% success, and cut kimi's cost
2.4× and qwen's 6.3× against simply raising the budget.
