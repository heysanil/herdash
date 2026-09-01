# herdash — multi-provider LLM backends

**Date:** 2026-08-31
**Status:** approved, ready for implementation planning
**Supersedes:** [`2026-08-30-herdash-design.md`](2026-08-30-herdash-design.md)
§3.5 (Summarization backend), §9 (Configuration), §10.1 (Privacy), and the
"No config file" line in §2 (Non-goals).

## 1. Purpose

herdash summarizes agent transcripts through exactly one backend: OpenRouter,
hardcoded to `https://openrouter.ai/api/v1/chat/completions`, unlocked by a
single `$OPENROUTER_API_KEY`. That forecloses three things people reasonably
want:

- **Their own account.** Someone with an OpenAI or Claude platform key should
  not need a second vendor relationship to use herdash.
- **Local inference.** Transcripts contain source code. Today the only way to
  keep them off the network is `--no-summaries`, which costs the entire feature.
  An LM Studio or Ollama endpoint gives full summaries with zero external
  egress — the strongest possible answer to §10.1's privacy warning.
- **Anything else.** vLLM, LiteLLM, a corporate gateway, a self-hosted proxy.

This design makes the summarizer speak **either the OpenAI v1 Chat Completions
protocol or the Anthropic Messages protocol**, against any base URL, selected
by a `--provider` preset. It adds a config file and a `herdash init` setup flow
so the resulting configuration is stated once rather than re-typed per launch.

## 2. Non-goals

- **Not an abstraction over every LLM API.** Two wire protocols, chosen because
  between them they reach every endpoint named above. Gemini's native protocol,
  Bedrock's, and Vertex's are out of scope; all three are reachable through an
  OpenAI-compatible gateway if someone wants them.
- **No streaming.** Summaries are short, non-interactive, and parsed whole.
- **No multi-provider fallback or routing.** One provider per run. A failed
  call surfaces as today's per-agent `⚠ summary unavailable` with backoff.
- **No re-benchmarking.** `docs/benchmark.md` measured seven models through
  OpenRouter; those numbers stay scoped to OpenRouter routing and are not
  re-run across providers.
- **No credential management beyond a file.** No keychain, no OS secret store.

## 3. Validated background

Every claim in this section was verified against vendor documentation on
2026-08-31 via Context7 and Firecrawl, not inferred from training data. The
traps are recorded because each one is a silent 400 that OpenRouter currently
hides from us.

### 3.1 The two wire protocols

|  | OpenAI v1 | Anthropic Messages |
| --- | --- | --- |
| Path | `{base}/chat/completions` | `{base}/v1/messages` |
| Auth | `Authorization: Bearer <key>` | `x-api-key: <key>` |
| Version header | none | `anthropic-version: 2023-06-01` |
| System prompt | `messages[0].role = "system"` | top-level `system` string |
| Schema | `response_format.json_schema` (`name` + `strict` required) | `output_config.format` (`type` + `schema`, **no** `name`, **no** `strict`) |
| Content | `choices[0].message.content` | first `content[]` block of `type: "text"` |
| Error body | `error.message` | `error.message` (envelope `{"type":"error","error":{…}}`) |
| Token cap | `max_tokens` / `max_completion_tokens` | `max_tokens` (required) |

**Base-URL convention differs by ecosystem, deliberately.** OpenAI-style base
URLs *include* `/v1` (`https://api.openai.com/v1`), Anthropic-style ones
*exclude* it (`https://api.anthropic.com`). herdash follows each vendor's own
convention so a URL copied from their docs works unmodified. This is a
documented behavior, not an inconsistency to fix.

### 3.2 OpenAI direct — two parameters that 400

Both are hidden today because OpenRouter normalizes them.

- **`max_tokens` is rejected** on gpt-5, the o-series, and gpt-4.1:
  `Unsupported parameter: 'max_tokens' is not supported with this model. Use
  'max_completion_tokens' instead.` `max_completion_tokens` is accepted by
  gpt-4o as well, so for the `openai` provider it is correct unconditionally.
  Third-party OpenAI-compatible servers generally know only `max_tokens`, so
  the substitution is scoped to `provider = openai`.
- **`temperature` is rejected** by the o-series at any value but `1`. The
  `openai` provider therefore omits it entirely rather than sending `0.2`.

