# syntax=docker/dockerfile:1

# writan-server — the optional self-hosted sync server (docs/SERVER.md,
# ADR-0007). It stores ciphertext blobs and nothing else: no keys, no
# plaintext, no titles.
#
#   docker build -t writan-server .
#   docker run -d --name writan-server -p 127.0.0.1:8787:8787 \
#     -v writan-data:/data writan-server
#
# Plain HTTP on purpose — put a TLS reverse proxy in front (docs/SERVER.md).

FROM rust:1.99-slim-bookworm AS build
WORKDIR /src

# Manifests first, so the dependency layers stay cached while the sources
# change. The whole workspace has to be present for cargo to resolve.
COPY Cargo.toml Cargo.lock ./
COPY crates/writan-core/Cargo.toml crates/writan-core/
COPY crates/writan-crypto/Cargo.toml crates/writan-crypto/
COPY crates/writan-sync/Cargo.toml crates/writan-sync/
COPY crates/writan-server/Cargo.toml crates/writan-server/
COPY app/src-tauri/Cargo.toml app/src-tauri/
COPY app-linux/Cargo.toml app-linux/

# Stub sources: warp the dependency cache with an empty build of the workspace
# members (writan-server's own dependencies — axum, rusqlite, writan-crypto —
# are the heavy part).
RUN set -eu; \
    mkdir -p crates/writan-core/src crates/writan-crypto/src crates/writan-sync/src \
             crates/writan-server/src app/src-tauri/src app-linux/src; \
    touch crates/writan-core/src/lib.rs crates/writan-crypto/src/lib.rs \
          crates/writan-sync/src/lib.rs app/src-tauri/src/lib.rs; \
    echo 'fn main() {}' > crates/writan-server/src/main.rs; \
    echo 'fn main() {}' > app-linux/src/main.rs; \
    cargo build --release --locked -p writan-server

# Real sources: only the workspace crates rebuild from here. COPY keeps the
# host mtimes, which can be older than the stub build, so touch the sources
# first — otherwise cargo considers the stubs still fresh.
COPY crates/writan-core crates/writan-core
COPY crates/writan-crypto crates/writan-crypto
COPY crates/writan-sync crates/writan-sync
COPY crates/writan-server crates/writan-server
RUN find crates -type f -name '*.rs' -exec touch {} + \
 && cargo build --release --locked -p writan-server

FROM debian:bookworm-slim AS runtime
RUN set -eu; \
    groupadd --system --gid 10001 writan; \
    useradd --system --uid 10001 --gid writan --home-dir /data writan; \
    mkdir -p /data; \
    chown writan:writan /data
COPY --from=build /src/target/release/writan-server /usr/local/bin/writan-server
ENV WRITAN_DATA_DIR=/data
VOLUME ["/data"]
EXPOSE 8787
USER writan
ENTRYPOINT ["/usr/local/bin/writan-server"]
CMD ["serve"]
