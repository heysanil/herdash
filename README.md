# herdash

A terminal dashboard for [herdr](https://herdr.dev) agent fleets.

`herdash` answers one question at a glance: *what is every one of my agents
doing right now, and which one needs me?* It shows live lifecycle status for
every agent herdr knows about, grouped by repository, and augments each with an
LLM-written summary of that agent's terminal output — the task it is on, what
it is doing now, and what it recently finished.

```
┌─ herdash ─────────────────────────── 6 agents · 2 working · 1 blocked · 3 idle ─┐
│ FLEET  acme-core is converging on invoice-classification; widget work is        │
│        exploratory. One agent is waiting on an approval.                        │
├────────────────────────────────────┬────────────────────────────────────────────┤
│ acme-core ───────────────────── 3  │ feat-invoice-classification                │
│ ⊘ feat-isolated-db-seeding w3R 3m  │ claude · w3P:p1 · acme-core · working 5m   │
│   Waiting on approval to write to  │ ~/.herdr/worktrees/acme-core/feat-invoice… │
│   the seeded database branch.      │                                            │
│ ● feat-invoice-classific… w3P  5m  │ TASK                                       │
│   Verifying doc-accuracy findings  │ Verifying two documentation-accuracy       │
│   on normalizer.ts.                │ findings from an external review of        │
│ ○ feat-modular-reports    w3M  1h  │ normalizer.ts.                             │
│   Finished wiring the aggregation  │                                            │
│   query for team reports.          │ NOW                                        │
│                                    │ Running `git show HEAD:…/normalizer.ts` to │
│ widget ─────────────────────── 1   │ judge the verifier's F7(d) recommendation  │
│ ● explore-widget-vs-acme-… w3Q 12m │ rather than apply it blindly.              │
│   Comparing repo organization      │                                            │
│   between widget and acme-core.    │ RECENT                                     │
│                                    │ · Confirmed F7(b); already test-guarded    │
│ ungrouped ──────────────────── 1   │   by core.test.ts:631-638                  │
│ ○ herdash                 w3S  2m  │ · Applied the comment-only correction      │
│   Designing a ratatui dashboard.   │ · Dispatched verify-f8 subagent            │
└────────────────────────────────────┴────────────────────────────────────────────┘
 ↑↓ select  ⏎ focus pane  r resummarize  R all  a active-only  ? keys  q quit
```

## Requirements

- A running herdr server (0.8.2 or later). Check with `herdr status server`.
- [mise](https://mise.jdx.dev). The crate needs Rust ≥ 1.88 and edition 2024
  (ratatui 0.30's floor); `mise.toml` pins 1.98, which is what CI and the
  contributor workflow assume.
- An OpenRouter API key, **optional**: without one herdash runs as a pure
  status board. See [Privacy](#privacy).

## Install

```sh
curl -fsSL https://raw.githubusercontent.com/heysanil/herdash/main/install.sh | sh
```

Installs the latest release binary into `~/.local/bin`, verifying it against
the release's `SHA256SUMS` first. Set `HERDASH_VERSION` to pin a version and
`HERDASH_INSTALL_DIR` to install elsewhere:

```sh
curl -fsSL https://raw.githubusercontent.com/heysanil/herdash/main/install.sh \
  | HERDASH_VERSION=v0.1.0 HERDASH_INSTALL_DIR=/usr/local/bin sh
```

Builds are published for macOS (Apple Silicon and Intel) and Linux (x86_64 and
aarch64). To build from source instead:

```bash
mise install
mise exec -- cargo install --path .
```

Then run `herdash` in any herdr pane.

## Usage

```bash
herdash                      # full dashboard with summaries
herdash --no-summaries       # status board only, zero external network egress
herdash --cooldown 90        # summarize a given agent at most every 90s
```

| Flag | Default | Purpose |
| --- | --- | --- |
| `--interval <secs>` | `1` | Seconds between herdr snapshot polls |
| `--cooldown <secs>` | `45` | Minimum seconds between summaries for one agent |
| `--model <slug>` | `openai/gpt-oss-120b` | OpenRouter model — see [benchmark](docs/benchmark.md) |
| `--lines <n>` | `200` | Transcript lines requested per agent |
| `--no-summaries` | off | Pure status board, no external network egress |
| `--socket <path>` | `$HERDR_SOCKET_PATH`, else `~/.config/herdr/herdr.sock` | herdr socket |
| `--no-mouse` | off | Disable mouse capture, restoring your terminal's own text selection |
| `--theme <auto\|ansi>` | `auto` | `auto` picks up `[theme.custom]` tokens from herdr's config; `ansi` uses terminal colors only |

### Keys

| Key | Action |
| --- | --- |
| `↑` `↓` / `k` `j` | Select an agent |
| `g` / `G` | First / last agent |
| `⏎` | Focus that pane in herdr |
| `r` / `R` | Resummarize the selected agent / every agent |
| `a` | Toggle active-only (hide `idle` and `unknown`) |
| `→` / `←` | Open / close detail on narrow terminals |
| `?` | Keybinding help |
| `q` / `Ctrl-C` | Quit |

Mouse works too: click a row to select it, click the selected row to focus that
pane in herdr, and scroll to move the selection. `--no-mouse` turns capture off
if you would rather keep your terminal's own text selection.

### Showing herdash in herdr's sidebar

herdash names its own herdr space, so it is identifiable in the sidebar with
**no configuration**:

```
 ▸ acme-core        main ✓
 ▸ herdash                        ← the space herdash is running in
 ▸ widget           feat/x ±
```

The previous name is restored when herdash exits. If it is killed rather than
closed, the next run notices the stale rename and puts the old name back before
claiming it again — the original is recorded in
`~/.local/state/herdash/spaces.json`, not inferred. If you rename the space
yourself while herdash is running, your name wins and is left alone.

```sh
herdash --space-name "fleet"   # use a different name
herdash --no-rename-space      # leave the space name alone
```

Renaming is used because it is the only thing herdr renders without user
configuration: a space row is `state_icon, workspace, branch, git_status`, and
of those only the label can be set by a program.

If you would rather not have the name replaced, herdash also publishes a
`$herdash` metadata token, which you can place anywhere in your own space row
template:

```toml
[ui.sidebar.spaces]
rows = [["state_icon", "workspace"], ["branch", "git_status", "$herdash"]]
```

Then `herdash --no-rename-space` keeps the repo name and shows herdash beside
it. The token carries a 30-second TTL that herdr expires on its own, so it
needs no cleanup. `--no-sidebar-token` disables it.

### Colors

herdash draws with **ANSI-named colors and no background**, so it adopts your
terminal's palette rather than imposing one. On a rose-pine or gruvbox terminal
it comes out rose-pine or gruvbox, with no configuration.

It cannot read herdr's own theme directly. herdr's `[theme]` styles herdr's
chrome — sidebar, borders, status bar — not the contents of a pane, and it
publishes no palette over its socket API (all 91 methods carry no color
surface). The built-in palettes are computed in herdr's code rather than stored
as literals, so they cannot be extracted either.

What *does* bridge the two is herdr's own override mechanism. Anything you set
under `[theme.custom]` in `~/.config/herdr/config.toml` themes herdr **and**
herdash identically:

```toml
[theme.custom]
accent = "#f5c2e7"        # section headings
red = "#ff6188"           # blocked agents, "waiting on you"
green = "#a6e3a1"         # finished work
yellow = "#f6c177"        # active work
selection_bg = "#313244"  # the selected row
```

Hex, `rgb(r, g, b)` and named colors all work, matching herdr's own syntax.
Anything you leave out keeps the terminal default. `--theme ansi` ignores the
config entirely.

### Waiting on you

The top section lists agents that are blocked on **you**, and it is driven by
the model reading each transcript — not by herdr's lifecycle state. That
distinction matters in both directions: an agent can be `working` while sitting
on a question it already asked, and `idle` simply because it finished cleanly
and wants nothing. Each row shows what is actually needed, phrased as the thing
to do:

```
⚠ waiting on you ────────────────── 2
⊘ scratch-investigate-auths  w3V  4m
  → Decide whether to write the expiry-mismatch guard
● feat-payment-sources       w3X  2m
  → Approve the Figma authorization
```

Agents are *lifted* into this section rather than duplicated, so `j`/`k` never
lands on the same agent twice. Until an agent has been summarized there is
nothing to classify, so herdr's `blocked` stands in as the best signal
available.

### Status glyphs

Glyph, color and sort order all encode one idea — how much does this want
your attention?

| | Status | Meaning |
| --- | --- | --- |
| `⊘` | `blocked` | herdr recognized an approval or question dialog. It needs you now. |
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

At five agents under heavy activity this costs a few cents an hour. Concurrent
calls are capped so a large fleet cannot fire dozens of requests at once, and
`R` reaches every agent including ones the `a` filter is hiding.

Which model to point it at is measured rather than guessed — see
[docs/benchmark.md](docs/benchmark.md) for seven models scored on cost,
latency, prose quality and how reliably they spot an agent that actually needs
you. `--model openai/gpt-oss-20b:nitro` is 4.6x cheaper with identical
attention accuracy and plainer writing.

## Privacy

**Summarization sends your agents' terminal transcripts — which contain your
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
| `⚠ summary unavailable` on one agent | That agent's summary failed; the last successful one stays on screen beneath the error. Retries follow a 5s → 15s → 45s → 135s → 5m backoff. `r` forces an immediate retry. |
| `herdr: …` notice in the header | herdr answered but not usefully — a protocol mismatch, say. Polling continues on the same schedule. |

## Development

See [AGENTS.md](AGENTS.md) for repository conventions and the herdr API
gotchas worth knowing before changing the client.

```bash
mise run check     # fmt --check + clippy -D warnings + all tests
mise run lint-ci   # actionlint + shellcheck
```

Every test runs without a herdr server and without network access.

Releases are tag-driven and build on [Namespace](https://namespace.so) runners;
see [docs/releasing.md](docs/releasing.md) for the semver policy and procedure.

## License

MIT. See [LICENSE](LICENSE).
