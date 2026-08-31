#!/usr/bin/env bash
# Prepare a release commit and tag. Does not push — pushing is the deliberate,
# separate step that actually publishes.
#
#   mise run release 0.2.0
#   mise run release 0.2.0 -- --dry-run
#
# See docs/releasing.md for the semver policy that decides the number.
set -euo pipefail

die() { printf '\033[31merror:\033[0m %s\n' "$*" >&2; exit 1; }
step() { printf '\033[36m==>\033[0m %s\n' "$*" >&2; }

DRY_RUN=false
VERSION=""
while [ $# -gt 0 ]; do
  case "$1" in
    --dry-run) DRY_RUN=true; shift ;;
    -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
    -*) die "unknown flag: $1" ;;
    *) [ -z "$VERSION" ] || die "unexpected argument: $1"; VERSION="$1"; shift ;;
  esac
done

[ -n "$VERSION" ] || die "usage: mise run release <version>   e.g. 0.2.0"
VERSION="${VERSION#v}"

# Strict semver, optionally with a pre-release suffix.
if ! printf '%s' "$VERSION" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$'; then
  die "'$VERSION' is not semver (expected MAJOR.MINOR.PATCH[-prerelease])"
fi

cd "$(git rev-parse --show-toplevel)"

step "Checking working tree"
[ -z "$(git status --porcelain)" ] || die "working tree is dirty; commit or stash first"

branch="$(git rev-parse --abbrev-ref HEAD)"
[ "$branch" = "main" ] || die "releases are cut from main, not '$branch'"

git fetch --quiet origin main
if [ -n "$(git rev-list HEAD..origin/main --count 2>/dev/null | grep -v '^0$' || true)" ]; then
  die "local main is behind origin/main; pull first"
fi

CURRENT="$(mise exec -- cargo metadata --format-version 1 --no-deps | jq -r '.packages[0].version')"
# Equal versions are legitimate: the very first release tags the version the
# crate already carries, and a version bumped in an earlier commit needs only
# a tag. The tag-existence check below is what actually prevents a re-release.
if [ "$CURRENT" = "$VERSION" ]; then
  NEEDS_BUMP=false
  step "Version already $VERSION; tagging without a bump"
else
  NEEDS_BUMP=true
  step "Current version: $CURRENT  ->  $VERSION"
fi

# Refuse to reuse a tag; retagging a published release breaks anyone pinned to it.
if git rev-parse --verify --quiet "refs/tags/v$VERSION" >/dev/null; then
  die "tag v$VERSION already exists"
fi

# Highest existing version must be lower, so history stays monotonic.
LATEST="$(git tag --list 'v*' --sort=-v:refname | head -n1 || true)"
if [ -n "$LATEST" ]; then
  highest="$(printf '%s\n%s\n' "${LATEST#v}" "$VERSION" | sort -V | tail -n1)"
  [ "$highest" = "$VERSION" ] || die "$VERSION is not greater than the latest tag $LATEST"
fi

step "Running full check"
mise run check

step "Updating Cargo.toml"
if ! $NEEDS_BUMP; then
  printf '  already at %s, nothing to change\n' "$VERSION" >&2
elif $DRY_RUN; then
  printf '  (dry run) would set version = "%s"\n' "$VERSION" >&2
else
  # Only the [package] version, which is the first `version =` in the file.
  awk -v v="$VERSION" '
    !done && /^version = "/ { sub(/"[^"]*"/, "\"" v "\""); done = 1 }
    { print }
  ' Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml
  # Resolving rewrites the package version inside Cargo.lock too, which the
  # release build needs because it runs with --locked.
  mise exec -- cargo check --quiet >/dev/null
fi

step "Updating CHANGELOG.md"
RANGE="${LATEST:+$LATEST..}HEAD"
notes="$(git log "$RANGE" --no-merges --format='%s' || true)"
{
  printf '## v%s — %s\n\n' "$VERSION" "$(date -u +%Y-%m-%d)"
  for type in feat fix perf refactor docs test build ci chore; do
    body="$(printf '%s\n' "$notes" | grep -E "^${type}(\(.+\))?!?: " || true)"
    [ -n "$body" ] || continue
    case "$type" in
      feat) heading="Added" ;; fix) heading="Fixed" ;; perf) heading="Performance" ;;
      docs) heading="Documentation" ;; *) heading="Internal" ;;
    esac
    printf '### %s\n\n' "$heading"
    printf '%s\n' "$body" | sed -E "s/^${type}(\(.+\))?!?: /- /"
    printf '\n'
  done
} > .changelog.new

if $DRY_RUN; then
  step "Dry run — CHANGELOG entry that would be added:"
  sed 's/^/  /' .changelog.new >&2
  rm -f .changelog.new
  if $NEEDS_BUMP; then
    git checkout -- Cargo.toml Cargo.lock 2>/dev/null || true
  fi
  exit 0
fi

if [ -f CHANGELOG.md ]; then
  { head -n1 CHANGELOG.md; printf '\n'; cat .changelog.new; tail -n +2 CHANGELOG.md; } > CHANGELOG.tmp
else
  { printf '# Changelog\n\n'; cat .changelog.new; } > CHANGELOG.tmp
fi
mv CHANGELOG.tmp CHANGELOG.md
rm -f .changelog.new

step "Committing and tagging"
git add CHANGELOG.md
$NEEDS_BUMP && git add Cargo.toml Cargo.lock
git commit -q -m "chore(release): v$VERSION"
git tag -a "v$VERSION" -m "herdash v$VERSION"

cat >&2 <<EOF

Prepared v$VERSION. Nothing has been pushed.

Review, then publish:

  git show --stat HEAD
  git push origin main
  git push origin v$VERSION

Pushing the tag starts .github/workflows/release.yml. Watch it with:

  gh run watch --exit-status
EOF
