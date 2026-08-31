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
  without it, the agents you most want summarised never get a summary.
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

## Design rules

- **`src/summary/policy.rs` must stay pure.** No clocks, no I/O, no async; the
  caller passes `now`. This is what makes the summarisation cadence testable.
- **`src/orchestrator.rs` must stay synchronous and clock-injected.** It holds
  the trickiest state in the program — latched bypasses, forced refreshes
  racing in-flight calls, failure backoff, fleet throttling. It lives in the
  library rather than `main.rs` precisely so it can be tested. Do not move
  logic back into the binary.
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
  unrecognised `agent_status` must degrade to `Unknown`, not error.

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
