# Changelog

## v0.1.0 — 2026-08-31

### Added

- american spelling throughout, and inherit terminal/herdr colors
- benchmark seven summarisation models and switch the default
- add attention section, richer detail, mouse support
- render repo-grouped sidebar, detail pane, header and help overlay
- add config resolution and app state with pane-id-keyed selection
- add summarisation policy and OpenRouter structured-output client
- group agents by repo with urgency-first ordering
- scaffold herdash crate and add herdr NDJSON socket client

### Fixed

- allow tagging a version the crate already carries
- pin rustfmt and clippy components in mise.toml
- actually call the reasoning escalation, and test the seam
- use the snapshot revision as the summarisation change signal
- measure terminal columns, not characters, in the sidebar
- correct the herdr connection model and integrate review findings

### Documentation

- scrub truncated project names missed by the whole-token pass
- use generic project names in examples
- add README, AGENTS.md and licence; correct the spec
- add herdash implementation plan
- add herdash design spec

### Internal

- add tag-driven release flow on Namespace runners


All notable changes are recorded here. Entries are generated from
[Conventional Commits](https://www.conventionalcommits.org) by
`mise run release`, then edited for clarity where it helps a reader.

Versions follow [Semantic Versioning](https://semver.org). While the version is
below 1.0, a **minor** bump may contain breaking changes; see
[docs/releasing.md](docs/releasing.md).
