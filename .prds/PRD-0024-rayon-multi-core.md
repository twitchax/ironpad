---
id: PRD-0024
title: "Rayon Multi-Core Parallelism for Cells"
status: done
owner: "Aaron Roney"
created: 2026-03-19
updated: 2026-03-19

depends_on:
- PRD-0013

principles:
- "Cell authors get full rayon par_iter with zero friction — just add rayon to deps"
- "Non-rayon cells pay zero cost — atomics compilation is opt-in via automatic detection"
- "Pre-built std sysroot eliminates the 30-60s -Zbuild-std penalty"
- "COOP/COEP headers are always on — verified safe with all bundled resources"

references:
- name: "Rayon multi-core research doc"
  url: docs/rayon-multi-core-research.md
- name: "wasm-bindgen-rayon"
  url: https://github.com/nickhobbs94/nickhobbs94.github.io/tree/master
- name: "SharedArrayBuffer security requirements (MDN)"
  url: https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/SharedArrayBuffer#security_requirements
- name: "COOP/COEP spike results"
  url: "Verified 2026-03-19: crossOriginIsolated=true, SharedArrayBuffer available, Monaco fully functional"

acceptance_tests:
- id: uat-001
  name: "crossOriginIsolated is true in the browser"
  command: cargo make playwright
  uat_status: unverified
- id: uat-002
  name: "Non-rayon cells compile and run identically to before (no regressions)"
  command: cargo make test
  uat_status: verified
- id: uat-003
  name: "A cell using rayon par_iter compiles and executes correctly"
  command: cargo make playwright
  uat_status: unverified
- id: uat-004
  name: "Rayon cell cache key differs from non-rayon cell with same source"
  command: cargo make test
  uat_status: verified
- id: uat-005
  name: "Docker image pre-warms atomics sysroot and rayon cells compile without -Zbuild-std"
  command: cargo make docker-build
  uat_status: unverified

tasks:
- id: T-001
  title: "Add COOP/COEP response headers"
  priority: 1
  status: done
  notes: "Add SetResponseHeaderLayer middleware in ironpad-server/src/main.rs after .with_state(). Headers: Cross-Origin-Opener-Policy: same-origin, Cross-Origin-Embedder-Policy: require-corp. Use axum::http::{HeaderName, HeaderValue} + tower_http::set_header::SetResponseHeaderLayer (already in deps with full features). Add unit test verifying headers are present."

- id: T-002
  title: "Detect rayon in merged dependencies and thread needs_atomics through pipeline"
  priority: 1
  status: done
  notes: "In compiler/scaffold.rs, after merge_dependencies(), check if merged deps contain 'rayon' (via crate_name_from_dep_line). Thread a needs_atomics: bool through build_micro_crate(), check_micro_crate(), content_hash(), and optimize(). Update all call sites in server_fns.rs and mod.rs. In cache.rs, add marker byte 0x03 + 'atomics=1'/'atomics=0' to blake3 hash input."

- id: T-003
  title: "Conditional RUSTFLAGS and target dir routing in build.rs"
  priority: 1
  status: done
  notes: "When needs_atomics is true: (1) set RUSTFLAGS='-C target-feature=+atomics,+bulk-memory,+mutable-globals' on the cargo subprocess, (2) route CARGO_TARGET_DIR to {cache_dir}/targets/atomics-shared instead of per-session dir. The shared atomics target dir contains pre-built std (from Docker warmup or local warmup task). Same changes needed for check_micro_crate. Update target_dir() helper to accept needs_atomics param."

- id: T-004
  title: "Add wasm-bindgen-rayon to ironpad-cell and re-export in prelude"
  priority: 1
  status: done
  notes: "Add wasm-bindgen-rayon as a dependency in ironpad-cell/Cargo.toml. Re-export rayon::prelude::* in the cell prelude (cfg(target_arch = 'wasm32')). Also export pub use wasm_bindgen_rayon::init_thread_pool which wasm-bindgen-rayon requires the WASM module to re-export."

- id: T-005
  title: "Thread pool initialization in executor.js"
  priority: 2
  status: done
  notes: "After wasm-bindgen WASM module init (loadBlob), check if the module exports initThreadPool (wasm-bindgen-rayon's convention). If so, call it with navigator.hardwareConcurrency. This spawns sub-Workers from within the cell's Web Worker. Handle the case where initThreadPool is absent (non-rayon cells) gracefully."

