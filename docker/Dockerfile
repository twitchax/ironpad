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

# Native-build essentials for cell dependencies' HOST build scripts: cargo
# build scripts compile for the host even when the cell targets wasm32, and
# crates like openssl-sys (reqwest via native-tls, e.g. in rtz-build) probe
# pkg-config for OpenSSL headers at that stage. Dev boxes have these; the slim
# image didn't, which made cells compile locally and fail only on prod.
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Cell compilation runs on three pinned toolchains — never the host default
# (keep in sync with `cell_toolchain` in crates/ironpad-app/src/compiler/build.rs:
# CELL_TOOLCHAIN in lib.rs, AUTODIFF_TOOLCHAIN + ATOMICS_TOOLCHAIN in build.rs):
#
#   nightly-2026-07-14  normal + SIMD cells (CELL_TOOLCHAIN). The common case,
#                       tracked fresh. Needs rust-src + the wasm32 target. Also
#                       the image's rustup default, so a stray bare cargo/rustc
#                       matches the common cell build.
#   nightly-2026-06-01  std::autodiff cells (AUTODIFF_TOOLCHAIN). Carries
#                       `enzyme` (matched libEnzyme/LLVM) + rust-src (for
#                       -Zbuild-std when autodiff+rayon combine); July 2026
#                       nightlies ICE on autodiff, so it stays back here.
#   nightly-2025-12-22  rayon/atomics cells (ATOMICS_TOOLCHAIN). wasm-bindgen-
#                       rayon's atomics guard breaks on newer nightlies.
#
# The base image's stable toolchain is uninstalled at the end: nothing uses it,
# and it costs ~1 GB of image.
RUN rustup toolchain install nightly-2026-07-14 --profile minimal \
    && rustup target add wasm32-unknown-unknown --toolchain nightly-2026-07-14 \
    && rustup component add rust-src --toolchain nightly-2026-07-14 \
    && rustup toolchain install nightly-2026-06-01 --profile minimal \
    && rustup target add wasm32-unknown-unknown --toolchain nightly-2026-06-01 \
    && rustup component add rust-src --toolchain nightly-2026-06-01 \
    && rustup component add enzyme --toolchain nightly-2026-06-01 \
    && rustup toolchain install nightly-2025-12-22 --profile minimal \
    && rustup target add wasm32-unknown-unknown --toolchain nightly-2025-12-22 \
    && rustup component add rust-src --toolchain nightly-2025-12-22 \
    && rustup default nightly-2026-07-14

# Best-effort (own RUN so a failure can't mask the toolchain installs above):
# drop the base image's stable toolchain — nothing uses it after the default
# switch, and it costs ~1 GB.
RUN rustup toolchain uninstall stable 1.93.0 || true

# Copy tool binaries from builder (avoids recompiling / reinstalling in runtime).
COPY --from=builder /usr/local/cargo/bin/wasm-bindgen /usr/local/cargo/bin/wasm-bindgen
COPY --from=builder /usr/bin/wasm-opt /usr/bin/wasm-opt

# Pre-warm the cargo registry cache AND the default cell target dir with the
# ironpad-cell dependency tree, on CELL_TOOLCHAIN (normal + SIMD cells, the
# common case): `build` artifacts make the first cell compile fast, `check`
# artifacts (distinct from build's) make live check-on-type warm from server
# boot (PRD-0045) — the warmth policy's "default deps are always warm" guarantee
# rests on this seed. The entrypoint copies it into the volume like the atomics
# sysroot. Autodiff cells carry their own toolchain (AUTODIFF_TOOLCHAIN) AND
# distinct RUSTFLAGS/profile, so this plain warmup never covered them — they are
# warmed post-deploy by the runtime cache warmer instead. Warmups fail the image
# build LOUDLY — a `|| true` here once hid a manifest error for months, shipping
# images with no warmed cache and (via the entrypoint seed) crashing fresh
# deployments.
COPY crates/ironpad-cell /tmp/warmup/crates/ironpad-cell
COPY docker/warmup-Cargo.toml /tmp/warmup/Cargo.toml
RUN mkdir -p /tmp/warmup/src && echo "" > /tmp/warmup/src/lib.rs \
    && cd /tmp/warmup \
    && CARGO_TARGET_DIR=/warmup-cache/targets/default \
       cargo +nightly-2026-07-14 build --target wasm32-unknown-unknown --release \
    && CARGO_TARGET_DIR=/warmup-cache/targets/default \
       cargo +nightly-2026-07-14 check --target wasm32-unknown-unknown --release \
    && rm -rf /tmp/warmup \
    && rm -rf /usr/local/cargo/registry/src

# Pre-build atomics sysroot for rayon-enabled cells with the SAME pinned
# toolchain runtime rayon builds use (ATOMICS_TOOLCHAIN) — a warmup on any
# other nightly produces artifacts the runtime build fingerprints as stale and
# silently rebuilds from scratch, which can blow the 300s build timeout on the
# 2-CPU machine.
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
       cargo +nightly-2025-12-22 build --target wasm32-unknown-unknown --release \
       -Zbuild-std=std,panic_abort \
    && rm -rf /tmp/warmup-atomics

WORKDIR /app

# Bake the toolchain's identity now, while it is being decided.
#
# The server needs `rustc --version` + `wasm-bindgen --version` for the compile
# cache key and the scaffold's pinned wasm-bindgen dependency. Asking rustc at
# runtime costs a cold exec that demand-pages ~350 MB of libLLVM +
# librustc_driver: measured at 5.9s of a 7.2s Fly cold start, to print a string
# that cannot change after this line. The file holds the RAW output of both
# commands and the server reads it back through the same parsers it uses on live
# output, so the derived fingerprint is byte-identical and cached cell blobs
# survive. A missing or malformed file falls back to probing, which is how dev
# boxes and CI (which never set this env var) already work.
# Fails the image build LOUDLY, like the warmups above: a silently truncated
# file degrades to probing, which is exactly the slow boot this removes, and
# nothing downstream would ever say so.
RUN set -e; \
    rustc +nightly-2026-07-14 --version > /app/toolchain-versions; \
    wasm-bindgen --version >> /app/toolchain-versions; \
    test "$(wc -l < /app/toolchain-versions)" = "2"; \
    cat /app/toolchain-versions
ENV IRONPAD_TOOLCHAIN_VERSIONS_FILE=/app/toolchain-versions

# Copy the compiled binaries.
COPY --from=builder /build/target/release/ironpad-server /app/ironpad-server
COPY --from=builder /build/target/release/ironpad-cli /app/ironpad-cli
COPY --from=builder /build/target/release/ironpad-proxy /app/ironpad-proxy

# Asset-hash manifest (cargo-leptos hash-files). Leptos reads it relative to
# the server binary at render time and PANICS if LEPTOS_HASH_FILES=true and
# it's missing — keep this copy in lockstep with the env below.
COPY --from=builder /build/target/release/hash.txt /app/hash.txt

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
ENV LEPTOS_HASH_FILES=true

EXPOSE 3111

VOLUME ["/ironpad"]

CMD ["/app/entrypoint.sh"]
