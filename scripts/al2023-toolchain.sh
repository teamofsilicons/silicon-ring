#!/bin/sh
# Prepares an Amazon Linux 2023 container (run as root) to build ring-server and ring-media.
# Shared by scripts/build-linux-server.sh (local, colima) and .github/workflows/media.yml.
#
# Building on AL2023 itself keeps the binaries on the production host's glibc (2.34).
# webrtc-sys compiles its C++ against the hermetic libc++ baked into libwebrtc.a, which needs
# clang 21.1 or later; AL2023 packages it as clang21.
set -eu

RUST_VERSION="${RUST_VERSION:-1.98.1}"
LLVM_MAJOR="${LLVM_MAJOR:-21}"
CARGO_HOME="${CARGO_HOME:-/usr/local/cargo}"
RUSTUP_HOME="${RUSTUP_HOME:-/usr/local/rustup}"
export CARGO_HOME RUSTUP_HOME

dnf -y -q install --setopt=install_weak_deps=False \
  "clang${LLVM_MAJOR}" "lld${LLVM_MAJOR}" gcc gcc-c++ binutils make cmake \
  glib2-devel pkgconf-pkg-config \
  git tar gzip xz unzip findutils which file
dnf clean all
rm -rf /var/cache/dnf

for tool in clang clang++; do
  ln -sf "/usr/bin/${tool}-${LLVM_MAJOR}" "/usr/local/bin/${tool}"
done
ln -sf "/usr/bin/ld.lld-${LLVM_MAJOR}" /usr/local/bin/ld.lld

curl --proto '=https' --tlsv1.2 --fail --silent --show-error https://sh.rustup.rs \
  | sh -s -- -y --no-modify-path --profile minimal --default-toolchain "$RUST_VERSION"
"$CARGO_HOME/bin/rustc" --version
clang++ --version | head -n 1
