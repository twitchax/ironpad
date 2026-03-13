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

# ── Stage 2: Runtime ─────────────────────────────────────────────────────────

FROM rust:1.93.0-slim AS runtime

# The runtime needs the Rust toolchain + wasm target for compiling user cells.
RUN rustup target add wasm32-unknown-unknown

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

WORKDIR /app

# Copy the compiled server binary.
COPY --from=builder /build/target/release/ironpad-server /app/ironpad-server

# Copy the site assets (CSS, JS, WASM frontend bundle).
COPY --from=builder /build/target/site /app/target/site

# Copy the ironpad-cell crate (injected as path dep into user cells).
COPY crates/ironpad-cell /app/crates/ironpad-cell

# Copy the Leptos configuration so the server can find site assets.
COPY Cargo.toml /app/Cargo.toml

ENV IRONPAD_DATA_DIR=/ironpad/data
ENV IRONPAD_CACHE_DIR=/ironpad/cache
ENV IRONPAD_PORT=3111
ENV IRONPAD_CELL_PATH=/app/crates/ironpad-cell
ENV RUST_LOG=info
ENV LEPTOS_SITE_ROOT=/app/target/site

EXPOSE 3111

VOLUME ["/ironpad"]

CMD ["/app/ironpad-server"]
