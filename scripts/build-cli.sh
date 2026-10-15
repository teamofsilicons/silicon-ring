#!/usr/bin/env bash
# Builds the ring CLI (silicons only) and packages one archive per target into dist/cli/:
#
#   ring-{x86_64,aarch64}-unknown-linux-musl.tar.gz   static, cargo zigbuild
#   ring-{x86_64,aarch64}-apple-darwin.tar.gz         native (macOS host), plus
#   ring-universal-apple-darwin.tar.gz                lipo of the two
#   ring-{x86_64,aarch64}-pc-windows-msvc.zip         cargo xwin, static CRT
#   SHA256SUMS
#
#   scripts/build-cli.sh                 all targets this host can build
#   scripts/build-cli.sh TARGET...       only these Rust targets
#
# Environment: CARGO_TARGET_DIR (default target/infra), RING_CODESIGN_IDENTITY (optional macOS
# Developer ID to sign with the hardened runtime; otherwise binaries keep the linker's ad-hoc
# signature).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$ROOT/target/infra}
OUT="$ROOT/dist/cli"
ALL=(x86_64-unknown-linux-musl aarch64-unknown-linux-musl aarch64-apple-darwin x86_64-apple-darwin
  x86_64-pc-windows-msvc aarch64-pc-windows-msvc)

log() { printf '==> %s\n' "$*" >&2; }
die() { printf 'build-cli: %s\n' "$*" >&2; exit 1; }

if [[ $# -gt 0 ]]; then
  TARGETS=("$@")
else
  TARGETS=("${ALL[@]}")
  [[ "$(uname -s)" == Darwin ]] || TARGETS=(x86_64-unknown-linux-musl aarch64-unknown-linux-musl x86_64-pc-windows-msvc aarch64-pc-windows-msvc)
fi

version=$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' "$ROOT/Cargo.toml")
[[ -n "$version" ]] || die 'no workspace version in Cargo.toml'
export SOURCE_DATE_EPOCH=${SOURCE_DATE_EPOCH:-$(git -C "$ROOT" log -1 --format=%ct 2>/dev/null || echo 1767225600)}
mkdir -p "$OUT"
cd "$ROOT"

expect_arch() {
  local path=$1 target=$2 description
  description=$(file -b "$path")
  case "$target" in
    x86_64-unknown-linux-musl) [[ "$description" == *"ELF 64-bit"*"x86-64"*"statically linked"* || "$description" == *"ELF 64-bit"*"x86-64"*"static-pie"* ]] ;;
    aarch64-unknown-linux-musl) [[ "$description" == *"ELF 64-bit"*"ARM aarch64"*"statically linked"* || "$description" == *"ELF 64-bit"*"ARM aarch64"*"static-pie"* ]] ;;
    x86_64-apple-darwin) [[ "$description" == *"Mach-O 64-bit"*"x86_64"* ]] ;;
    aarch64-apple-darwin) [[ "$description" == *"Mach-O 64-bit"*"arm64"* ]] ;;
    x86_64-pc-windows-msvc) [[ "$description" == *"PE32+"*"x86-64"* ]] ;;
    aarch64-pc-windows-msvc) [[ "$description" == *"PE32+"*"Aarch64"* || "$description" == *"PE32+"*"ARM64"* ]] ;;
  esac || die "$path is not a $target binary: $description"
}

sign_macos() {
  [[ -n "${RING_CODESIGN_IDENTITY:-}" ]] || return 0
  codesign --force --options runtime --timestamp --sign "$RING_CODESIGN_IDENTITY" "$1"
}

# The three commands Silicon Apps runs, signed out and offline, plus --version.
smoke() {
  local bin=$1 home
  home=$(mktemp -d)
  "$bin" --version | grep -q "$version" || die "$bin --version does not report $version"
  HOME=$home SILICON_HOME=$home "$bin" --help >/dev/null
  HOME=$home SILICON_HOME=$home "$bin" accounts --json | python3 -I -c 'import json,sys; d=json.load(sys.stdin); assert d["app_id"] == "ring", d'
  HOME=$home SILICON_HOME=$home "$bin" login status --json | python3 -I -c 'import json,sys; d=json.load(sys.stdin); assert d["authenticated"] is False, d'
  rm -rf "$home"
  log "smoke-tested $bin"
}

built_darwin=()
for target in "${TARGETS[@]}"; do
  log "building ring for $target"
  exe=ring
  case "$target" in
    *-unknown-linux-musl)
      cargo zigbuild --release --locked -p silicon-ring-cli --target "$target" ;;
    *-apple-darwin)
      [[ "$(uname -s)" == Darwin ]] || die "$target needs a macOS host"
      MACOSX_DEPLOYMENT_TARGET=${MACOSX_DEPLOYMENT_TARGET:-12.0} cargo build --release --locked -p silicon-ring-cli --target "$target" ;;
    *-pc-windows-msvc)
      exe=ring.exe
      # ring (rustls' crypto) compiles its aarch64 Windows assembly with the GNU-style clang driver,
      # which rejects the clang-cl flags cargo-xwin passes by default.
      compiler=clang-cl
      [[ "$target" != aarch64-* ]] || compiler=clang
      env "CARGO_TARGET_$(echo "$target" | tr '[:lower:]-' '[:upper:]_')_RUSTFLAGS=-C target-feature=+crt-static" \
        XWIN_CROSS_COMPILER="$compiler" \
        cargo xwin build --release --locked -p silicon-ring-cli --target "$target" ;;
    *) die "unsupported target $target" ;;
  esac
  bin="$CARGO_TARGET_DIR/$target/release/$exe"
  expect_arch "$bin" "$target"
  case "$target" in
    *-apple-darwin)
      sign_macos "$bin"
      built_darwin+=("$target")
      [[ "$target" != "$(uname -m | sed 's/arm64/aarch64/')-apple-darwin" ]] || smoke "$bin"
      python3 -I scripts/make-archive.py "$OUT/ring-$target.tar.gz" "$bin=ring" "LICENSE=LICENSE" >/dev/null ;;
    *-pc-windows-msvc)
      python3 -I scripts/make-archive.py "$OUT/ring-$target.zip" "$bin=ring.exe" "LICENSE=LICENSE" >/dev/null ;;
    *)
      python3 -I scripts/make-archive.py "$OUT/ring-$target.tar.gz" "$bin=ring" "LICENSE=LICENSE" >/dev/null ;;
  esac
done

if [[ ${#built_darwin[@]} -eq 2 ]]; then
  universal="$CARGO_TARGET_DIR/universal-apple-darwin/release/ring"
  mkdir -p "$(dirname "$universal")"
  lipo -create -output "$universal" \
    "$CARGO_TARGET_DIR/aarch64-apple-darwin/release/ring" "$CARGO_TARGET_DIR/x86_64-apple-darwin/release/ring"
  sign_macos "$universal"
  smoke "$universal"
  python3 -I scripts/make-archive.py "$OUT/ring-universal-apple-darwin.tar.gz" "$universal=ring" "LICENSE=LICENSE" >/dev/null
fi

(
  cd "$OUT"
  shopt -s nullglob
  archives=(ring-*.tar.gz ring-*.zip)
  [[ ${#archives[@]} -gt 0 ]] || die 'nothing was built'
  shasum -a 256 "${archives[@]}" > SHA256SUMS
)
log "wrote $OUT"
cat "$OUT/SHA256SUMS"
