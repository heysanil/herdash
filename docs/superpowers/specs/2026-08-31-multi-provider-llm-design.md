# herdash — multi-provider LLM backends

**Date:** 2026-08-31
**Status:** approved, revised after review, ready for implementation planning
**Review:** peer-reviewed 2026-08-31 (Codex, gpt-5.6-sol). Revisions: dialect
replaces wire as the unit of variation (§4.1a, §4.2, §4.3); rung-aware token
budgets (§4.3a); monotonic rung advancement (§4.3); credential/origin binding
(§5.3); loopback-derived privacy badge (§4.4); `Option<T>` settings resolver
moved into layer 1 (§5.1, §12); testing re-ordered by risk (§10).
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

### 4.1 Preset and resolved provider

Two types, deliberately separate. A **preset** is static table data with holes;
a **resolved provider** is what `LlmClient` runs on and has no holes left.
`herdash init` (§6) exists precisely to hold the incomplete middle state, so
one struct cannot serve both.

```rust
pub enum Wire { OpenAi, Anthropic }

/// Static, from the preset table. Fields the user must supply are None.
pub struct ProviderPreset {
    pub id: ProviderId,
    pub wire: Wire,
    pub dialect: Dialect,
    pub default_base_url: Option<&'static str>,
    pub default_model: Option<&'static str>,
    pub key: KeyRequirement,   // Required | None | Optional
}

/// Fully resolved; constructing one validates that nothing is missing.
pub struct ResolvedProvider {
    pub id: ProviderId,
    pub wire: Wire,
    pub dialect: Dialect,
    pub base_url: String,
    pub api_key: Option<String>,
    pub model: String,
}
```

`ProviderId` is a `clap::ValueEnum`, so `--provider` gets its values and help
text for free. The preset table is the single source of truth:

| `--provider` | dialect (wire) | default base URL | key | default model |
| --- | --- | --- | --- | --- |
| `openrouter` *(default)* | `OpenRouter` (OpenAI v1) | `https://openrouter.ai/api/v1` | required | `openai/gpt-oss-120b` |
| `openai` | `OpenAiDirect` (OpenAI v1) | `https://api.openai.com/v1` | required | `gpt-5-mini` |
| `anthropic` | `Anthropic` (Anthropic) | `https://api.anthropic.com` | required | `claude-haiku-4-5` |
| `ollama` | `OpenAiGeneric` (OpenAI v1) | `http://localhost:11434/v1` | none | **required** |
| `lmstudio` | `OpenAiGeneric` (OpenAI v1) | `http://localhost:1234/v1` | none | **required** |
| `openai-compatible` | `OpenAiGeneric` (OpenAI v1) | **required** | optional | **required** |
| `anthropic-compatible` | `Anthropic` (Anthropic) | **required** | optional | **required** |

`--base-url` overrides the default for any provider. Where the table says
**required**, its absence is a startup error naming the missing flag — not a
silent default. `openrouter` keeps every current default, so an existing
install upgrades with no behavior change.

### 4.1a Dialect, not wire, is the unit of variation

Four dialects over two wires. The distinction matters because **three of the
four OpenAI-wire dialects disagree with each other** on things the wire itself
does not specify:

| | `OpenRouter` | `OpenAiDirect` | `OpenAiGeneric` | `Anthropic` |
| --- | --- | --- | --- | --- |
| Wire | OpenAI v1 | OpenAI v1 | OpenAI v1 | Anthropic |
| Token-cap field | `max_tokens` | `max_completion_tokens` | `max_tokens` | `max_tokens` |
| Sends `temperature` | yes (`0.2`) | no | yes (`0.2`) | no |
| Reasoning syntax | `reasoning` object | `reasoning_effort` | *(none)* | `thinking` / `output_config.effort` |
| Start rung | `Disabled` | `LowEffort` | `ProviderDefault` | `Disabled` |

So a wire-keyed abstraction cannot express the request. **`Dialect` is the
parameter; `Wire` only selects which codec consumes it.** Each codec
(`openai.rs`, `anthropic.rs`) takes the dialect and renders accordingly; there
is no separate reasoning module, because reasoning syntax is a dialect
property like the other three rows.

The start rung is a dialect property for the same reason the others are: the
`reasoning` object is an OpenRouter extension that OpenAI direct rejects as an
unknown argument, and llama.cpp-backed servers have no reasoning knob at all,
so starting below `ProviderDefault` there only buys a wasted round trip.

### 4.2 Module layout

