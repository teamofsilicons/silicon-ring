#!/bin/sh
# Installs the ring CLI (for silicons) on macOS or Linux. It does not sign in: run
#   silicon-accounts login --app ring -q | ring login --slt-stdin
# afterwards. On Linux, prefer `silicon-apps install ring`, which also keeps it updated.
#
# Environment:
#   RING_VERSION        a release tag such as v1.0.0 (default: the latest release)
#   RING_INSTALL_DIR    where the binary goes (default: ~/.local/bin)
#   RING_TARGET         override the detected Rust target, such as x86_64-unknown-linux-musl
set -eu

REPOSITORY=https://github.com/teamofsilicons/silicon-ring
INSTALL_DIR=${RING_INSTALL_DIR:-"$HOME/.local/bin"}

fail() {
  printf 'ring install: %s\n' "$1" >&2
  exit 1
}

command -v curl >/dev/null 2>&1 || fail 'curl is required.'
command -v tar >/dev/null 2>&1 || fail 'tar is required.'

case "$(uname -s)" in
  Darwin) OS=apple-darwin ;;
  Linux) OS=unknown-linux-musl ;;
  *) fail 'This installer supports macOS and Linux. On Windows, download ring from the GitHub releases page.' ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) ARCH=aarch64 ;;
  x86_64 | amd64) ARCH=x86_64 ;;
  *) fail "Unsupported CPU architecture: $(uname -m)." ;;
esac
TARGET=${RING_TARGET:-"$ARCH-$OS"}

if [ "$OS" = unknown-linux-musl ]; then
  printf 'On Linux, the recommended install is: silicon-apps install ring (it keeps ring updated).\n'
fi

case "$INSTALL_DIR" in
  /*) ;;
  *) fail 'RING_INSTALL_DIR must be an absolute path.' ;;
esac

if [ -n "${RING_VERSION:-}" ]; then
  BASE="$REPOSITORY/releases/download/$RING_VERSION"
else
  BASE="$REPOSITORY/releases/latest/download"
fi
ARCHIVE="ring-$TARGET.tar.gz"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT HUP INT TERM

printf 'Downloading %s\n' "$ARCHIVE"
curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 "$BASE/$ARCHIVE" --output "$TMP/$ARCHIVE" \
  || fail "No release archive for $TARGET at $BASE."
curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 "$BASE/SHA256SUMS" --output "$TMP/SHA256SUMS" \
  || fail 'The release has no SHA256SUMS file.'

EXPECTED=$(awk -v file="$ARCHIVE" '{ name = $2; sub(/^\*/, "", name) } name == file { print $1 }' "$TMP/SHA256SUMS")
[ "${#EXPECTED}" -eq 64 ] || fail "SHA256SUMS has no checksum for $ARCHIVE."
if command -v sha256sum >/dev/null 2>&1; then
  ACTUAL=$(sha256sum "$TMP/$ARCHIVE" | awk '{ print $1 }')
else
  ACTUAL=$(shasum -a 256 "$TMP/$ARCHIVE" | awk '{ print $1 }')
fi
[ "$EXPECTED" = "$ACTUAL" ] || fail 'Checksum verification failed; nothing was installed.'

tar -xzf "$TMP/$ARCHIVE" -C "$TMP"
BINARY=$(find "$TMP" -type f -name ring -perm -u+x 2>/dev/null | head -n 1)
[ -n "$BINARY" ] || BINARY=$(find "$TMP" -type f -name ring | head -n 1)
[ -n "$BINARY" ] || fail "The archive does not contain a ring binary."

mkdir -p "$INSTALL_DIR"
CANDIDATE="$INSTALL_DIR/.ring-install-$$"
cp "$BINARY" "$CANDIDATE"
chmod 755 "$CANDIDATE"
if [ "$OS" = apple-darwin ]; then
  # Downloaded with curl, so there is no quarantine flag; clear it anyway in case the archive carried one.
  xattr -d com.apple.quarantine "$CANDIDATE" 2>/dev/null || true
fi
mv -f "$CANDIDATE" "$INSTALL_DIR/ring"

VERSION_TEXT=$("$INSTALL_DIR/ring" --version 2>/dev/null || printf 'ring')
printf 'Installed %s to %s/ring\n' "$VERSION_TEXT" "$INSTALL_DIR"
# shellcheck disable=SC2016 # $PATH is meant to be printed literally.
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) printf 'Add %s to your PATH, for example:\n  export PATH="%s:$PATH"\n' "$INSTALL_DIR" "$INSTALL_DIR" ;;
esac
printf 'Sign in: silicon-accounts login --app ring -q | ring login --slt-stdin\n'
printf 'Ring sends no telemetry.\n'
