# herdash

A terminal dashboard for [herdr](https://herdr.dev) agent fleets.

`herdash` answers one question at a glance: *what is every one of my agents
doing right now, and which one needs me?* It shows live lifecycle status for
every agent herdr knows about, grouped by repository, and augments each with an
LLM-written summary of that agent's terminal output — the task it is on, what
it is doing now, and what it recently finished.

```
┌─ herdash ─────────────────────────── 6 agents · 2 working · 1 blocked · 3 idle ─┐
│ FLEET  midnights is converging on revenue-categorization; willow work is        │
│        exploratory. One agent is waiting on an approval.                        │
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
 ↑↓ select  ⏎ focus pane  r resummarise  R all  a active-only  ? keys  q quit
```

## Requirements

- A running herdr server (0.8.2 or later). Check with `herdr status server`.
- [mise](https://mise.jdx.dev) — the pinned Rust 1.98 toolchain is required, as
  ratatui 0.30 needs Rust ≥ 1.88 and edition 2024.
- An OpenRouter API key, **optional**: without one herdash runs as a pure
  status board. See [Privacy](#privacy).

## Install

```bash
mise install
mise exec -- cargo install --path .
```

Then run `herdash` in any herdr pane.

## Usage

```bash
herdash                      # full dashboard with summaries
herdash --no-summaries       # status board only, zero external network egress
herdash --cooldown 90        # summarise a given agent at most every 90s
```

| Flag | Default | Purpose |
| --- | --- | --- |
| `--interval <secs>` | `1` | Seconds between herdr snapshot polls |
| `--cooldown <secs>` | `45` | Minimum seconds between summaries for one agent |
| `--model <slug>` | `meta-llama/llama-4-scout:nitro` | OpenRouter model |
| `--lines <n>` | `200` | Transcript lines requested per agent |
| `--no-summaries` | off | Pure status board, no external network egress |
| `--socket <path>` | `$HERDR_SOCKET_PATH`, else `~/.config/herdr/herdr.sock` | herdr socket |

### Keys

| Key | Action |
| --- | --- |
| `↑` `↓` / `k` `j` | Select an agent |
| `g` / `G` | First / last agent |
| `⏎` | Focus that pane in herdr |
| `r` / `R` | Resummarise the selected agent / every agent |
| `a` | Toggle active-only (hide `idle` and `unknown`) |
| `→` / `←` | Open / close detail on narrow terminals |
| `?` | Keybinding help |
| `q` / `Ctrl-C` | Quit |

### Status glyphs

Glyph, colour and sort order all encode one idea — how much does this want
your attention?

| | Status | Meaning |
| --- | --- | --- |
| `⊘` | `blocked` | herdr recognised an approval or question dialog. It needs you now. |
| `◆` | `done` | Finished work you have not seen yet. |
| `●` | `working` | Busy. |
| `○` | `idle` | Ready for input, and you have seen it. |
| `?` | `unknown` | An agent is present but unclassified. This does **not** mean finished. |

Agents sort by that order within a repository, and repositories sort by their
most urgent member — so whatever needs you is always at the top.

Ages are measured from when herdash first saw an agent, because herdr exposes
no timestamp for a status. An age that is only a lower bound is shown with a
`~` prefix (`~1h`) until herdash observes that agent change state.

## Summaries

Two independent clocks keep the dashboard live without being expensive:

- **Status** is polled every second. It is free — no model involved.
- **Summaries** are generated only when an agent's output actually changed,
  with a 45-second per-agent cooldown. Urgent transitions (an agent becoming
  blocked, or finishing) bypass the cooldown, because those are the moments
  you care about.

At five agents under heavy activity this costs roughly a cent an hour.

## Privacy

**Summarisation sends your agents' terminal transcripts — which contain your
source code and file contents — to OpenRouter, and on to whichever provider it
routes to.**

If that is not acceptable, `--no-summaries` gives you the full status board
with zero external network egress, as does simply not configuring a key. The
header tells you which mode you are in.

The API key is read from `$OPENROUTER_API_KEY`, then `~/.openrouter-key`.

## Troubleshooting

| Symptom | Cause and fix |
| --- | --- |
| `could not reach herdr at …` | The server is not running. `herdr status server`, or start it with `herdr server`. herdash exits before touching the terminal, so nothing is left in a broken state. |
| `⟳ reconnecting` in the header | The socket is unreachable. The last known state stays on screen and polling retries automatically. |
| `summaries off (no key)` | No key in `$OPENROUTER_API_KEY` or `~/.openrouter-key`. |
| `summaries off` | You passed `--no-summaries`. |
| `⚠ summary unavailable` on one agent | That agent's summary failed; the previous one stays visible. Retries follow a 5s → 15s → 45s → 5m backoff. `r` forces an immediate retry. |
| `herdr: …` notice in the header | herdr answered but not usefully — a protocol mismatch, say. Polling continues on the same schedule. |

## Development

See [AGENTS.md](AGENTS.md) for repository conventions and the herdr API
gotchas worth knowing before changing the client.

```bash
mise run check   # fmt --check + clippy -D warnings + all tests
```

Every test runs without a herdr server and without network access.

## Licence

MIT. See [LICENSE](LICENSE).
