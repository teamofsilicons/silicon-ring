# syntax=docker/dockerfile:1
ARG BUILDPLATFORM
FROM --platform=$BUILDPLATFORM rust:1.98-bookworm AS backend
ARG TARGETARCH
WORKDIR /build
RUN case "$TARGETARCH" in \
      amd64) triple=x86_64-unknown-linux-gnu; compiler=gcc-x86-64-linux-gnu; devlib=libc6-dev-amd64-cross ;; \
      arm64) triple=aarch64-unknown-linux-gnu; compiler=gcc-aarch64-linux-gnu; devlib=libc6-dev-arm64-cross ;; \
      *) exit 1 ;; \
    esac \
    && apt-get update && apt-get install -y --no-install-recommends "$compiler" "$devlib" \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add "$triple" && printf '%s' "$triple" > /build/rust-target
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc \
    CC_x86_64_unknown_linux_gnu=x86_64-linux-gnu-gcc \
    CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --locked --release --target "$(cat /build/rust-target)" -p ring-server --features ring-providers/s3 \
    && cp "target/$(cat /build/rust-target)/release/ring-server" /build/ring-server

FROM --platform=$BUILDPLATFORM node:24-bookworm-slim AS web
WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
COPY scripts/prepare-web.mjs scripts/install.sh /build/scripts/
RUN npm run build

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home ring && mkdir /data && chown ring:ring /data
COPY --from=backend /build/ring-server /usr/local/bin/ring-server
COPY --from=web /build/web/dist /app/web/dist
ENV RING_BIND=0.0.0.0:8765 RING_DATA_DIR=/data RING_WEB_DIR=/app/web/dist
USER ring
EXPOSE 8765
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s CMD curl --fail --silent http://127.0.0.1:8765/health || exit 1
CMD ["ring-server"]