`src/summary/openrouter.rs` (415 lines, currently prompts + bodies + parsing +
HTTP + escalation) splits along the seam the new wire exposes:

| File | Contents | Purity |
| --- | --- | --- |
| `src/summary/prompts.rs` | `AGENT_SYSTEM`, `FLEET_SYSTEM`, the summary JSON schema, `clamp_transcript` | pure |
| `src/summary/openai.rs` | OpenAI v1 codec: body construction + parsing, dialect-driven | pure |
| `src/summary/anthropic.rs` | Anthropic Messages codec: body construction + parsing | pure |
| `src/summary/client.rs` | `LlmClient`: reqwest, headers, the escalation loop, `impl Summarizer` | the only I/O |
| `src/summary/provider.rs` | `ProviderPreset`, `ResolvedProvider`, `ProviderId`, `Wire`, `Dialect`, `ReasoningMode`, the preset table | pure |

Five files, not six. There is **no `reasoning.rs`**: per §4.1a, reasoning
syntax is one row of the dialect table alongside the token-cap field and the
temperature policy, so splitting it out would isolate one column of a
four-column decision and force both codecs to reach back into it.
`ReasoningMode` and its rung sequence live with `Dialect` in `provider.rs`;
each codec renders the rung it is handed.

This preserves the property that makes the current code testable — bodies and
parsing are free functions callable without a socket — and confines network
access to one file.

**The `Summarizer` trait is untouched**, so `orchestrator.rs` and every
summarizer stub in `tests/` are unaffected — the whole orchestration layer,
which holds the trickiest state in the program, is out of scope for this change.
Two call sites outside `src/summary/` do move:

- `main.rs` — construct `LlmClient::new(resolved)` instead of
  `OpenRouter::new(key, model)`, and dispatch the `init` subcommand (§6)
  **immediately after clap parsing** — before provider resolution, before the
  `client.snapshot()` reachability probe, and before terminal setup. `init`
  must run when no provider is configured yet (that is its whole purpose) and
  must not require a running herdr server, so any of those three steps ahead
  of it would make it fail exactly when it is needed.
- `app.rs` — `SummariesMode` gains the local-provider state described in §4.4.
  This is a display-state change only; the enum stays a plain projection with
  no new behavior attached.

### 4.3 The reasoning ladder, generalized

`ReasoningMode`'s three rungs and its escalate-on-rejection-then-cache behavior
are kept. Rendering becomes **dialect**-driven (§4.1a), not wire-driven:

| rung | `OpenRouter` | `OpenAiDirect` | `Anthropic` | `OpenAiGeneric` |
| --- | --- | --- | --- | --- |
| `Disabled` | `reasoning: {enabled: false}` | — | `thinking: {type: "disabled"}` | — |
| `LowEffort` | `reasoning: {effort: "low"}` | `reasoning_effort: "low"` | `output_config: {effort: "low"}` | — |
| `ProviderDefault` | *(field omitted)* | *(field omitted)* | *(field omitted)* | *(field omitted)* |

On the Anthropic wire `output_config` already carries `format`; `effort` merges
into that same object rather than introducing a second one.

**Advancement must be monotonic.** Up to `MAX_SUMMARY_TASKS` (6) workers share
one cached rung. The current `set_reasoning_mode` uses a plain atomic `store`,
so a slow request that started at rung 0, was rejected, and returns *after*
another worker already reached `ProviderDefault` will write `LowEffort` back
over it — and the ladder oscillates, paying for a rejected call on every lap.
Advancement therefore uses `fetch_max` (or a CAS loop), never `store`. **This
is a latent bug in the shipped single-provider code**, not one this design
introduces; it is fixed here because more dialects make it easier to hit.

**Rejection detection is classified, not string-matched alone.** A retry only
happens when *all three* hold: the response is a 4xx, the current rung actually
sent a reasoning field, and the error body names that field. Vendors expose no
machine-readable code for this, so the field-name check stays textual — but
gating it on status and rung is what stops an unrelated 400 (a bad model name,
a malformed schema) from being silently retried two more times at cost and
then reported as a reasoning failure. A rejection at the top rung is returned
as-is.

Worst case is **two** extra round trips, not one, since a dialect can be
refused at two successive rungs; both are cached for the process lifetime. The
ladder is what lets `claude-haiku-4-5` be the Anthropic default despite
rejecting `output_config.effort` (§3.3) — it starts at `Disabled`, which Haiku
accepts, so in practice it costs nothing.

