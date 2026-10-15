# Build environment for the Linux app (used for local builds from macOS through colima,
# and mirrors the packages the GitHub workflow installs on Ubuntu).
#   docker build -t ring-linux-build linux/docker
FROM rust:1.98-trixie

ARG LLVM_VERSION=21

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates gnupg wget pkg-config \
      libgtk-4-dev libadwaita-1-dev libglib2.0-dev libdbus-1-dev libssl-dev \
      libpulse-dev libasound2-dev \
      libx11-dev libxext-dev libxfixes-dev libxdamage-dev libxrandr-dev libxcomposite-dev \
      libdrm-dev libgbm-dev \
      dpkg-dev fakeroot file patchelf desktop-file-utils appstream librsvg2-bin librsvg2-common \
      xvfb xauth dbus dbus-x11 \
      libgtk-4-media-gstreamer gstreamer1.0-plugins-good gstreamer1.0-pulseaudio \
 && rm -rf /var/lib/apt/lists/*

# libwebrtc ships Chromium's hermetic libc++, which needs a recent clang (webrtc-sys enforces it).
RUN wget -qO /etc/apt/trusted.gpg.d/apt.llvm.org.asc https://apt.llvm.org/llvm-snapshot.gpg.key \
 && echo "deb http://apt.llvm.org/trixie/ llvm-toolchain-trixie-${LLVM_VERSION} main" > /etc/apt/sources.list.d/llvm.list \
 && apt-get update \
 && apt-get install -y --no-install-recommends clang-${LLVM_VERSION} lld-${LLVM_VERSION} \
 && ln -sf /usr/bin/clang-${LLVM_VERSION} /usr/local/bin/clang \
 && ln -sf /usr/bin/clang++-${LLVM_VERSION} /usr/local/bin/clang++ \
 && ln -sf /usr/bin/ld.lld-${LLVM_VERSION} /usr/local/bin/ld.lld \
 && rm -rf /var/lib/apt/lists/*
