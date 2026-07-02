---
id: PRD-0031
title: "Toolchain & cell execution fix (P0 — un-break every cell)"
status: draft
owner: "Aaron Roney"
created: 2026-07-02
updated: 2026-07-02

depends_on:
- PRD-0030

principles:
- "Cells must compile, link, and run on the current nightly + host wasm-bindgen CLI"
- "Toolchain identity belongs in the cache key — stale artifacts must not be served"
- "Add a regression test that would have caught this class of break"

references:
- name: "Review report — sections P0-1, P0-2"
  url: reviews/2026-07-02-codebase-review.md
- name: "wasm-bindgen schema-version matching"
  url: https://github.com/wasm-bindgen/wasm-bindgen

acceptance_tests:
- id: uat-001
  name: "A cell calling sim::read + host_message compiles, links (no undefined-symbol), and runs"
  command: cargo make test-integration
  uat_status: unverified
- id: uat-002
  name: "Cache key changes when the wasm-bindgen CLI / rustc / ironpad-cell sources change (unit test)"
  command: cargo make test
  uat_status: unverified
- id: uat-003
  name: "Running the Lagrange Points and Fractal Tree public notebooks produces live-sim output, no link error"
  command: cargo make playwright
  uat_status: unverified

tasks:
- id: T-001
  title: "Add wasm_import_module = \"env\" to bare extern blocks in ironpad-cell"
  priority: 1
  status: todo
  notes: "crates/ironpad-cell/src/sim.rs:8-22 (ironpad_sim_read, ironpad_sim_read_all) and crates/ironpad-cell/src/lib.rs:18-22 (ironpad_host_message) are plain extern \"C\" blocks; current nightly rust-lld no longer passes --allow-undefined so they fail with 'undefined symbol'. Executor supplies these under imports.env (public/executor-core.js:662,719). Add #[link(wasm_import_module = \"env\")] to each block. Audit crates/ironpad-cell/src/gpu.rs for the same pattern."
- id: T-002
  title: "Pin scaffolded wasm-bindgen to the exact host CLI version"
  priority: 1
  status: todo
  notes: "scaffold.rs:123 injects floating wasm-bindgen = \"0.2\" while build.rs:182 shells out to the fixed host `wasm-bindgen` CLI; schema mismatch breaks all cells in both directions. Read `wasm-bindgen --version` once at startup and inject an exact `=X.Y.Z` pin into the generated Cargo.toml. Consider the same for wasm-bindgen-futures if it participates in the schema."
- id: T-003
  title: "Fold toolchain identity into the blake3 cache key"
  priority: 1
  status: todo
  notes: "cache.rs:29-62 omits rustc version, wasm-bindgen CLI version, and ironpad-cell source contents, so stale/incompatible blobs are served until CACHE_EPOCH is bumped by hand (CLAUDE.md pitfall). Fold `rustc --version`, `wasm-bindgen --version`, and a hash of the ironpad-cell crate sources into content_hash. Overlaps PRD-0036 T-? (cache correctness) — do this one here."
- id: T-004
  title: "Add an integration test that compiles a sim/host_message cell"
  priority: 1
  status: todo
  notes: "New #[ignore]-gated test in compiler/mod.rs e2e_tests (run via cargo make test-integration). Compile a cell that calls ironpad_cell sim::read and host_message so CI catches link-level toolchain rot (the exact failure this PRD fixes). Assert a valid WASM blob is produced."
- id: T-005
  title: "Correct the CLAUDE.md 'rust-lld linking failures' known-issue writeup"
  priority: 2
  status: todo
  notes: "The 'Known Issue: rust-lld Linking Failures' section attributes cell link failures to a missing rust-lld component; the real cause (for sim/host_message cells) is bare extern blocks losing --allow-undefined. Update the diagnosis so future agents don't chase the wrong fix."
---

# Summary

Two independent toolchain issues broke every cell run (and every live-simulation demo) at review time. This epic fixes both at the source, folds toolchain identity into the compile cache so the breakage can't silently persist, and adds a regression test.

# Problem

1. **Bare `extern "C"` imports fail to link.** `ironpad-cell` declares its host-import functions (`ironpad_sim_read`, `ironpad_sim_read_all`, `ironpad_host_message`) in plain `extern "C"` blocks. Current nightly `rust-lld` no longer defaults to `--allow-undefined` for `wasm32-unknown-unknown`, so any cell using sim/host_message features fails with `undefined symbol: ironpad_sim_read`. The UI surfaces only `linking with rust-lld failed`, with the real cause hidden under an unrelated "mutable static" *warning*.

2. **wasm-bindgen version drift.** The scaffold injects floating `wasm-bindgen = "0.2"` while the build shells out to a fixed host CLI. wasm-bindgen requires an exact schema match, so cells break whenever the resolved crate and the CLI drift — and because the cache key includes neither version (nor rustc, nor the injected `ironpad-cell` sources), the breakage is sticky and invisible.

# Goals

1. Cells using sim/LiveView/host_message features link cleanly on current nightly.
2. Scaffolded cells always resolve the exact wasm-bindgen version the host CLI expects.
3. The compile cache invalidates automatically on any toolchain or runtime change.
4. A regression test catches this failure class in CI.

# Technical Approach

## T-001: `wasm_import_module`

Annotate each host-import block:

```rust
#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    fn ironpad_sim_read(key_ptr: *const u8, key_len: u32) -> u32;
    // ...
}
```

The executor registers these under `imports.env` (`public/executor-core.js:662,719`), so `"env"` is the correct module name. Apply to `sim.rs`, `lib.rs`, and audit `gpu.rs`.

## T-002 / T-003: version pinning + cache key

Read the CLI version at startup (`wasm-bindgen --version` → parse `X.Y.Z`), inject `wasm-bindgen = "=X.Y.Z"` into the scaffolded `Cargo.toml`, and add `rustc --version`, that CLI version string, and a hash of the `ironpad-cell` crate sources to `content_hash` in `cache.rs`. This ties the cached blob's identity to the toolchain that produced it.

## T-004: regression test

Add an integration test (mirroring existing `compiler/mod.rs` e2e tests) that scaffolds and builds a cell exercising `sim::read`/`host_message`, asserting a valid WASM artifact — so a future toolchain bump that reintroduces the link failure fails CI instead of shipping.

# Assumptions

- The host has a working `wasm-bindgen` CLI (the repo's `cargo make install-tools` installs it).
- `imports.env` remains the module the executor registers host functions under.

# Constraints

- Pinning must use the CLI version actually present at runtime, not a hardcoded constant (that would just move the drift problem).
- Cache-key changes invalidate all existing blobs on first deploy (expected, one-time recompile).

# References to Code

- `crates/ironpad-cell/src/sim.rs:8-22`, `crates/ironpad-cell/src/lib.rs:18-22`, `crates/ironpad-cell/src/gpu.rs`
- `crates/ironpad-app/src/compiler/scaffold.rs:123`
- `crates/ironpad-app/src/compiler/build.rs:182`
- `crates/ironpad-app/src/compiler/cache.rs:29-62`
- `crates/ironpad-app/src/compiler/mod.rs` (e2e_tests)

# Non-Goals (MVP)

- Supporting multiple simultaneous wasm-bindgen versions.
- Reworking the compile pipeline beyond version pinning and the cache key.

# History

(Entries appended during implementation go below this line.)