### 3.3 Anthropic — sampling and thinking

- **`temperature`, `top_p` and `top_k` were removed** on Opus 5, Sonnet 5, and
  Opus 4.7/4.8 — sending any of them is a 400. The Anthropic wire omits
  sampling parameters unconditionally.
- **Thinking configuration is not uniform.** `thinking: {type: "disabled"}` is
  accepted on Opus 5 (at effort ≤ high), Sonnet 5, and Opus 4.7/4.8, but is a
  400 on Fable 5. `output_config.effort` is a 400 on Haiku 4.5 and Sonnet 4.5.
  No single setting works everywhere — the same finding that produced
  `ReasoningMode`, and the reason §4.3 keeps the ladder rather than replacing
  it with a per-provider constant.
- Structured output is GA at `output_config.format`; the older `output_format`
  parameter is deprecated. No beta header is required.

### 3.4 Local servers

- **LM Studio** serves `/v1/chat/completions` on `localhost:1234` and honors
  `response_format.json_schema` (GGUF via grammar sampling, MLX via Outlines).
  Auth is optional. Historically it rejected `response_format.type` values
  other than `json_schema`; herdash either sends `json_schema` (agent summary)
  or omits `response_format` entirely (fleet summary), so it never trips this.
  Models below ~7B are documented as unreliable at structured output.
- **Ollama** serves `/v1/chat/completions` on `localhost:11434` and documents
  `response_format` as supported. Self-hosted Ollama ≥ 0.5.0 plumbs the schema
  into llama.cpp's grammar-constrained decoder and genuinely enforces it.
  **Ollama Cloud accepts the schema and does not enforce it** — acknowledged
  upstream. herdash's strict parse turns that into the existing `model did not
  return the requested schema` error, which is the correct and informative
  outcome; it is documented rather than worked around.
- Neither requires an API key. This is what forces §4.4's change to the
  "no key ⇒ no summaries" invariant.

### 3.5 Model discovery

Every provider in scope exposes a model list — `{base}/models` on the OpenAI
wire, `{base}/v1/models` on the Anthropic wire. `herdash init` uses it for the
searchable picker (§6). For Ollama and LM Studio the same call doubles as a
liveness probe: connection refused means the server is not running, which is
the single most likely local-setup failure.

## 4. Architecture

### 4.1 Provider and wire

```rust
pub enum Wire { OpenAi, Anthropic }

pub struct Provider {
    pub id: ProviderId,
    pub wire: Wire,
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
    pub start_rung: ReasoningMode,
}
```

`ProviderId` is a `clap::ValueEnum`, so `--provider` gets its values and help
text for free. The preset table is the single source of truth:

| `--provider` | wire | default base URL | key | default model | start rung |
| --- | --- | --- | --- | --- | --- |
| `openrouter` *(default)* | OpenAI v1 | `https://openrouter.ai/api/v1` | required | `openai/gpt-oss-120b` | `Disabled` |
| `openai` | OpenAI v1 | `https://api.openai.com/v1` | required | `gpt-5-mini` | `LowEffort` |
| `anthropic` | Anthropic | `https://api.anthropic.com` | required | `claude-haiku-4-5` | `Disabled` |
| `ollama` | OpenAI v1 | `http://localhost:11434/v1` | none | **required** | `ProviderDefault` |
| `lmstudio` | OpenAI v1 | `http://localhost:1234/v1` | none | **required** | `ProviderDefault` |
| `openai-compatible` | OpenAI v1 | **required** | optional | **required** | `ProviderDefault` |
| `anthropic-compatible` | Anthropic | **required** | optional | **required** | `ProviderDefault` |

`--base-url` overrides the default for any provider. Where the table says
**required**, its absence is a startup error naming the missing flag — not a
silent default. `openrouter` keeps every current default, so an existing
install upgrades with no behavior change.

`start_rung` exists because the cheapest rung is not universally reachable:
the `reasoning` field is an OpenRouter extension that OpenAI direct rejects as
an unknown argument, and llama.cpp-backed servers have no reasoning knob at
all, so starting below `ProviderDefault` there only buys a wasted round trip.

### 4.2 Module layout

`src/summary/openrouter.rs` (415 lines, currently prompts + bodies + parsing +
HTTP + escalation) splits along the seam the new wire exposes:

