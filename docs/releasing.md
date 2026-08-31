# Releasing herdash

Releases are **tag-driven**. Pushing a tag matching `v*.*.*` runs
[`.github/workflows/release.yml`](../.github/workflows/release.yml), which
builds four binaries, publishes a GitHub Release with checksums, and writes
install instructions into the release notes. Nothing else publishes anything.

This page is written to be executable by a coding agent with no prior context.
Follow it top to bottom.

---

## 1. Decide the version

herdash follows [Semantic Versioning](https://semver.org). Read the change and
pick the smallest bump that honestly describes it.

> **While the version is below 1.0**, the leading zero absorbs one level:
> breaking changes go in **MINOR** (`0.1.x` → `0.2.0`) and everything else in
> **PATCH**. There is no way to signal "major" until 1.0. Do not reach for
> `1.0.0` to communicate significance — 1.0 is a promise of stability, not a
> milestone of effort.

For a terminal application, "breaking" means a user's existing invocation or
muscle memory stops working:

| Change | Pre-1.0 | Post-1.0 |
| --- | --- | --- |
| Remove or rename a CLI flag | minor | **major** |
| Change what an existing flag does | minor | **major** |
| Change a keybinding to something incompatible | minor | **major** |
| Raise the minimum herdr version required | minor | **major** |
| Move the install location or binary name | minor | **major** |
| Add a flag, key, or panel | patch | minor |
| Change a default value (e.g. the summarization model) | patch | minor |
| Raise the Rust toolchain floor | patch | minor |
| Fix a bug without changing the interface | patch | patch |
| Documentation, tests, CI, refactors with no user-visible effect | patch | patch |

Two judgement calls worth stating, because they recur:

- **Changing a default is not breaking, but it is not invisible either.** Every
  existing invocation still works; the behavior differs. That is a minor bump
  post-1.0, and it belongs in the release notes prominently.
- **A prompt change that alters summary wording is a patch**, unless it changes
  a field's meaning — for example, redefining when `needs_attention` fires.
  That changes what the attention panel means, so treat it as a default change.

Pre-release tags are supported: `v0.2.0-rc.1` publishes as a GitHub
pre-release and is excluded from "latest", so `install.sh` will not pick it up
unless asked for by name.

---

## 2. Cut it

```bash
mise run release 0.2.0            # prepare
mise run release 0.2.0 -- --dry-run   # or preview the changelog entry first
```

The script refuses to continue unless the tree is clean, you are on `main`,
`main` is not behind `origin`, the version is greater than the newest existing
tag, and `mise run check` passes. Then it:

1. sets the `[package]` version in `Cargo.toml`,
2. resolves so `Cargo.lock` records the new version — the release build runs
   with `--locked`, so a stale lock fails the build,
3. prepends a `CHANGELOG.md` section grouped from Conventional Commit prefixes
   since the previous tag,
4. commits `chore(release): vX.Y.Z` and creates an annotated tag.

**It does not push.** Review, then publish:

```bash
git show --stat HEAD
git push origin main
git push origin v0.2.0     # this is the step that publishes
```

---

## 3. Watch it land

```bash
gh run watch --exit-status
gh release view v0.2.0
```

The workflow runs three stages:

| Job | What it guarantees |
| --- | --- |
| `verify` | The tag and `Cargo.toml` agree, and the full check suite passes |
| `build` | Four targets compile and package, each on a Namespace runner |
| `publish` | Assets and `SHA256SUMS` are attached, notes generated |

Targets built: `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
`aarch64-apple-darwin`, `x86_64-apple-darwin`.

Then verify the thing users actually run:

```bash
curl -fsSL https://raw.githubusercontent.com/heysanil/herdash/main/install.sh \
  | HERDASH_VERSION=v0.2.0 HERDASH_INSTALL_DIR=/tmp/verify sh
/tmp/verify/herdash --version    # must print 0.2.0
```

A release is not done until that command prints the version you tagged.

---

## 4. When something goes wrong

**The tag does not match `Cargo.toml`.** `verify` fails in seconds and nothing
is published. Delete the tag, fix the version, tag again:

```bash
git tag -d v0.2.0 && git push origin :refs/tags/v0.2.0
```

**A build fails after some targets succeeded.** `fail-fast` is off, so you can
see every target's result in one run. No release is created unless all four
build, so a partial failure publishes nothing. Fix and re-tag.

**A bad release is already published.** Do **not** move the tag — anyone who
already installed it would silently get different bytes under the same version.
Publish a new patch version instead, and mark the bad one:

```bash
gh release edit v0.2.0 --prerelease --notes "Superseded by v0.2.1. Do not use."
```

**Jobs sit queued forever.** The runners are [Namespace](https://namespace.so),
not GitHub-hosted. The repository must be connected to a Namespace workspace,
or nothing will ever pick up a job carrying an `nscloud-*` label. Check
<https://cloud.namespace.so>. To fall back temporarily, swap the `runs-on`
labels for `ubuntu-24.04` / `macos-14`; the workflow needs no other change,
though the macOS build then runs on an Intel or Apple Silicon GitHub runner
rather than cross-compiling.

---

## 5. Runners

All workflows run on Namespace. Labels follow
`nscloud-{os}-{arch}-{shape}[-with-cache]`, with cache tag and size as separate
companion labels. The valid set for this repo is listed in
[`.github/actionlint.yaml`](../.github/actionlint.yaml), which is also what
lets `actionlint` check these files — run `mise run lint-ci`.

Two things about Namespace shape the workflow:

- **macOS runners are arm64 only.** The Intel Mac binary is therefore
  cross-compiled from Apple Silicon with `rustup target add
  x86_64-apple-darwin`. This is verified to work, including `aws-lc-rs`, which
  is the dependency most likely to object.
- **Cache volumes mount at `/cache`** and are attached by the `-with-cache`
  suffix, then linked into Cargo and mise by `nscloud-cache-action`. Jobs
  sharing a `nscloud-cache-tag-*` label share one volume, which is why the
  Linux release builds use per-architecture tags — an amd64 and an arm64 build
  must not fight over one Cargo directory.

Linux release binaries build on **Ubuntu 22.04 deliberately**, not the newest
image: linking against the older glibc keeps the binary usable on
distributions that a newer runner would silently exclude.
