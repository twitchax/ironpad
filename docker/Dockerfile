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
RUN cargo binstall -y cargo-chef cargo-leptos wasm-bindgen-cli@0.2.127

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
    clang \
    && rm -rf /var/lib/apt/lists/*

# Cell compilation runs on ONE pinned nightly — never the host default (keep in
# sync with `cell_toolchain` in crates/ironpad-app/src/compiler/build.rs;
# CELL_TOOLCHAIN lives in lib.rs, BROWSERPOD_TOOLCHAIN in docker/browserpod.env
# and installed below):
#
#   nightly-2026-05-19  every cell, whatever its features (CELL_TOOLCHAIN).
#                       Needs the wasm32 target, `rust-src` (for -Zbuild-std on
#                       rayon cells) and `enzyme` (matched libEnzyme/LLVM, for
#                       std::autodiff cells). Also the image's rustup default,
#                       so a stray bare cargo/rustc matches a cell build.
#
# This was FOUR nightlies until PRD-0067 (~2.9 GB of toolchains). Autodiff sets
# the ceiling and it is hard: July 2026 nightlies ICE with `incorrect autodiff
# typetree handling for slice`, and since `-Zautodiff=Enable` applies to the
# whole crate graph the ICE lands on a transitive dependency, so no cell-side
# workaround exists. BrowserPod sets the floor: its pack's `libexec/{rustc,cargo}`
# are symlinks into this exact nightly, whose 22 prebuilt rlibs are ABI-tied to
# it. Between the two sits one usable date, and because the pack installs it
# anyway, every other cell build rides along for the cost of three components.
#
# The two pins that went away were held for reasons that had expired without
# anyone re-testing them: rayon builds clean five months past the "newest
# nightly wasm-bindgen-rayon's atomics guard tolerates", and the workspace pin
# beneath it was still held for `thaw`, deleted in v0.12.13.
#
# The base image's stable toolchain is uninstalled at the end: nothing uses it,
# and it costs ~1 GB of image.
RUN rustup toolchain install nightly-2026-05-19 --profile minimal \
    && rustup target add wasm32-unknown-unknown --toolchain nightly-2026-05-19 \
    && rustup component add rust-src --toolchain nightly-2026-05-19 \
    && rustup component add enzyme --toolchain nightly-2026-05-19 \
    && rustup default nightly-2026-05-19

# Best-effort (own RUN so a failure can't mask the toolchain installs above):
# drop the base image's stable toolchain — nothing uses it after the default
# switch, and it costs ~1 GB.
RUN rustup toolchain uninstall stable 1.93.0 || true

# ── The second toolchain: BrowserPod (PRD-0066 Linux cells) ─────────────
#
# Installed from a VENDORED tarball, never `curl | sh` at image-build time.
# The upstream installer is a two-stage script that downloads from
# rt.browserpod.io, and a production build that can be stopped twenty minutes
# in by somebody else's CDN is a dependency worth spending 38MB of build
# context to remove. `cargo make vendor-browserpod` fetches and verifies it
# beforehand (a cheap, retryable, mirror-redirectable step), and
# `cargo make docker-build` runs that first. The RUNTIME still depends on that
# CDN — nothing here changes that, and nothing can.
#
# The digest is checked AGAIN here rather than trusted from the fetch: the
# whole point of vendoring is that this layer stops believing the network, and
# a build context is not a trust boundary.
#
# The pack installs as an ordinary rustup toolchain by being copied into
# $RUSTUP_HOME/toolchains (rustup discovers it by directory listing), and pulls
# its own pinned nightly underneath from static.rust-lang.org — the one
# registry this image already depends on. Since PRD-0067 that nightly is the
# one installed above, so this step finds it already present and adds nothing:
# `browserpod_pin_matches_cell_toolchain` fails the build if they ever drift. Its rustc wrapper bakes in
# RUST_TARGET_PATH, so `--target wasm32-browserpod-linux-musl` resolves by name
# with no `rustup target add`.
#
# Installed alongside: clang. The pack ships a C-compiler wrapper
# (`wasm32-browserpod-linux-musl-clang`) that shells out to the system clang,
# so a Linux cell whose dependency tree runs a `cc` build script for the pod
# target fails with "clang: not found" without it. Pure-Rust cells never
# invoke it, which is why this was deferred at first; it is ~350MB against the
# 2.14GB PRD-0067 took off the toolchains, and "your dependency happens to
# vendor C" is not a distinction a notebook author should have to know about.
COPY docker/browserpod.env docker/vendor/ /tmp/browserpod/
RUN set -eu; \
    . /tmp/browserpod/browserpod.env; \
    tarball="/tmp/browserpod/browserpod-rust-${BROWSERPOD_VERSION}.tar.gz"; \
    if [ ! -f "$tarball" ]; then \
        echo "ERROR: $tarball missing. Run: cargo make vendor-browserpod" >&2; \
        exit 1; \
    fi; \
    echo "${BROWSERPOD_SHA256}  $tarball" | sha256sum -c -; \
    tar -xzf "$tarball" -C /tmp/browserpod; \
    sh "/tmp/browserpod/browserpod-rust-${BROWSERPOD_VERSION}/install.sh"; \
    rustup toolchain list | grep -q "browserpod-${BROWSERPOD_VERSION}"; \
    rm -rf /tmp/browserpod

# Copy tool binaries from builder (avoids recompiling / reinstalling in runtime).
COPY --from=builder /usr/local/cargo/bin/wasm-bindgen /usr/local/cargo/bin/wasm-bindgen
COPY --from=builder /usr/bin/wasm-opt /usr/bin/wasm-opt

# Pre-warm the cargo registry cache AND the default cell target dir with the
# ironpad-cell dependency tree, on CELL_TOOLCHAIN (which since PRD-0067 is
# every cell): `build` artifacts make the first cell compile fast, `check`
# artifacts (distinct from build's) make live check-on-type warm from server
# boot (PRD-0045) — the warmth policy's "default deps are always warm" guarantee
# rests on this seed. The entrypoint copies it into the volume like the atomics
# sysroot. Autodiff cells no longer carry their own toolchain, but they still
# carry distinct RUSTFLAGS and a fat-LTO profile, which cargo fingerprints apart
# from these artifacts just as it used to by rustc version — so this plain
# warmup still does not cover them, and they are still warmed post-deploy by
# the runtime cache warmer instead. Warmups fail the image
# build LOUDLY — a `|| true` here once hid a manifest error for months, shipping
# images with no warmed cache and (via the entrypoint seed) crashing fresh
# deployments.
COPY crates/ironpad-cell /tmp/warmup/crates/ironpad-cell
COPY docker/warmup-Cargo.toml /tmp/warmup/Cargo.toml
RUN mkdir -p /tmp/warmup/src && echo "" > /tmp/warmup/src/lib.rs \
    && cd /tmp/warmup \
    && CARGO_TARGET_DIR=/warmup-cache/targets/default \
       cargo +nightly-2026-05-19 build --target wasm32-unknown-unknown --release \
    && CARGO_TARGET_DIR=/warmup-cache/targets/default \
       cargo +nightly-2026-05-19 check --target wasm32-unknown-unknown --release \
    && rm -rf /tmp/warmup \
    && rm -rf /usr/local/cargo/registry/src

# Pre-build the atomics sysroot for rayon-enabled cells. This needs its own
# target dir and RUSTFLAGS (a `-Zbuild-std` std rebuilt with +atomics is a
# different sysroot, not a different compiler), which is why it survives
# PRD-0067's collapse as a separate warmup even though it now shares
# CELL_TOOLCHAIN with the one above. The toolchain must MATCH what the runtime
# rayon build uses — a warmup on any other nightly produces artifacts the
# runtime build fingerprints as stale and silently rebuilds from scratch, which
# can blow the 300s build timeout on the 2-CPU machine.
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
       cargo +nightly-2026-05-19 build --target wasm32-unknown-unknown --release \
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
    rustc +nightly-2026-05-19 --version > /app/toolchain-versions; \
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
