#!/usr/bin/env bash
# Builds ring-server and ring-media for the production host (aarch64, Amazon Linux 2023,
# glibc 2.34) inside an amazonlinux:2023 arm64 container, and writes them to dist/server/.
#
#   scripts/build-linux-server.sh            # needs a running arm64 docker host (colima start)
#
# Environment:
#   RING_BUILD_IMAGE   image tag for the toolchain (default ring-al2023-build:rust-1.98.1-llvm21)
#   CARGO_BUILD_JOBS   parallel rustc jobs inside the container (default: all CPUs)
#   CARGO_TARGET_DIR   a directory inside the repository (it is mounted into the container); the
#                      build goes to its al2023/ subdirectory (default target/infra/al2023)
#   LK_CUSTOM_WEBRTC   an unpacked webrtc-linux-arm64-release directory inside the repository, used
#                      instead of downloading libwebrtc from GitHub inside the container (for slow
#                      or flaky networks)
#
# The cargo registry lives in named docker volumes so repeat builds do not download crates again.
# Build output never shares a directory with host builds (build scripts differ).
set -euo pipefail

ROOT=$(cd "$(dirname "$0")/.." && pwd)
RUST_VERSION=1.98.1
LLVM_MAJOR=21
IMAGE=${RING_BUILD_IMAGE:-ring-al2023-build:rust-${RUST_VERSION}-llvm${LLVM_MAJOR}}
PLATFORM=linux/arm64
OUT="$ROOT/dist/server"

log() { printf '==> %s\n' "$*" >&2; }
die() { printf 'build-linux-server: %s\n' "$*" >&2; exit 1; }

TARGET_DIR=target/infra/al2023
if [[ -n "${CARGO_TARGET_DIR:-}" ]]; then
  case "$CARGO_TARGET_DIR" in
    "$ROOT"/*) TARGET_DIR="${CARGO_TARGET_DIR#"$ROOT"/}/al2023" ;;
    /*) die "CARGO_TARGET_DIR must be inside $ROOT, which is what the build container sees" ;;
    *) TARGET_DIR="${CARGO_TARGET_DIR%/}/al2023" ;;
  esac
fi

WEBRTC=
if [[ -n "${LK_CUSTOM_WEBRTC:-}" ]]; then
  dir=$(cd "$LK_CUSTOM_WEBRTC" 2>/dev/null && pwd) || die "LK_CUSTOM_WEBRTC $LK_CUSTOM_WEBRTC does not exist"
  case "$dir" in
    "$ROOT"/*) WEBRTC="/src/${dir#"$ROOT"/}" ;;
    *) die "LK_CUSTOM_WEBRTC must be inside $ROOT, which is what the build container sees" ;;
  esac
fi

command -v docker >/dev/null || die 'docker is required (colima start gives an arm64 docker host).'

# A credential helper that is configured but not installed (Docker Desktop removed, colima in
# use) breaks anonymous pulls. Fall back to an empty client config on the same daemon.
client_config="${DOCKER_CONFIG:-$HOME/.docker}/config.json"
store=$(python3 -I -c 'import json, sys
try: print(json.load(open(sys.argv[1])).get("credsStore", ""))
except (OSError, ValueError): print("")' "$client_config")
if [[ -n "$store" ]] && ! command -v "docker-credential-$store" >/dev/null; then
  host=$(docker context inspect --format '{{.Endpoints.docker.Host}}' 2>/dev/null || true)
  export DOCKER_CONFIG="$ROOT/$TARGET_DIR/docker-config"
  mkdir -p "$DOCKER_CONFIG"
  [[ -f "$DOCKER_CONFIG/config.json" ]] || printf '{}\n' > "$DOCKER_CONFIG/config.json"
  if [[ -z "${DOCKER_HOST:-}" && -n "$host" ]]; then export DOCKER_HOST="$host"; fi
  log "docker-credential-$store is not installed; using an empty docker client config"
fi

arch=$(docker info --format '{{.Architecture}}' 2>/dev/null) || die 'the docker daemon is not reachable (try: colima start).'
case "$arch" in
  aarch64 | arm64) ;;
  *) die "an arm64 docker host is required (the production host is Graviton); this one is $arch." ;;
esac

if ! docker image inspect "$IMAGE" >/dev/null 2>&1; then
  log "building toolchain image $IMAGE"
  docker build --platform "$PLATFORM" -t "$IMAGE" -f - "$ROOT/scripts" <<DOCKERFILE
FROM amazonlinux:2023
ENV CARGO_HOME=/usr/local/cargo RUSTUP_HOME=/usr/local/rustup PATH=/usr/local/cargo/bin:/usr/local/bin:/usr/bin:/bin
COPY al2023-toolchain.sh /tmp/al2023-toolchain.sh
RUN RUST_VERSION=${RUST_VERSION} LLVM_MAJOR=${LLVM_MAJOR} sh /tmp/al2023-toolchain.sh && rm /tmp/al2023-toolchain.sh
DOCKERFILE
fi

mkdir -p "$ROOT/$TARGET_DIR" "$OUT"
log 'compiling ring-server and ring-media (the first build downloads libwebrtc and takes a while)'
# shellcheck disable=SC2016 # expanded inside the container
docker run --rm --platform "$PLATFORM" \
  -v "$ROOT:/src" -w /src \
  -v ring-al2023-cargo-registry:/usr/local/cargo/registry \
  -v ring-al2023-cargo-git:/usr/local/cargo/git \
  -e CARGO_TARGET_DIR="/src/$TARGET_DIR" \
  -e CARGO_TERM_COLOR=always \
  -e JOBS="${CARGO_BUILD_JOBS:-}" \
  -e WEBRTC="$WEBRTC" \
  -e CC=clang -e CXX=clang++ \
  "$IMAGE" bash -euo pipefail -c '
    if [[ -n "$JOBS" ]]; then export CARGO_BUILD_JOBS=$JOBS; fi
    if [[ -n "$WEBRTC" ]]; then export LK_CUSTOM_WEBRTC=$WEBRTC; fi
    cargo build --release --locked -p silicon-ring-server -p silicon-ring-media
    sh scripts/check-linux-binary.sh "$CARGO_TARGET_DIR/release/ring-server" "$CARGO_TARGET_DIR/release/ring-media"
    rustc --version > "$CARGO_TARGET_DIR/release/rustc-version.txt"
  '

for bin in ring-server ring-media; do
  install -m 0755 "$ROOT/$TARGET_DIR/release/$bin" "$OUT/$bin"
done
python3 -I "$ROOT/scripts/server-receipt.py" "$OUT" --rustc "$(cat "$ROOT/$TARGET_DIR/release/rustc-version.txt")"
log "wrote $OUT"
cat "$OUT/SHA256SUMS"