- id: T-006
  title: "Add --enable-threads to wasm-opt for atomics cells"
  priority: 2
  status: done
  notes: "In compiler/optimize.rs, when needs_atomics is true, add '--enable-threads' flag to the wasm-opt command. Verify binaryen version supports this flag. Make optimization non-fatal (already best-effort)."

- id: T-007
  title: "Docker pre-build atomics sysroot"
  priority: 2
  status: done
  notes: "In docker/Dockerfile runtime stage: (1) add 'rustup component add rust-src', (2) after existing warmup, run a second warmup with RUSTFLAGS='-C target-feature=+atomics,+bulk-memory,+mutable-globals' and -Zbuild-std=std,panic_abort targeting the atomics-shared target dir. Add rayon = '1' to docker/warmup-Cargo.toml. This pre-compiles std with atomics so cells don't pay the 30-60s penalty."

- id: T-008
  title: "Local dev warmup task in Makefile.toml"
  priority: 2
  status: done
  notes: "Add a 'warmup-atomics' cargo-make task that runs the same -Zbuild-std build locally, targeting {cache_dir}/targets/atomics-shared. One-time 30-60s cost, then cached. Document in DEVELOPMENT.md."

- id: T-009
  title: "Update Mandelbrot notebook to use rayon par_iter"
  priority: 3
  status: done
  notes: "Update public/notebooks/mandelbrot.ironpad to use rayon::prelude::par_iter for pixel computation. Add rayon = '1' to the notebook's Cargo.toml. This serves as the showcase for multi-core execution."

- id: T-010
  title: "Tests and documentation"
  priority: 3
  status: done
  notes: "Add unit tests for: rayon detection in scaffold, cache key differentiation with/without atomics, RUSTFLAGS injection in build. Add integration test for a rayon cell compile (if feasible without full browser). Update DEVELOPMENT.md with rayon/multi-core section."
---

# Summary

Enable multi-core CPU parallelism for ironpad notebook cells via `wasm-bindgen-rayon`. Cell authors write standard `rayon::par_iter()` code; the compiler pipeline automatically detects rayon usage, enables WASM atomics, and routes builds through a pre-warmed sysroot to eliminate compile-time overhead.

# Problem

Compute-heavy algorithms (Mandelbrot, ray marching, physics simulations) currently run on a single WASM thread. Modern browsers expose 4-16+ CPU cores that go unused. Users writing parallel-friendly code can't leverage multi-core execution.

# Goals

1. Cell authors can `use rayon::prelude::*` and call `par_iter()` with zero additional configuration
2. Non-rayon cells compile and execute identically — no performance regression
3. First rayon cell compilation is fast (no 30-60s std rebuild) thanks to pre-built sysroot
4. `SharedArrayBuffer` and `crossOriginIsolated` are available in the browser

# Technical Approach

## Architecture

```
Browser (crossOriginIsolated = true)
  └─ Web Worker (cell executor, from PRD-0013)
       └─ WASM module (compiled with +atomics)
            └─ rayon thread pool (sub-Workers via SharedArrayBuffer)
                 ├─ Thread 1
                 ├─ Thread 2
                 └─ Thread N (navigator.hardwareConcurrency)
```

## COOP/COEP Headers

All server responses include:
- `Cross-Origin-Opener-Policy: same-origin`
- `Cross-Origin-Embedder-Policy: require-corp`

This enables `window.crossOriginIsolated = true` and unlocks `SharedArrayBuffer`. **Spike verified** (2026-03-19): Monaco, KaTeX, and all bundled resources work correctly since everything is same-origin.

## Automatic Rayon Detection

The compiler pipeline detects `rayon` in merged cell dependencies (via `crate_name_from_dep_line()` after `merge_dependencies()`). When detected, `needs_atomics = true` flows through:

1. **Cache key** — includes atomics flag so rayon/non-rayon builds get separate entries
2. **Build command** — injects `RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals"`
3. **Target directory** — routes to shared `targets/atomics-shared/` (pre-warmed with std)
4. **Wasm-opt** — adds `--enable-threads` flag

## Pre-Built Std Sysroot

The main cost of WASM atomics is rebuilding std with `-Zbuild-std=std,panic_abort` (30-60s). This is eliminated by pre-building once:

- **Docker**: Warmup stage builds a dummy crate with atomics flags, caching std in `targets/atomics-shared/`
- **Local**: `cargo make warmup-atomics` one-time task