**This preserves the AGENTS.md rule "reasoning must be negotiated, not
assumed."** Adding providers makes that rule more load-bearing, not less.

### 4.3a Token budgets must follow the rung

`max_tokens: 900` (agent) and `200` (fleet) were tuned under
`reasoning: {enabled: false}`. **Reasoning tokens are counted against that same
output budget**, so a rung that enables reasoning can consume the whole cap and
return empty content with `finish_reason: "length"` — a call you still pay for.
That is exactly the kimi-k2.6 failure the `ReasoningMode` doc comment already
describes, and the `openai` dialect *starts* at `LowEffort`, so the design as
first written would have reintroduced it by default. The 200-token fleet cap is
the most exposed.

Two consequences:

- **Caps are a function of dialect and rung.** At `Disabled` /
  `ProviderDefault`-with-no-reasoning the current 900/200 stand. At any rung
  that enables reasoning, the cap is raised to leave room for both the hidden
  reasoning and the visible answer.
- **`finish_reason` / `stop_reason` is inspected before parsing.** A length
  stop, a refusal, or an empty content block is reported as itself — not
  funnelled into `model did not return the requested schema`, which today
  misattributes a truncation to the model ignoring the schema and sends the
  reader looking in the wrong place.

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

This is the privacy story §10.1 could not previously tell — but it is a claim
about egress, so it must be **derived from the resolved base URL, never from
the preset name**. `--provider ollama --base-url http://192.168.1.50:11434/v1`
is a remote endpoint wearing a local preset's name; badging it `(local)` would
tell the user their source code stayed on the machine when it did not.

The badge is shown only when the resolved URL's host is loopback —
`127.0.0.0/8`, `::1`, or `localhost` — after parsing, not by string match on
the configured value. Anything else, including a LAN address, gets the plain
`summaries on · <provider>`.

## 5. Configuration

### 5.1 Precedence

Resolved **per setting**, not per file, so a config file that sets `model` does
not suppress a flag-supplied `provider`. Two chains, because an environment
layer only exists where a vendor convention already does:

- **Credentials** — `HERDASH_API_KEY` → `{PROVIDER}_API_KEY` → `credentials.toml`
  → `~/.openrouter-key` (§5.3).
- **Everything else** — CLI flag → `config.toml` → built-in default. No env
  vars are invented for `interval`, `cooldown`, `model`, `provider` or the rest;
  inventing a `HERDASH_*` variable per flag is surface with no demand behind it.

**This forces a change to the `Cli` struct, and it lands in layer 1, not
layer 2.** Every layered field currently uses `default_value_t`
(`config.rs:20` onward), which makes the parsed value indistinguishable from a
supplied one — `--interval 1` and no flag at all both arrive as `1`, so a
config file could never be correctly overridden or correctly deferred to. Those
fields become `Option<T>` with the default applied during resolution instead.
The resolver is therefore built in layer 1 even though no config file reads it
until layer 2; retrofitting it later would mean rewriting layer 1's settings
plumbing rather than adding to it.

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

Writes create the file with 0600 **at creation**, not `chmod` after the fact,
and go to a temporary file in the same directory followed by a rename, so an
interrupted `init` cannot leave a truncated or world-readable credentials file.
A symlink already at either path is an error rather than something to follow.
Rewriting one provider's key preserves the others.

### 5.3 Key resolution

For provider *P*, first match wins:

1. `$HERDASH_API_KEY` — provider-agnostic, always wins
2. `$OPENROUTER_API_KEY` / `$OPENAI_API_KEY` / `$ANTHROPIC_API_KEY`, per *P*
3. `credentials.toml` → `[P]`
4. `~/.openrouter-key`, for `openrouter` only

Step 4 preserves the existing path so no current install breaks. Empty and
whitespace-only values are treated as unset at every step, matching
`resolve_api_key`'s current behavior.

**Stored credentials are bound to the preset's own origin.** Steps 2–4 read a
*vendor* key — `sk-ant-…`, `sk-…` — and `--base-url` can point a vendor preset
anywhere. Without a rule, `--provider openai --base-url http://elsewhere/v1`
would silently send a real OpenAI key to an unrelated host, turning a config
convenience into credential exfiltration.

