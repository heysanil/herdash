#!/bin/sh
# Install herdash from GitHub release binaries.
#
#   curl -fsSL https://raw.githubusercontent.com/heysanil/herdash/main/install.sh | sh
#
# Environment (the usable interface when piped into sh):
#   HERDASH_VERSION      tag to install, e.g. v0.2.0. Default: latest release.
#   HERDASH_INSTALL_DIR  where to put the binary. Default: ~/.local/bin.
#
# Flags, when the script is downloaded and run directly:
#   --version <tag>   --dir <path>   --help
#
# Downloads are verified against the release's SHA256SUMS before anything is
# written to the install directory. A checksum mismatch aborts.

set -eu

REPO="heysanil/herdash"
BIN="herdash"

# ----------------------------------------------------------------- output --
# Colour only when stderr is a terminal, so piped logs stay clean.
if [ -t 2 ] && [ -z "${NO_COLOR:-}" ]; then
    C_RED=$(printf '\033[31m'); C_GREEN=$(printf '\033[32m')
    C_DIM=$(printf '\033[2m');  C_OFF=$(printf '\033[0m')
else
    C_RED=''; C_GREEN=''; C_DIM=''; C_OFF=''
fi

info() { printf '%s\n' "$*" >&2; }
dim()  { printf '%s%s%s\n' "$C_DIM" "$*" "$C_OFF" >&2; }
ok()   { printf '%s%s%s\n' "$C_GREEN" "$*" "$C_OFF" >&2; }
die()  { printf '%serror:%s %s\n' "$C_RED" "$C_OFF" "$*" >&2; exit 1; }

usage() {
    cat >&2 <<'USAGE'
Install herdash from GitHub release binaries.

Usage:
  install.sh [--version <tag>] [--dir <path>]

Options:
  --version <tag>   Release tag to install (default: latest)
  --dir <path>      Install directory (default: ~/.local/bin)
  --help            Show this message

Environment:
  HERDASH_VERSION, HERDASH_INSTALL_DIR
USAGE
}

# ------------------------------------------------------------------- args --
VERSION="${HERDASH_VERSION:-}"
INSTALL_DIR="${HERDASH_INSTALL_DIR:-}"

while [ $# -gt 0 ]; do
    case "$1" in
        --version) [ $# -ge 2 ] || die "--version needs a value"; VERSION="$2"; shift 2 ;;
        --dir)     [ $# -ge 2 ] || die "--dir needs a value";     INSTALL_DIR="$2"; shift 2 ;;
        --help|-h) usage; exit 0 ;;
        *)         die "unknown argument: $1" ;;
    esac
done

: "${INSTALL_DIR:=${HOME}/.local/bin}"

# --------------------------------------------------------------- platform --
need() { command -v "$1" >/dev/null 2>&1 || die "$1 is required but not installed"; }
need tar
need mktemp

if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
    fetch_stdout() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO "$2" "$1"; }
    fetch_stdout() { wget -qO- "$1"; }
else
    die "either curl or wget is required"
fi

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
    Darwin) os_part="apple-darwin" ;;
    Linux)  os_part="unknown-linux-gnu" ;;
    *)      die "unsupported operating system: $os (herdash ships macOS and Linux builds)" ;;
esac

case "$arch" in
    x86_64|amd64)  arch_part="x86_64" ;;
    arm64|aarch64) arch_part="aarch64" ;;
    *)             die "unsupported architecture: $arch" ;;
esac

TARGET="${arch_part}-${os_part}"

# ---------------------------------------------------------------- version --
if [ -z "$VERSION" ]; then
    dim "Resolving latest release..."
    # Read the tag from the API without needing jq.
    VERSION="$(fetch_stdout "https://api.github.com/repos/${REPO}/releases/latest" \
        | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
        | head -n 1)"
    [ -n "$VERSION" ] || die "could not determine the latest release of ${REPO}"
fi
# Accept both "0.2.0" and "v0.2.0".
case "$VERSION" in v*) ;; *) VERSION="v${VERSION}" ;; esac

ARCHIVE="${BIN}-${VERSION}-${TARGET}.tar.gz"
BASE="https://github.com/${REPO}/releases/download/${VERSION}"

info "Installing ${BIN} ${VERSION} (${TARGET})"

# --------------------------------------------------------------- download --
TMP="$(mktemp -d 2>/dev/null || mktemp -d -t herdash)"
# shellcheck disable=SC2064  # expand TMP now: it must not change before EXIT.
trap "rm -rf '$TMP'" EXIT INT TERM

dim "Downloading ${ARCHIVE}"
fetch "${BASE}/${ARCHIVE}" "${TMP}/${ARCHIVE}" \
    || die "no build for ${TARGET} in ${VERSION} — see https://github.com/${REPO}/releases"

dim "Verifying checksum"
fetch "${BASE}/SHA256SUMS" "${TMP}/SHA256SUMS" \
    || die "release ${VERSION} has no SHA256SUMS; refusing to install unverified binaries"

if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "${TMP}/${ARCHIVE}" | cut -d' ' -f1)"
elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "${TMP}/${ARCHIVE}" | cut -d' ' -f1)"
else
    die "no sha256sum or shasum available to verify the download"
fi

expected="$(grep -F " ${ARCHIVE}" "${TMP}/SHA256SUMS" | cut -d' ' -f1 | head -n 1)"
[ -n "$expected" ] || die "${ARCHIVE} is not listed in SHA256SUMS"
if [ "$actual" != "$expected" ]; then
    die "checksum mismatch for ${ARCHIVE}
  expected ${expected}
  actual   ${actual}
Refusing to install. This may mean a corrupted download or a tampered asset."
fi

# ---------------------------------------------------------------- install --
tar -xzf "${TMP}/${ARCHIVE}" -C "$TMP" || die "could not extract ${ARCHIVE}"

extracted="$(find "$TMP" -type f -name "$BIN" -perm -u+x 2>/dev/null | head -n 1)"
[ -n "$extracted" ] || die "archive did not contain a ${BIN} binary"

mkdir -p "$INSTALL_DIR" || die "could not create ${INSTALL_DIR}"
# Install to a temporary name first, then move into place, so an interrupted
# install cannot leave a half-written binary on PATH.
staged="${INSTALL_DIR}/.${BIN}.$$"
cp "$extracted" "$staged" || die "could not write to ${INSTALL_DIR} (try: --dir ~/.local/bin)"
chmod 0755 "$staged"
mv -f "$staged" "${INSTALL_DIR}/${BIN}" || die "could not install into ${INSTALL_DIR}"

ok "Installed ${BIN} ${VERSION} to ${INSTALL_DIR}/${BIN}"

# ------------------------------------------------------------------- PATH --
case ":${PATH}:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
        info ""
        info "${INSTALL_DIR} is not on your PATH. Add it:"
        info ""
        info "  export PATH=\"${INSTALL_DIR}:\$PATH\""
        info ""
        ;;
esac

if "${INSTALL_DIR}/${BIN}" --version >/dev/null 2>&1; then
    dim "$("${INSTALL_DIR}/${BIN}" --version)"
else
    info "Warning: the installed binary did not run. It may not match your platform."
fi

info ""
info "Run it inside a herdr pane:  ${BIN}"
