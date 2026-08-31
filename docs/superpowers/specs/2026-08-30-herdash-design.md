# herdash — design

**Date:** 2026-08-30
**Status:** approved, ready for implementation planning

## 1. Purpose

`herdash` is a terminal dashboard for [herdr](https://herdr.dev), an agent
multiplexer. It runs in a herdr pane and answers one question at a glance:
*what is every one of my agents doing right now, and which one needs me?*

It shows live lifecycle status for every agent herdr knows about, grouped by
repository, and augments each with an LLM-written summary of that agent's
terminal output: the task it is on, what it is doing now, and what it recently
finished.

## 2. Non-goals

- Not a herdr replacement or a general control surface. The only mutation it
  performs is focusing a pane.
- No prompting, no starting or stopping agents, no layout management.
- No persistence. State is in-memory and rebuilt on launch.
- No config file. Defaults plus a handful of CLI flags.

## 3. Validated background

Every claim below was verified against herdr 0.8.2 on 2026-08-30, not inferred.

### 3.1 Transport

herdr's server exposes a Unix socket, path in `$HERDR_SOCKET_PATH`, defaulting
to `~/.config/herdr/herdr.sock`. The protocol is newline-delimited JSON with no
handshake and no auth. Wire protocol version is `20`.

Request: `{"id": "<caller-chosen>", "method": "<name>", "params": {...}}\n`
Success: `{"id": "...", "result": {"type": "<name>", ...}}\n`
Error:   `{"id": "...", "error": {"code": "...", "message": "..."}}\n`

`params` is required even when empty (`{}`). The full schema is obtainable with
`herdr api schema --json` and enumerates 91 methods.

### 3.2 Methods used

| Method | Params | Purpose |
| --- | --- | --- |
| `session.snapshot` | `{}` | Whole-session state: agents, workspaces, tabs, panes |
| `agent.read` | `{target, source, lines, strip_ansi, format}` | Pane transcript text |
| `agent.focus` | `{target}` | Jump herdr's UI to that agent's pane |

`agent.read`'s `source` enum is `visible | recent | recent_unwrapped |
detection` — note the **underscore**; the CLI's `recent-unwrapped` spelling is
rejected on the wire. Its result is nested under `result.read`, with the
transcript in `result.read.text`.

Reads do not mark an agent's tab as seen, so the dashboard is observationally
inert: polling never flips a `done` agent back to `idle`.

### 3.3 Agent shape

`session.snapshot` returns `result.snapshot.agents[]`, each carrying:

`agent` (kind, e.g. `claude`), `agent_status`, `workspace_id`, `tab_id`,
`pane_id`, `terminal_id`, `terminal_title`, `terminal_title_stripped`, `cwd`,
`foreground_cwd`, `focused`, `revision`, `state_change_seq`, `agent_session`.

`agent_status` is one of `idle | working | blocked | done | unknown`.

- `idle` — ready for input, and its tab has been seen in the focused UI.
- `done` — the same underlying idle state, but the work finished unseen.
- `blocked` — herdr recognised an approval or question dialog.
- `unknown` — an agent is present but unclassified. Does *not* imply completion.

`revision` increments as pane content changes and is the change-detection
signal for summarisation. `state_change_seq` increments on lifecycle changes.

### 3.4 Workspace shape

`result.snapshot.workspaces[]` carries `workspace_id`, `label`, `number`,
`focused`, `agent_status`, `tab_count`, `pane_count`, and an **optional**
`worktree` object with `repo_name`, `repo_root`, `checkout_path`,
`is_linked_worktree`. Workspaces without a git checkout omit `worktree`
entirely — the grouping code must handle this.

### 3.5 Summarisation backend

OpenRouter, model `meta-llama/llama-4-scout:nitro`. Verified live:

- `:nitro` is current, and is a superset of `provider.sort = "throughput"`; it
  also makes priority-tier endpoints eligible. Routed to Groq in testing.
- The model supports `response_format: json_schema` with `strict: true`.
  A probe returned a correctly-shaped object on the first attempt.
- The model ignores terminal chrome (status lines, progress bars, token
  counters) without preprocessing. **No regex scrubbing** — it would be
  fragile across agent kinds for no benefit.
- Cost: 193 prompt + 64 completion tokens = $0.000043. A realistic 200-line
  transcript is roughly $0.0002 per summary.

API key resolution order: `$OPENROUTER_API_KEY`, then `~/.openrouter-key`
(trailing newline stripped).

## 4. Architecture

Single binary. Fully async on tokio. The render loop owns all state; workers
own no state and communicate only by message.

```
                    ┌──────────────── main select! loop ────────────────┐
crossterm           │                                                   │
EventStream ───────▶│  App { fleet, summaries, selection, conn_state }   │
                    │                    │                              │
poll task ─────────▶│                    ▼                              │
(1 Hz snapshot)     │              ui::draw(frame)                      │
                    │                                                   │
summary worker ────▶│                                                   │
(mpsc)              └───────────────────────────────────────────────────┘
```

Three inputs are multiplexed with `tokio::select!`:

1. `crossterm::event::EventStream` — keyboard, resize.
2. `mpsc::Receiver<Update>` — `Update::Snapshot`, `Update::Summary`,
   `Update::FleetSummary`, `Update::Connection`, `Update::SummaryFailed`.
3. A redraw tick for age counters and spinner animation.

Rendering is pure: `ui::draw(&App, &mut Frame)` performs no I/O and no
mutation. All network work happens in spawned tasks that send `Update`s.

## 5. Modules

```
src/
  main.rs              terminal init/restore, select! loop, task spawning
  app.rs               App state, key handling, selection, filters
  config.rs            CLI args, key resolution, socket path resolution
  herdr/
    mod.rs
    client.rs          UnixStream, NDJSON framing, request/response
    types.rs           Snapshot, AgentInfo, Workspace, ReadResult, AgentStatus
  fleet.rs             Agent, RepoGroup; merge, group, urgency sort
  summary/
    mod.rs             the Summarizer trait (stubbed in tests)
    types.rs           AgentSummary, FleetSummary, SummaryState
    policy.rs          pure decision function  (primary unit-test target)
    openrouter.rs      one structured-output call
  ui/
    mod.rs             frame layout
    header.rs          counts, fleet summary, connection state
    sidebar.rs         repo groups, agent rows, one-line headlines
    detail.rs          selected agent: identity, TASK, NOW, RECENT
    footer.rs          keybinding hints
    theme.rs           colours, glyphs
tests/
  fixtures/            recorded snapshot.json, agent_read.json
  policy.rs            decision-table tests
  fleet.rs             grouping and sorting tests
  protocol.rs          NDJSON framing tests
```

Each unit answers *what does it do / how do you use it / what does it depend
on* without reading its internals. `ui/*` depends on domain types but never on
`herdr::client`. `summary/policy.rs` depends on nothing but `std` and its own
types, which is what makes the cadence logic exhaustively testable.

## 6. Data model

```rust
enum AgentStatus { Blocked, Done, Working, Idle, Unknown }

struct Agent {
    pane_id: String,          // "w3P:p1" — stable identity
    workspace_id: String,
    kind: String,             // "claude"
    title: String,            // terminal_title_stripped
    label: String,            // workspace label, preferred display name
    repo: Option<String>,     // worktree.repo_name
    cwd: PathBuf,
    status: AgentStatus,
    revision: u64,
    state_change_seq: u64,
    status_since: Instant,    // derived locally, see 6.1
}

struct AgentSummary {
    headline: String,         // <= 60 chars, sidebar one-liner
    task: String,             // the overall objective
    now: String,              // what it is doing at this moment
    recent: Vec<String>,      // recently completed steps
    generated_at: Instant,
    from_revision: u64,
}
```

### 6.1 Derived age

herdr does not expose a timestamp for the current status. `status_since` is
maintained locally: when a poll observes a `state_change_seq` different from
the stored one, `status_since` resets to now. On first sight of an agent it is
set to now, so ages are "since herdash saw it", displayed as `3m`, `1h`.

An agent that already existed when herdash launched has an age that is only a
lower bound, so it renders with a `~` prefix (`~1h`). The prefix is dropped the
first time that agent's `state_change_seq` changes under observation, because
from then on the age is exact.

## 7. Refresh and summarisation policy

Two independent clocks.

### 7.1 Status clock — 1 Hz, free

A task polls `session.snapshot` every second and sends `Update::Snapshot`.
Statuses, ages, grouping, and agent appearance/disappearance update
immediately. No LLM involvement, no cost. A 13 KB response over a local Unix
socket makes this negligible.

### 7.2 Summary clock — event-driven, throttled

`policy::decide` is a pure function evaluated per agent on each snapshot:

```rust
fn decide(agent: &Agent, state: &SummaryState, now: Instant, cfg: &Cfg) -> Decision
enum Decision { Skip, Summarize }
```

Returns `Summarize` when **all** hold:

1. Summaries are enabled (key present, `--no-summaries` absent).
2. No call is already in flight for this agent.
3. `agent.revision != state.from_revision` (output actually moved).
4. Either the cooldown has elapsed (`now - state.generated_at >= 45s`)
   **or** a bypass applies.

Bypasses, which exist because these are the moments that matter:

| Trigger | Rationale |
| --- | --- |
| any status → `Blocked` | it is waiting on you right now |
| `Working` → `Done` or `Working` → `Idle` | it just finished |
| `r` keypress on selection | explicit user request |
| `R` keypress | explicit refresh of all |

A newly appeared agent always summarises once, immediately.

Failures back off exponentially per agent: 5s, 15s, 45s, capped at 5 min. The
backoff is part of `SummaryState` and therefore part of the pure decision.

### 7.3 The call

```
agent.read { target: <pane_id>, source: "recent_unwrapped",
             lines: 200, strip_ansi: true, format: "text" }
```

`recent_unwrapped` joins soft wraps, which is correct for transcripts. The text
is capped at 12 KB (tail-biased — recent output matters more) and sent with a
strict JSON schema requiring `{headline, task, now, recent}`. `headline` is
constrained to 60 characters by prompt and truncated defensively on receipt.

Empty or whitespace-only fields render as `—` rather than blank space.

### 7.4 Fleet summary

One extra call folding every current `headline` + `task` into one or two
sentences for the header. Recomputed when the set of agent summaries changes,
throttled to once per 2 minutes. Skipped entirely when fewer than two agents
have summaries — with one agent the header would just restate the detail pane.

### 7.5 Cost envelope

Five agents under heavy activity: on the order of one cent per hour.

## 8. UI

### 8.1 Layout

```
┌─ herdash ─────────────────────────── 5 agents · 2 working · 1 blocked · 2 idle ─┐
│ FLEET  midnights is converging on revenue-categorization (2 agents, 1 verifying │
│        docs); willow work is exploratory. One agent is waiting on an approval.  │
├────────────────────────────────────┬────────────────────────────────────────────┤
│ midnights ───────────────────── 3  │ feat-revenue-categorization                │
│ ⊘ feat-planetscale-seeding w3R 3m  │ claude · w3P:p1 · midnights · working 5m   │
│   Waiting on approval to write to  │ ~/.herdr/worktrees/midnights/feat-revenue… │
│   the seeded Planetscale branch.   │                                            │
│ ● feat-revenue-categoriz… w3P  5m  │ TASK                                       │
│   Verifying doc-accuracy findings  │ Verifying two documentation-accuracy       │
│   on blankspace.ts.                │ findings from an external review of        │
│ ○ feat-dynamic-reports    w3M  1h  │ blankspace.ts.                             │
│   Finished wiring the aggregation  │                                            │
│   query for team reports.          │ NOW                                        │
│                                    │ Running `git show HEAD:…/blankspace.ts` to │
│ willow ─────────────────────── 1   │ judge the verifier's F7(d) recommendation  │
│ ● explore-willow-vs-midni… w3Q 12m │ rather than apply it blindly.              │
│   Comparing repo organization      │                                            │
│   between willow and midnights.    │ RECENT                                     │
│                                    │ · Confirmed F7(b); already test-guarded    │
│ ungrouped ──────────────────── 1   │   by core.test.ts:631-638                  │
│ ○ herdash                 w3S  2m  │ · Applied the comment-only correction      │
│   Designing a ratatui dashboard.   │ · Dispatched verify-f8 subagent            │
└────────────────────────────────────┴────────────────────────────────────────────┘
 ↑↓ select  ⏎ focus pane  r resummarize  R all  a active-only  ? keys  q quit
```

Sidebar is a fixed 38 columns; the detail pane takes the remainder.

### 8.2 Status glyphs and ordering

Both encode one idea: *how much does this want my attention?*

| Status | Glyph | Colour | Order |
| --- | --- | --- | --- |
| `blocked` | `⊘` | red | 1 |
| `done` | `◆` | green | 2 |
| `working` | `●` | amber | 3 |
| `idle` | `○` | dim | 4 |
| `unknown` | `?` | dark grey | 5 |

Agents sort by this priority, then longest-in-state first, then by label.

Repo groups sort by their most urgent member. Ties break alphabetically,
except that `ungrouped` loses every tie and therefore sorts last among groups
of equal urgency. It does **not** sort last unconditionally: a blocked agent in
an unworktreed workspace still surfaces at the top, because burying the one
agent that needs you would defeat the dashboard's purpose.

### 8.3 Sidebar rows

Two lines per agent: glyph + label (truncated with `…`) + workspace id + age;
then the dim wrapped `headline`. While a summary is generating, the second line
shows an animated `summarising…`. With summaries disabled the second line is
the `terminal_title_stripped`, which keeps the sidebar useful.

### 8.4 Detail pane

Header block: title, then `kind · pane_id · repo · status age`, then the cwd
with `$HOME` abbreviated to `~` and middle-elided if long. Then `TASK`, `NOW`,
`RECENT` as labelled sections. A footer line shows summary freshness
(`summary 12s ago · rev 41`).

### 8.5 Interaction

| Key | Action |
| --- | --- |
| `↑`/`↓`, `k`/`j` | move selection (skips group headers) |
| `g`/`G` | first / last |
| `⏎` | `agent.focus` on the selected pane |
| `r` / `R` | resummarise selected / all |
| `a` | toggle active-only (hide `idle` and `unknown`) |
| `?` | keybinding overlay |
| `q`, `Ctrl-C` | quit |

Selection is keyed on `pane_id`, so it survives re-sorting and list churn. If
the selected agent disappears, selection moves to the nearest surviving row.

### 8.6 Responsive behaviour

Below 100 columns the detail pane is hidden and the sidebar takes the full
width; `→`/`l` opens detail as a full-screen view, `←`/`h` or `Esc` returns.
Below 20 rows the fleet summary block collapses to a single line.

## 9. Configuration

| Flag | Default | Purpose |
| --- | --- | --- |
| `--interval <secs>` | `1` | snapshot poll interval |
| `--cooldown <secs>` | `45` | minimum seconds between summaries per agent |
| `--model <slug>` | `meta-llama/llama-4-scout:nitro` | OpenRouter model |
| `--lines <n>` | `200` | transcript lines requested per read |
| `--no-summaries` | off | pure status board, no network egress |
| `--socket <path>` | `$HERDR_SOCKET_PATH`, else `~/.config/herdr/herdr.sock` | override socket path |

## 10. Failure modes

| Failure | Behaviour |
| --- | --- |
| Socket missing or server down | Message naming the attempted path and suggesting `herdr server`, exit 1 |
| Socket drops while running | Header shows `⟳ reconnecting`, last known state stays rendered, reconnect with backoff to 10s |
| Malformed or unknown JSON from herdr | Unknown fields ignored via serde; a hard parse error is logged to the header once and the poll continues |
| No API key | Full dashboard minus summaries; header notes `summaries off (no key)` |
| OpenRouter 4xx/5xx/timeout | That agent shows `⚠ summary unavailable`, per-agent backoff, app unaffected |
| Model returns unusable content | Strict schema guarantees shape; empty fields render `—` |
| Terminal too small | Degrades per 8.6; never panics |

A panic hook restores the terminal before printing, so a crash never leaves a
broken tty.

### 10.1 Privacy

Summarisation transmits agent terminal transcripts — which contain source code
and file contents — to OpenRouter and its routed provider. This must be stated
in the README. `--no-summaries` and the missing-key path both provide the
dashboard with zero external network egress.

## 11. Testing

Recorded fixtures from a live herdr session let every test run with no herdr
server and no network.

- **protocol** — NDJSON framing across split reads, multiple messages in one
  read, partial trailing lines, error envelopes, unknown fields.
- **fleet** — grouping by `repo_name`, the missing-`worktree` fallback, urgency
  sort, group ordering, selection survival across churn.
- **policy** — the decision table: cooldown not elapsed, revision unchanged,
  call in flight, each bypass, backoff progression, new-agent first summary.
  This is the highest-value suite; the cadence logic is where bugs would hide.
- **summary parsing** — valid payload, empty arrays, missing fields,
  over-length headline truncation.

`openrouter.rs` sits behind a `Summarizer` trait so tests substitute a stub.

## 12. Toolchain

`mise.toml` pins the toolchain, which is **required**, not cosmetic: the rustc
on PATH is 1.84.1, while ratatui 0.30.2 declares `rust-version = "1.88"` and
`edition = "2024"`.

```toml
[tools]
rust = "1.98"

[tasks.run]
run = "cargo run"
[tasks.test]
run = "cargo test"
[tasks.lint]
run = "cargo clippy --all-targets -- -D warnings"
[tasks.fmt]
run = "cargo fmt --all"
```

Dependencies: `ratatui` 0.30, `crossterm` 0.29 (`event-stream`), `tokio` 1.53
(`rt-multi-thread`, `net`, `sync`, `time`, `macros`), `reqwest` 0.13 (`json`, `rustls`,
`rustls-native-certs`), `serde` 1 (`derive`), `serde_json` 1, `anyhow`, `clap` 4
(`derive`).