So steps 2–4 apply **only when the resolved base URL's origin matches the
preset's default origin**. When `--base-url` moves a key-requiring preset off
its own origin, herdash uses `HERDASH_API_KEY` if set and otherwise starts with
no key, reporting which credential it declined to attach and why. `HERDASH_API_KEY`
is exempt because naming it is an explicit act by the user for the run at hand;
`credentials.toml` is not. The `*-compatible` presets have no default origin and
so never auto-attach a vendor key at all.

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
| Provider rejects a reasoning rung | Escalates one rung, caches monotonically, retries. At the top rung the error is returned as-is. |
| 4xx that is *not* a reasoning rejection | Returned unretried, verbatim (§4.3). Never silently escalated. |
| Response truncated (`finish_reason: "length"`) or refused | Reported as itself, not as a schema failure (§4.3a). |
| `--base-url` moves a vendor preset off its own origin | Stored key is **not** sent; notice names the credential declined and points at `HERDASH_API_KEY` (§5.3). |
| Non-loopback endpoint under a local preset | No `(local)` badge (§4.4). |
| `credentials.toml` is a symlink, or write is interrupted | Written via create-new-then-rename with 0600 set at creation; an existing symlink at that path is an error, not followed. |

## 10. Testing

Every test continues to run without a herdr server and without network access.

Ordered by risk, because the parts most likely to be wrong are the ones with
concurrency, credentials, or URL handling in them — not the pure formatters.

- **Concurrent rung advancement** — six workers, staggered rejections, asserting
  the cached rung never decreases and the sequence never oscillates (§4.3).
  This is the finding most likely to survive review and still ship broken.
- **Credential binding and file handling** — a stored vendor key is *not* sent
  when `--base-url` leaves the preset's origin (§5.3); `credentials.toml` is
  created 0600 by `open`, not chmod-after-write; an existing symlink is
  refused; a rewrite preserves unrelated providers' keys; keys never appear in
  an error, notice, or `Debug` output.
- **Endpoint construction** — path joining with and without a trailing slash on
  the base URL, per wire; the `/v1` presence/absence convention (§3.1); the
  loopback determination behind the `(local)` badge, including a LAN address
  and a hostname that resolves nowhere.
- **Request shape per dialect** — headers (bearer vs `x-api-key` +
  `anthropic-version`), token-cap field name, temperature presence, and rung
  rendering, across all four dialects. Extends `tests/summary_parse.rs`; adds
  `tests/anthropic_parse.rs` for the Messages envelope, `output_config`
  placement, `content[]` extraction, and the error envelope.
- **Escalation semantics** — per dialect, against stub servers: the rung
  sequence; a non-reasoning 4xx returned unretried; a top-rung rejection
  returned as-is; a `finish_reason: "length"` reported as truncation rather
  than a schema failure (§4.3a).
- **Config layering** — extend `tests/config.rs`: flag-over-file precedence
  including the `Option<T>` distinction between "flag set to the default value"
  and "flag absent" (§5.1), key resolution order, preset defaults,
  required-field errors, unknown-key tolerance, malformed-file errors.
- **`init` as an injected state machine** — cancel at each step, model-discovery
  failure falling back to free text, verification failure, overwrite refused.
  The crossterm loop stays thin enough not to need testing; the decisions do.
- **Picker ranking** — pure unit tests for `rank`. Cheap and worth keeping, but
  the lowest-value item here; it is a formatter, not a risk.

`tests/summary_parse.rs` and `tests/reasoning_escalation.rs` change their
import paths as `openrouter.rs` splits. No existing test's *intent* changes.

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

0. **Monotonic rung advancement** (§4.3). `fetch_max` instead of `store`, plus
   the concurrency test. Independent of everything else, fixes a latent bug in
   the current single-provider code, and is worth landing on its own so it is
   not entangled with the refactor that follows.
1. **Dialects and providers** (§3, §4). Split `openrouter.rs` into the five
   files, add the Anthropic codec, make rendering dialect-driven, add rung-aware
   token budgets and `finish_reason` inspection (§4.3a), and add `--provider` /
   `--base-url` / the preset table. **Includes the `Option<T>` settings
   resolver** (§5.1) even though nothing reads a config file yet — deferring it
   would mean rewriting this layer's plumbing in layer 2 rather than extending
   it. At the end of this layer every provider works from flags and environment
   variables alone. This is the layer that satisfies the original request; the
   rest is ergonomics.
2. **Config file** (§5). `config.toml`, `credentials.toml`, origin binding,
   permission warning, atomic 0600 writes. Adds one lower rung to the
   resolution chain built in layer 1; nothing above it changes.
3. **`herdash init` and the installer** (§6, §7). The picker, the flow, the
   `install.sh` prompt.

Documentation (§8) is written alongside each layer rather than deferred: the
provider reference lands with layer 1, the config reference with layer 2.