| File | Contents | Purity |
| --- | --- | --- |
| `src/summary/prompts.rs` | `AGENT_SYSTEM`, `FLEET_SYSTEM`, the summary JSON schema, `clamp_transcript` | pure |
| `src/summary/openai.rs` | OpenAI v1 body construction + response parsing | pure |
| `src/summary/anthropic.rs` | Anthropic Messages body construction + response parsing | pure |
| `src/summary/reasoning.rs` | `ReasoningMode`, per-wire rendering, `is_reasoning_rejection` | pure |
| `src/summary/client.rs` | `LlmClient`: reqwest, headers, the escalation loop, `impl Summarizer` | the only I/O |
| `src/summary/provider.rs` | `Provider`, `ProviderId`, `Wire`, the preset table | pure |

This preserves the property that makes the current code testable — bodies and
parsing are free functions callable without a socket — and confines network
access to one file.

**The `Summarizer` trait is untouched**, so `orchestrator.rs` and every
summarizer stub in `tests/` are unaffected — the whole orchestration layer,
which holds the trickiest state in the program, is out of scope for this change.
Two call sites outside `src/summary/` do move:

- `main.rs` — construct `LlmClient::new(provider)` instead of
  `OpenRouter::new(key, model)`, and dispatch the new `init` subcommand (§6)
  before any terminal setup.
- `app.rs` — `SummariesMode` gains the local-provider state described in §4.4.
  This is a display-state change only; the enum stays a plain projection with
  no new behavior attached.

### 4.3 The reasoning ladder, generalized

`ReasoningMode`'s three rungs and its escalate-once-then-cache behavior are
kept exactly. Only rendering becomes wire-aware:

| rung | `openrouter` | `openai` | `anthropic` | local / compatible |
| --- | --- | --- | --- | --- |
| `Disabled` | `reasoning: {enabled: false}` | — | `thinking: {type: "disabled"}` | — |
| `LowEffort` | `reasoning: {effort: "low"}` | `reasoning_effort: "low"` | `output_config: {effort: "low"}` | — |
| `ProviderDefault` | *(field omitted)* | *(field omitted)* | *(field omitted)* | *(field omitted)* |

On the Anthropic wire `output_config` already carries `format`; `effort` merges
into that same object rather than introducing a second one.

`is_reasoning_rejection` widens from OpenRouter's phrasings to also match
OpenAI's `Unrecognized request argument supplied: reasoning_effort` /
`Unsupported parameter: 'reasoning_effort'` and Anthropic's thinking and effort
refusals. The escalation is what makes `claude-haiku-4-5` work as the Anthropic
default despite rejecting `effort` (§3.3): it starts at `Disabled`, which Haiku
accepts, and would self-correct in one cached round trip if it did not.

**This preserves the AGENTS.md rule "reasoning must be negotiated, not
assumed."** Adding providers makes that rule more load-bearing, not less.

### 4.4 The "no key" invariant changes

Today `SummariesMode` is `On` / `OffByFlag` / `OffNoKey`, and a resolved key is
what enables summaries. That is no longer sound: Ollama and LM Studio need no
key, and refusing to summarize against them would be wrong.

The condition becomes **"the provider has what it needs"**: a key was resolved,
*or* the provider requires none. `OffNoKey` is retained for the case that
actually deserves it — a key-requiring provider with no key found — and its
header text names the provider and the env var it looked for, rather than
saying `OPENROUTER_API_KEY` regardless of what is configured.

A fourth header state is added for the case worth advertising:

```
summaries on · ollama (local)
```

shown when the provider is local, which is precisely when transcripts leave no
machine. This is the privacy story §10.1 could not previously tell.

## 5. Configuration

### 5.1 Precedence

**CLI flag → environment variable → config file → built-in default**, resolved
per setting rather than per file, so a config file that sets `model` does not
suppress an env-var `provider`.

### 5.2 Files

Two files under `~/.config/herdash/`, split so that the first is safe to commit
to a dotfiles repository and the second never is:

```toml
# config.toml — mode 0644
[llm]
provider = "anthropic"
model    = "claude-haiku-4-5"
base_url = "https://api.anthropic.com"   # optional

[dashboard]
interval = 1
cooldown = 90
lines    = 200
mouse    = true
theme    = "auto"
```

