# syntax=docker/dockerfile:1
# ── ironpad Dockerfile ────────────────────────────────────────────────────────
#
# Multi-stage build:
#   Stage 1 (builder): Compile the ironpad server + frontend WASM via cargo-leptos.
#   Stage 2 (runtime): Slim Rust toolchain (needed for cell compilation) + built artifacts.
#
# Build context is the repo root:
#   docker build -f docker/Dockerfile -t ironpad .

# ── Stage 1: Builder ─────────────────────────────────────────────────────────

FROM rust:1.93.0 AS builder

RUN rustup target add wasm32-unknown-unknown

RUN apt-get update && apt-get install -y --no-install-recommends \
    binaryen \
    && rm -rf /var/lib/apt/lists/*

RUN cargo install cargo-leptos wasm-bindgen-cli

WORKDIR /build
COPY . .

RUN cargo leptos build --release
RUN cargo build --release -p ironpad-cli
RUN cargo build --release -p ironpad-proxy

# ── Stage 2: Runtime ─────────────────────────────────────────────────────────

FROM rust:1.93.0-slim AS runtime

# The runtime needs the Rust toolchain + wasm target for compiling user cells.
RUN rustup target add wasm32-unknown-unknown

# rust-src is needed to rebuild std with atomics flags (for rayon cells).
RUN rustup component add rust-src

# Nightly toolchain + rust-src for pre-building the atomics sysroot at image build time.
# Cell compilation at runtime uses stable; it reuses the pre-built artifacts.
RUN rustup toolchain install nightly \
    && rustup target add wasm32-unknown-unknown --toolchain nightly \
    && rustup component add rust-src --toolchain nightly

# Copy tool binaries from builder (avoids recompiling / reinstalling in runtime).
COPY --from=builder /usr/local/cargo/bin/wasm-bindgen /usr/local/cargo/bin/wasm-bindgen
COPY --from=builder /usr/bin/wasm-opt /usr/bin/wasm-opt

# Pre-warm the cargo registry cache with ironpad-cell dependencies so that
# the first cell compile only downloads user-added deps.
COPY crates/ironpad-cell /tmp/warmup/crates/ironpad-cell
COPY docker/warmup-Cargo.toml /tmp/warmup/Cargo.toml
RUN mkdir -p /tmp/warmup/src && echo "" > /tmp/warmup/src/lib.rs \
    && cd /tmp/warmup \
    && cargo build --target wasm32-unknown-unknown --release 2>/dev/null || true \
    && rm -rf /tmp/warmup \
    && rm -rf /usr/local/cargo/registry/src

# Pre-build atomics sysroot for rayon-enabled cells.
# Uses nightly -Zbuild-std to compile std with atomics support; cell compilation
# at runtime points CARGO_TARGET_DIR here and reuses these artifacts via stable.
COPY crates/ironpad-cell /tmp/warmup-atomics/crates/ironpad-cell
COPY docker/warmup-Cargo.toml /tmp/warmup-atomics/Cargo.toml
RUN mkdir -p /tmp/warmup-atomics/src && echo "" > /tmp/warmup-atomics/src/lib.rs \
    && cd /tmp/warmup-atomics \
    && CARGO_TARGET_DIR=/warmup-cache/targets/atomics-shared \
       RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals" \
       cargo +nightly build --target wasm32-unknown-unknown --release \
       -Zbuild-std=std,panic_abort 2>/dev/null || true \
    && rm -rf /tmp/warmup-atomics

WORKDIR /app

# Copy the compiled binaries.
COPY --from=builder /build/target/release/ironpad-server /app/ironpad-server
COPY --from=builder /build/target/release/ironpad-cli /app/ironpad-cli
COPY --from=builder /build/target/release/ironpad-proxy /app/ironpad-proxy

# Copy the site assets (CSS, JS, WASM frontend bundle).
COPY --from=builder /build/target/site /app/target/site

# Copy the ironpad-cell crate (injected as path dep into user cells).
COPY crates/ironpad-cell /app/crates/ironpad-cell

# Copy the Leptos configuration so the server can find site assets.
COPY Cargo.toml /app/Cargo.toml

# Copy the entrypoint script.
COPY docker/entrypoint.sh /app/entrypoint.sh
RUN chmod +x /app/entrypoint.sh

ENV IRONPAD_DATA_DIR=/ironpad/data
ENV IRONPAD_CACHE_DIR=/ironpad/cache
ENV IRONPAD_PORT=3111
ENV IRONPAD_CELL_PATH=/app/crates/ironpad-cell
ENV IRONPAD_COMPILATION_PROXY=http://127.0.0.1:3112
ENV IRONPAD_PROXY_ALLOWLIST=crates.io,github.com,githubusercontent.com
ENV RUST_LOG=info
ENV LEPTOS_SITE_ROOT=/app/target/site

EXPOSE 3111

VOLUME ["/ironpad"]

CMD ["/app/entrypoint.sh"]
