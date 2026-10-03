FROM node:24-bookworm-slim AS web
WORKDIR /build/web
COPY web/package.json web/package-lock.json ./
RUN npm ci
COPY web/ ./
RUN npm run build

FROM rust:1.98-bookworm AS backend
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY crates/ ./crates/
RUN cargo build --locked --release -p ring-server --features ring-providers/s3

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --create-home ring && mkdir /data && chown ring:ring /data
COPY --from=backend /build/target/release/ring-server /usr/local/bin/ring-server
COPY --from=web /build/web/dist /app/web/dist
ENV RING_BIND=0.0.0.0:8765 RING_DATA_DIR=/data RING_WEB_DIR=/app/web/dist
USER ring
EXPOSE 8765
VOLUME ["/data"]
HEALTHCHECK --interval=30s --timeout=5s --start-period=15s CMD curl --fail --silent http://127.0.0.1:8765/health || exit 1
CMD ["ring-server"]