```toml
# credentials.toml — mode 0600
openrouter = "sk-or-..."
anthropic  = "sk-ant-..."
openai     = "sk-..."
```

Keying credentials by provider lets one machine hold several and switch with
`--provider` alone. `--config <path>` overrides the config file location;
credentials are always read from `credentials.toml` beside it.

Unknown keys are ignored with a startup notice rather than being a hard error —
the same forgiveness rule AGENTS.md already applies to herdr's wire types, for
the same reason: a config written by a newer herdash must not break an older
one. A malformed file *is* an error, reported with the path and the parse
error, before the terminal is taken over.

If `credentials.toml` is more permissive than 0600, herdash warns once at
startup with the path and the `chmod` to run. It does not refuse to start —
the user's key, the user's call.

### 5.3 Key resolution

For provider *P*, first match wins:

1. `$HERDASH_API_KEY` — provider-agnostic, always wins
2. `$OPENROUTER_API_KEY` / `$OPENAI_API_KEY` / `$ANTHROPIC_API_KEY`, per *P*
3. `credentials.toml` → `[P]`
4. `~/.openrouter-key`, for `openrouter` only

Step 4 preserves the existing path so no current install breaks. Empty and
whitespace-only values are treated as unset at every step, matching
`resolve_api_key`'s current behavior.

### 5.4 Flags

| Flag | Default | Purpose |
| --- | --- | --- |
| `--provider <id>` | `openrouter` | Backend preset; sets wire protocol and default base URL |
| `--base-url <url>` | per provider | Override the endpoint |
| `--model <name>` | per provider | Model name or slug |
| `--config <path>` | `~/.config/herdash/config.toml` | Config file location |

`--interval`, `--cooldown`, `--lines`, `--no-summaries`, `--socket`,
`--no-mouse` and `--theme` are unchanged, and all but `--no-summaries` and
`--socket` gain a `[dashboard]` config-file equivalent.

## 6. `herdash init`

An optional clap subcommand. Bare `herdash` still launches the dashboard — the
subcommand is `Option<Command>`, so no existing invocation changes.

### 6.1 Flow

1. **Provider** — searchable picker over the seven presets, each with its
   one-line description.
2. **Base URL** — prompted only when the preset requires one, pre-filled with
   the preset default otherwise.
3. **API key** — masked prompt, skipped entirely when the provider needs none.
   An already-resolvable key is offered as the default so re-running `init` to
   change only the model does not require re-entering it.
4. **Model** — `GET {base}/models`, then a searchable picker over the real
   slugs. On any failure the flow falls back to a free-text prompt, stating
   why (connection refused, 401, or an unparseable body) rather than failing.
   This is where the picker earns itself: OpenRouter lists several hundred.
5. **Verify** — one live agent-summary call against a short canned transcript,
   reporting `ok (412ms)` or the error with a provider-specific hint. Skipped
   by `--no-verify`.
6. **Write** — `config.toml` at 0644 and, if a key was entered,
   `credentials.toml` at 0600. An existing config is shown as a before/after
   summary and overwritten only on confirmation.

### 6.2 Picker

Lives in `src/init/`, **not** `src/ui/`. The existing rule that `src/ui/*` is a
pure projection of `App` state stays intact; the picker is a self-contained
modal loop with its own event handling, sharing only the palette.

Ranking is a pure function:

```rust
pub fn rank(items: &[String], query: &str) -> Vec<Ranked>;
```

Case-insensitive subsequence matching, scored so that earlier and more
contiguous matches win — `clhk` finds `claude-haiku-4-5`, `oll3` finds
`llama3.3`. Being pure and offline, it is unit-tested like `policy.rs`.

Built on the existing ratatui 0.30 and crossterm 0.29 dependencies. **No new
crates.** `toml` and `serde` are already present for the palette reader, so
§5's files cost nothing either.

## 7. install.sh

The installer always ends by printing:

```
Next: run `herdash init` to set up summaries.
```

It auto-runs `herdash init` only when **both** `HERDASH_INIT=1` is set and
`/dev/tty` is readable. The reason is structural: under
`curl -fsSL … | sh` the script *is* stdin, so a prompt reading stdin would
consume installer bytes or see EOF. Requiring an explicit opt-in *and* a real
tty keeps `curl | sh` non-interactive in CI, Docker builds, and provisioning
scripts, where it must never block.

