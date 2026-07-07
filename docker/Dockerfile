# syntax=docker/dockerfile:1
# ── ironpad Dockerfile ────────────────────────────────────────────────────────
#
# Multi-stage build:
#   Stage 1a (planner): Generate cargo-chef recipe for dependency caching.
#   Stage 1b (builder): Cook deps (cached), then compile ironpad.
#   Stage 2  (runtime): Slim Rust toolchain (needed for cell compilation) + built artifacts.
#
# Build context is the repo root:
#   docker build -f docker/Dockerfile -t ironpad .

# ── Stage 1a: Planner (cargo-chef recipe) ────────────────────────────────

FROM rust:1.93.0 AS planner

# Install cargo-binstall, then fetch the prebuilt cargo-chef binary (no from-source build).
RUN curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash
RUN cargo binstall -y cargo-chef

WORKDIR /build
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 1b: Builder ────────────────────────────────────────────────────

FROM rust:1.93.0 AS builder

RUN rustup target add wasm32-unknown-unknown

RUN apt-get update && apt-get install -y --no-install-recommends \
    binaryen \
    && rm -rf /var/lib/apt/lists/*

# Install cargo-binstall, then fetch prebuilt tool binaries (avoids slow from-source builds).
# wasm-bindgen-cli is pinned to the workspace's locked wasm-bindgen crate version
# (kept in sync with Makefile.toml); a mismatch fails the frontend WASM post-processing.
RUN curl -L --proto '=https' --tlsv1.2 -sSf https://raw.githubusercontent.com/cargo-bins/cargo-binstall/main/install-from-binstall-release.sh | bash
RUN cargo binstall -y cargo-chef cargo-leptos wasm-bindgen-cli@0.2.114

WORKDIR /build

# Cook dependencies first (cached until Cargo.toml/Cargo.lock change).
COPY --from=planner /build/recipe.json recipe.json
# Bring in the toolchain pin BEFORE cooking so deps are compiled with the same
# toolchain (nightly, per rust-toolchain.toml) the app is built with below. Without
# this, cargo-chef cooks on the base-image stable toolchain, then the nightly build
# rebuilds everything from scratch and the dependency-cache layer is wasted.
COPY rust-toolchain.toml rust-toolchain.toml
RUN cargo chef cook --release --recipe-path recipe.json

# Now copy source and build (only recompiles our code).
COPY . .

RUN cargo leptos build --release
RUN cargo build --release -p ironpad-cli
RUN cargo build --release -p ironpad-proxy

# ── Stage 2: Runtime ─────────────────────────────────────────────────────

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
COPY docker/warmup-atomics-Cargo.toml /tmp/warmup-atomics/Cargo.toml
RUN mkdir -p /tmp/warmup-atomics/src && echo "" > /tmp/warmup-atomics/src/lib.rs \
    && cd /tmp/warmup-atomics \
    && CARGO_TARGET_DIR=/warmup-cache/targets/atomics-shared \
       RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals \
                  -C link-arg=--shared-memory \
                  -C link-arg=--max-memory=2147483648 \
                  -C link-arg=--import-memory \
                  -C link-arg=--export=__wasm_init_tls \
                  -C link-arg=--export=__tls_size \
                  -C link-arg=--export=__tls_align \
                  -C link-arg=--export=__tls_base" \
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