At cell-compile time, `-Zbuild-std` is NOT passed — std is already in the shared target dir.

## Thread Pool Initialization

In `executor.js`, after loading a cell's WASM module via wasm-bindgen, check for the `initThreadPool` export (injected by `wasm-bindgen-rayon`). If present, call `initThreadPool(navigator.hardwareConcurrency)` which spawns sub-Workers using `SharedArrayBuffer`.

# Assumptions

- PRD-0013 (Web Worker Cell Execution) is complete — cells execute in Workers
- All static resources (Monaco, KaTeX, Sortable) remain same-origin bundled
- `wasm-bindgen-rayon` is compatible with wasm-bindgen 0.2.x (used in ironpad-cell)
- Binaryen (wasm-opt) supports `--enable-threads` flag

# Constraints

- COOP/COEP headers prevent loading cross-origin subresources without CORP headers — all resources must remain same-origin or include appropriate headers
- Shared atomics target dir serializes concurrent rayon cell builds (cargo file locks) — acceptable since rayon cells are uncommon
- Nightly Rust features (`-Zbuild-std`) required for Docker/local sysroot pre-build — but NOT at cell-compile time
- The pre-built std must match the exact Rust toolchain version (pinned to 1.93.0)

# References to Code

| File | Role |
|------|------|
| `crates/ironpad-server/src/main.rs` | COOP/COEP middleware (lines 51-71) |
| `crates/ironpad-app/src/compiler/scaffold.rs` | Rayon detection after `merge_dependencies()` (line 159) |
| `crates/ironpad-app/src/compiler/build.rs` | RUSTFLAGS injection + target dir routing (lines 86-118) |
| `crates/ironpad-app/src/compiler/cache.rs` | Cache key with atomics marker (lines 22-46) |
| `crates/ironpad-app/src/compiler/optimize.rs` | `--enable-threads` flag (lines 45-51) |
| `crates/ironpad-cell/src/lib.rs` | Prelude re-exports (lines 57-86) |
| `crates/ironpad-cell/Cargo.toml` | Add wasm-bindgen-rayon dep |
| `public/executor.js` | Thread pool init after WASM load (lines 174-281) |
| `docker/Dockerfile` | Atomics sysroot warmup (after line 49) |
| `docker/warmup-Cargo.toml` | Add rayon dep for warmup |
| `Makefile.toml` | Add warmup-atomics task |
| `docs/rayon-multi-core-research.md` | Full design research |

# Non-Goals (MVP)

- Custom thread count per cell (always `navigator.hardwareConcurrency`)
- Rayon lite / message-passing fallback (Approach 2 from research doc)
- Automatic parallelism (users must opt-in via rayon dependency)
- WebGPU compute integration
- Benchmarking infrastructure for parallel vs sequential comparison

# History

- 2026-03-19: PRD created. COOP/COEP spike verified — all resources load correctly under cross-origin isolation. Monaco bridge fully functional with `crossOriginIsolated: true` and `SharedArrayBuffer` available.

## 2026-03-19 — Full Implementation (T-001 through T-010)

- **Tasks completed**: T-001, T-002, T-003, T-004, T-005, T-006, T-007, T-008, T-009, T-010
- **Changes**:
  - T-001: COOP/COEP headers via `SetResponseHeaderLayer` in ironpad-server/src/main.rs
  - T-002: Rayon detection in scaffold.rs, `needs_atomics` threaded through cache/build/optimize/server_fns
  - T-003: RUSTFLAGS injection + `atomics_target_dir()` routing in build.rs
  - T-004: wasm-bindgen-rayon + rayon deps in ironpad-cell, prelude re-exports
  - T-005: `initThreadPool(navigator.hardwareConcurrency)` in executor.js
  - T-006: `--enable-threads` flag in wasm-opt for atomics cells
  - T-007: Nightly toolchain + atomics sysroot warmup in Dockerfile
  - T-008: `warmup-atomics` cargo-make task in Makefile.toml
  - T-009: Mandelbrot notebook updated with rayon `par_iter` + multi-core section
  - T-010: All 440 tests pass, DEVELOPMENT.md rayon section added
- **Test results**: 440 passed, 6 skipped (same as baseline), clippy clean
- **UATs verified**: uat-002 (no regressions), uat-004 (cache key differentiation)
- **UATs deferred**: uat-001 (requires browser), uat-003 (requires wasm32 atomics build), uat-005 (requires Docker build)
- **Constitution compliance**: No violations