## 8. Documentation

- **`docs/providers.md`** *(new)* — the configuration reference. One section
  per provider: OpenRouter, OpenAI, Claude platform, LM Studio, Ollama, plus a
  closing section on arbitrary compatible endpoints. Each carries a working
  command, the env var it reads, a model suggestion, and its known gotchas
  (§3.2–§3.4).
- **`README.md`** — a compact provider table with one quickstart line each,
  linking to `docs/providers.md`; updated Requirements, flags table, Privacy
  (now able to describe a zero-egress-with-summaries mode), and Troubleshooting
  rows for the new failure modes.
- **`AGENTS.md`** — "OpenRouter gotchas" becomes "LLM provider gotchas",
  carrying §3.2–§3.4 so they are not rediscovered.
- **`docs/benchmark.md`** — a scope note that its numbers describe OpenRouter
  routing and are not claims about other providers.

## 9. Failure modes

| Condition | Behavior |
| --- | --- |
| Provider requires `--base-url`/`--model` and it is absent | Startup error naming the flag, with an example. Exits before touching the terminal. |
| Key-requiring provider, no key found | `summaries off (no key: $ANTHROPIC_API_KEY)` — names the provider's own env var. Dashboard runs. |
| Local endpoint not running | Per-agent `⚠ summary unavailable` with existing backoff; the notice names connection-refused and the base URL. |
| Malformed config file | Startup error with path and parse error. Exits before the terminal. |
| Unknown config key | Ignored; one startup notice. |
| `credentials.toml` looser than 0600 | One startup warning with the `chmod`. Does not block. |
| Model ignores the schema (e.g. Ollama Cloud) | Existing `model did not return the requested schema` error, per agent. |
| Provider rejects a reasoning rung | Escalates one rung, caches, retries. Unchanged. |

## 10. Testing

Every test continues to run without a herdr server and without network access.

- **Per-wire body and parse tests** — extend `tests/summary_parse.rs`; add
  `tests/anthropic_parse.rs` covering the Messages envelope, `output_config`
  placement, `content[]` text extraction, and the error envelope.
- **Escalation tests per wire** — extend `tests/reasoning_escalation.rs` with
  stub servers asserting each wire's rung sequence and rejection phrasings.
- **Config layering** — extend `tests/config.rs`: flag-over-env-over-file
  precedence, the four-step key resolution, preset defaults, required-field
  errors, unknown-key tolerance, malformed-file errors.
- **Picker ranking** — new pure unit tests for `rank`.
- **`init`** — the non-interactive parts (config serialization, file modes,
  overwrite detection) tested directly; the interactive loop is not.

`tests/summary_parse.rs` and `tests/reasoning_escalation.rs` change their
import paths as `openrouter.rs` splits. No test's *intent* changes.

## 11. Compatibility

An existing install with `$OPENROUTER_API_KEY` set and no config file behaves
identically: same provider, same endpoint, same model, same reasoning rung,
same key path including `~/.openrouter-key`.

The break is `--model`, which is now provider-relative — a script passing an
OpenRouter slug while `--provider openai` is configured will fail. Under
AGENTS.md's rule that MINOR absorbs breaking changes below 1.0, this ships as
**0.2.0**.

## 12. Build sequence

The work is three additive layers, each shippable and testable on its own. The
implementation plan should follow this order — later layers depend on earlier
ones, and nothing depends on a later one.

1. **Wire protocols and providers** (§3, §4). Split `openrouter.rs`, add the
   Anthropic wire, generalize the reasoning ladder, add `--provider`,
   `--base-url`, and the preset table. At the end of this layer every provider
   works from flags and environment variables alone. This is the layer that
   satisfies the original request; the rest is ergonomics.
2. **Config file** (§5). `config.toml`, `credentials.toml`, precedence,
   permission warning. Nothing above it changes — this only adds a lower rung
   to the resolution chain.
3. **`herdash init` and the installer** (§6, §7). The picker, the flow, the
   `install.sh` prompt.

Documentation (§8) is written alongside each layer rather than deferred: the
provider reference lands with layer 1, the config reference with layer 2.
