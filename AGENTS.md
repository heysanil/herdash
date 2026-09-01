# AGENTS.md

Conventions for coding agents working in this repository.

## What this is

`herdash` is a Rust/ratatui terminal dashboard over [herdr](https://herdr.dev),
an agent multiplexer. It reads herdr's Unix-socket API and renders every
agent's status grouped by repository, augmented with LLM-written summaries of
their terminal output.

Read `docs/superpowers/specs/2026-08-30-herdash-design.md` for the design.

## Toolchain

**Always use mise.** The system `rustc` is 1.84 and cannot build this crate —
ratatui 0.30 requires Rust ≥ 1.88 and edition 2024.

```bash
mise install          # once
mise run test         # cargo test --all-targets
mise run lint         # clippy, warnings denied
mise run fmt
mise run check        # fmt --check + lint + test
```

Never run bare `cargo` — prefix with `mise exec --`, or use the tasks above.

## herdr API gotchas

These cost real debugging time. Do not rediscover them.

- **One request per connection.** herdr answers a single request and then
  closes the socket. A second request on the same stream fails with `EPIPE`,
  and pipelined requests get one response followed by EOF. `Client` therefore
  opens a fresh connection per call. Do not "optimise" this into a pooled or
  long-lived connection; it will appear to work against a lenient test stub
  and die instantly against the real server.
  (`events.subscribe` is the exception — it turns the connection into an event
  stream — but per-pane subscriptions churn as panes come and go, so herdash
  polls instead.)
- **`agent.read` refuses oversized reads on a *working* agent.** A working
  agent renders on the terminal's alternate screen, which has no scrollback,
  so herdr returns `agent_not_idle` rather than truncating. Measured on a
  55-row pane: `lines = 60` fails, `lines = 40` succeeds, and `visible` always
  works. `Client::read_agent` falls back to `visible` for exactly this reason —
  without it, the agents you most want summarized never get a summary.
- **`agent.read` always reports `revision: 0`.** The field exists on the read
  payload but carries no information, so it is useless as a change signal.
  Change detection uses the *snapshot* revision, captured when the job is
  dispatched. Recording the read's value would make every agent look
  permanently changed and defeat the "only summarize when output moved" rule.
- **`source` uses underscores on the wire**: `visible | recent |
  recent_unwrapped | detection`. The CLI's hyphenated `recent-unwrapped` is
  rejected.
- **Results nest**: `result.snapshot` for `session.snapshot`,
  `result.read.text` for `agent.read`. The read discriminator is `pane_read`.
- **Nullable strings.** `agent`, `cwd`, `terminal_title_stripped`,
  `terminal_title`, `label` and `foreground_cwd` are all `["string", "null"]`
  in the schema. `#[serde(default)]` covers an *absent* key but **not** an
  explicit `null`, which is a hard error. Use the `null_to_default`
  deserializer in `src/herdr/types.rs`.
- **Workspaces may omit `worktree` entirely.** Never index it unconditionally.
- **Reading does not mark a tab as seen**, so polling never flips a `done`
  agent back to `idle`.
- `params` is required on every request, even when empty (`{}`).
- Inspect the live API with `herdr api schema --json` and `herdr api snapshot`.

## LLM provider gotchas

- **Reasoning must be negotiated, not assumed.** Summarization is extraction,
  so chain-of-thought buys nothing and costs a lot. But no single setting works
  everywhere: `reasoning: {enabled: false}` is cheapest and is the only thing
  that makes kimi-k2.6 and qwen3.5-35b return content at all (otherwise they
  spend the whole `max_tokens` budget thinking and return `finish_reason:
  "length"` with an empty body — a call you still pay for), while the OpenAI
  and Gemini endpoints reject it with "Reasoning is mandatory for this
  endpoint". `ReasoningMode` starts at `Disabled` and escalates only on an
  explicit refusal, caching the result. Do not hardcode one mode.
- **Model choice is measured, not assumed.** `docs/benchmark.md` scores seven
  OpenRouter-routed models on cost, latency, prose quality and attention
  accuracy. Re-run `examples/bench.rs` before changing the default.
- **OpenAI direct rejects `max_tokens`.** gpt-5, the o-series and gpt-4.1 all
  return "Unsupported parameter" for it; herdash sends `max_completion_tokens`
  unconditionally for that dialect. The o-series also rejects any
  `temperature` but `1`, so `Dialect::OpenAiDirect` omits the field rather than
  pin it.
- **Sampling parameters are gone on current Claude models.** `temperature`,
  `top_p` and `top_k` were removed, not deprecated — sending one is a 400. The
  Anthropic codec never sends them.
- **The Anthropic wire has its own auth and versioning.** Requests carry
  `x-api-key` instead of a bearer token, and `anthropic-version` is required
  on every call, not just on first use. Structured output rides
  `output_config.format`, which is GA — not a beta header. Reasoning support
  under it is inconsistent, though: `thinking: {type: "disabled"}` is accepted
  on Opus 5 / Sonnet 5 / Opus 4.7-4.8 but rejected on Fable 5, while
  `output_config.effort` is rejected on Haiku 4.5 and Sonnet 4.5. The ladder
  exists to negotiate between the two rather than assume either.
- **Ollama Cloud accepts `json_schema` without enforcing it.** Self-hosted
  Ollama 0.5.0+ makes the schema a hard constraint via llama.cpp's
  grammar-constrained decoder; the cloud service parses the same field and
  silently ignores it, so a cloud model can return prose that fails
  `parse_agent_response` with "model did not return the requested schema".
  This only reproduces against Ollama Cloud — a local pull will not catch it.
- **Dialect, not wire, is the unit of variation.** `OpenRouter`, `OpenAiDirect`
  and `OpenAiGeneric` all speak `Wire::OpenAi`, yet disagree on the token-cap
  field, whether `temperature` is accepted, and how reasoning is expressed —
  none of which the wire itself specifies. Add a new OpenAI-wire vendor by
  extending `Dialect`, never by branching on `Wire`.
- **Rung advancement must use `fetch_max`, never `store`.** Up to
  `MAX_SUMMARY_TASKS` (six) workers share one cached rung behind an atomic. A
  worker that started at a low rung, was refused, and returns after another
  worker already escalated past it must not overwrite that progress — a plain
  `store` lets a stale rejection lower the rung and the ladder oscillates,
  re-paying for a refused call on every lap.
- **Token budgets must follow the rung.** Reasoning tokens are billed against
  the same output cap as the answer, so a budget sized for
  reasoning-disabled gets consumed entirely by thinking the moment a dialect
  starts above `Disabled` — the empty-body-with-`finish_reason: "length"`
  failure this whole ladder exists to avoid. `agent_max_tokens` and
  `fleet_max_tokens` take `reasons: bool` for exactly this reason; do not pass
  a fixed cap.

## Design rules

- **`src/summary/policy.rs` must stay pure.** No clocks, no I/O, no async; the
  caller passes `now`. This is what makes the summarization cadence testable.
- **`src/orchestrator.rs` must stay synchronous and clock-injected.** It holds
  the trickiest state in the program — latched bypasses, forced refreshes
  racing in-flight calls, failure backoff, fleet throttling. It lives in the
  library rather than `main.rs` precisely so it can be tested. Do not move
  logic back into the binary.
- **Colors come from `src/ui/palette.rs`, never hardcoded.** The defaults are
  ANSI-*named* on purpose: `Color::Red` lets a themed terminal render its own
  red, where `Color::Rgb(255, 0, 0)` would override the user's palette with a
  color from nowhere. Body text is `Color::Reset` and no background is painted,
  which is what makes herdash look native beside herdr. herdr publishes no
  palette over its API and computes its built-in themes in code, so the only
  exact bridge is its `[theme.custom]` tokens, which the palette reads.
- **`src/ui/*` must stay pure.** Rendering is a projection of `App` state; no
  network, no mutation. The in-flight spinner animates off `App::tick`, which
  the event loop increments, rather than reading a clock during render.
- **Selection is keyed on `pane_id`**, never an index — the list re-sorts on
  every poll. When a selected agent disappears, fall back by searching the
  *previous ordering* outward for a survivor; index reuse picks the wrong
  agent when the poll also inserted or reordered rows.
- **Status transitions are latched, not consumed on sight.** An urgent edge
  (`→ blocked`, `working → done/idle`) that lands during an in-flight call or
  an active backoff must still force a summary afterwards.
- **Never deny unknown JSON fields.** herdr adds fields between releases; an
  unrecognized `agent_status` must degrade to `Unknown`, not error.

## Testing

Every test runs without herdr and without network access — recorded-shape
fixtures in `tests/fixtures/`, in-memory duplex streams, and temp Unix
sockets. Keep it that way. `Summarizer` is a trait so tests can stub it.

The stub server in `tests/e2e_socket.rs` deliberately closes after one
response and rejects oversized reads on `busy:p1`, mirroring real herdr. A
more forgiving stub would hide the two bugs that actually shipped.

Fixtures must be shapes herdr could really produce; `the_fixture_matches_the_real_wire_shape`
guards this. Prefer `base()` (an instant an hour ahead) over `Instant::now()`
in tests that subtract durations, so they cannot underflow on a freshly
booted machine.

Run `mise run check` before committing.

## Commits

Conventional Commits. Do not add AI co-author trailers or "generated by" lines
to commit messages or PR descriptions.

The prefixes are load-bearing, not decoration: `mise run release` groups
`CHANGELOG.md` entries from them, so a change committed as `chore:` when it was
really a `fix:` disappears from the release notes.

## Releasing

**Read [docs/releasing.md](docs/releasing.md) before cutting a release.** It
carries the semver decision table, the procedure, and what to do when a release
goes wrong. The short version:

```bash
mise run release 0.2.0 -- --dry-run   # preview the changelog entry
mise run release 0.2.0                # commit + tag, does NOT push
git push origin main && git push origin v0.2.0
```

Rules that are easy to get wrong:

- **Releases are tag-driven.** Pushing `vX.Y.Z` is the only thing that
  publishes. The workflow refuses to build unless the tag and `Cargo.toml`
  agree, so never hand-edit one without the other — use the script.
- **The version is below 1.0, so MINOR absorbs breaking changes** (`0.1.x` →
  `0.2.0`) and PATCH takes everything else. Do not reach for `1.0.0` to signal
  that a change is significant; 1.0 is a promise of interface stability.
- **Never move a published tag.** Anyone who already installed that version
  would silently get different bytes. Ship a patch release instead.
- **A release is not done when the workflow goes green.** Install the published
  artifact and confirm it reports the version you tagged; the verification
  command is in the doc.

## CI

Workflows run on [Namespace](https://namespace.so) runners, labelled
`nscloud-{os}-{arch}-{shape}`. `actionlint` cannot know custom labels, so the
permitted set is declared in `.github/actionlint.yaml` — add a label there
before using it, or lint fails. Check workflows and shell with:

```bash
mise run lint-ci
```

CI runs `mise run check`, the same task used locally, so the two cannot drift.
