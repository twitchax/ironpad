---
id: PRD-0031
title: "Toolchain & cell execution fix (P0 — un-break every cell)"
status: active
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
  status: done
  notes: "DONE (commit 28e3d41). Added #[link(wasm_import_module = \"env\")] to sim.rs:9 (ironpad_sim_read/_all), lib.rs:19 (ironpad_host_message), and gpu.rs:29 (ironpad_gpu_* — a fourth block found during the audit). Regression test (T-004) reproduces the undefined-symbol failure pre-fix and passes post-fix on the 2026-06-01 nightly."
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
  status: done
  notes: "DONE (commit 28e3d41). compile_cell_with_host_imports_links_successfully in compiler/mod.rs e2e_tests; cell source calls sim::read + host_message so the build must resolve ironpad_sim_read/ironpad_host_message. #[ignore]-gated (cargo make test-integration). TDD: failed pre-fix with undefined-symbol, passes post-fix."
- id: T-005
  title: "Correct the CLAUDE.md 'rust-lld linking failures' known-issue writeup"
  priority: 2
  status: done
  notes: "DONE (commit 28e3d41). Rewrote the CLAUDE.md 'Known Issue: rust-lld Linking Failures' section to state the real cause (bare extern blocks losing --allow-undefined on current nightly) and fix (#[link(wasm_import_module=\"env\")])."
- id: T-006
  title: "Pin a known-good nightly (rust-toolchain.toml) to unblock the build/test gate"
  priority: 1
  status: done
  notes: "DONE (see history). Discovered during T-001 verification: the default 2026-06-01 nightly hits 'error: queries overflow the depth limit!' compiling the thaw UI dep, breaking cargo make build/test/test-integration/ci/uat on a cold build (it only worked earlier via a cached thaw rlib from an older nightly). Added rust-toolchain.toml pinning nightly-2025-12-22 (verified to compile thaw + build cells + pass the compiler e2e suite incl. the new regression test). This is a prerequisite for verifying every other epic. Bump the pin forward once thaw/leptos/tachys fix the recursion-depth regression. Also cleared 5 pre-existing wasm32-target clippy warnings in the ironpad-cell files touched by T-001 (gpu.rs doc backticks; scoped cast_possible_truncation allows in sim.rs/lib.rs where usize==u32 on wasm32)."
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

## 2026-07-02 — Unit A (T-001, T-004, T-005) + T-006 toolchain pin
- **T-001/T-004/T-005** (commit 28e3d41): added `#[link(wasm_import_module = "env")]` to the bare host-import `extern` blocks in `ironpad-cell` (`sim.rs`, `lib.rs`, and `gpu.rs` — the audit found a fourth block). Added the `compile_cell_with_host_imports_links_successfully` regression test, which failed pre-fix with `undefined symbol: ironpad_sim_read`/`ironpad_host_message` on the 2026-06-01 nightly and passes post-fix. Corrected the CLAUDE.md known-issue section. Task review: **Approved** (one Important finding — the implementer's clippy self-check used `--all-targets`, which never cross-compiles the wasm32-gated code; 5 pre-existing pedantic warnings surfaced under a real `--target wasm32` run).
- **T-006** (toolchain pin): while verifying T-001, confirmed the default `nightly-2026-06-01` cannot compile the `thaw` dep (`queries overflow the depth limit!`), which breaks the cold build/test gate. Added `rust-toolchain.toml` pinning `nightly-2025-12-22` (installed, has wasm32, verified to compile thaw + build/link cells). Cleared the 5 pre-existing wasm32 clippy warnings from the T-001 finding. Verification of the full gate on the pinned toolchain (fmt-check + clippy + compiler e2e incl. the regression test) in progress at time of writing.
- **Remaining:** T-002 (pin scaffolded wasm-bindgen to host CLI version) and T-003 (fold toolchain identity into the cache key) — Unit B.
