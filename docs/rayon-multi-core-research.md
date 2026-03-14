# Multi-Core WASM via wasm-bindgen-rayon

Research notes and design exploration for adding multi-core parallelism to ironpad cells via `rayon` and `wasm-bindgen-rayon`. This is a **future enhancement** that builds on the Web Worker cell execution foundation (see PRD-0013).

---

## Overview

Currently, WASM cells execute on a single thread. Compute-heavy algorithms (Mandelbrot, ray marching, simulations) can't use multiple CPU cores. `wasm-bindgen-rayon` enables `rayon`'s parallel iterators (`par_iter`, `par_chunks`, etc.) inside WASM by spawning sub-Workers backed by `SharedArrayBuffer`.

---

## Architecture

With Web Worker execution (PRD-0013) as the foundation, rayon adds a thread pool **inside** the cell's Worker:

```
Web Worker (main cell executor)
┌───────────────────────────────────────┐
│ executor-worker.js                     │
│                                        │
│  WASM module with rayon thread pool    │
│  ┌─────────┐ ┌─────────┐ ┌─────────┐ │
│  │ Thread 1 │ │ Thread 2 │ │ Thread N │ │
│  │ (Worker) │ │ (Worker) │ │ (Worker) │ │
│  └─────────┘ └─────────┘ └─────────┘ │
│                                        │
│  SharedArrayBuffer (requires COOP/COEP)│
└───────────────────────────────────────┘
```

---

## Prerequisites

- **Web Worker cell execution** (PRD-0013) must be complete first.
- **COOP/COEP headers** on all server responses (required for `SharedArrayBuffer`):
  - `Cross-Origin-Opener-Policy: same-origin`
  - `Cross-Origin-Embedder-Policy: require-corp`
  - `Cross-Origin-Resource-Policy: same-origin`
  - Add via Axum middleware in `crates/ironpad-server/src/main.rs`.
  - **Risk**: May break cross-origin resource loading (Monaco, external assets). Needs careful testing.

---

## Implementation Plan

### 1. COOP/COEP Headers

Add headers as Axum middleware. Test that all resources (Monaco editor, executor scripts, etc.) still load correctly under the stricter policy.

### 2. ironpad-cell Dependency

Add `wasm-bindgen-rayon` to `crates/ironpad-cell/Cargo.toml`. Export `rayon::prelude::*` in the cell prelude (`crates/ironpad-cell/src/lib.rs`, prelude module). Cells should be able to use `par_iter()` without extra imports.

### 3. Conditional Atomics in Compiler Pipeline

**Key insight**: Atomics compilation has a significant one-time cost (~30-60s) due to `-Zbuild-std=std,panic_abort` rebuilding the standard library. Normal cells should **not** pay this cost.

**Approach**: Detect rayon usage at scaffold time and conditionally enable atomics.

- **Detection**: After `merge_dependencies()` in `scaffold.rs`, check `merged_deps.contains_key("rayon")`.
- **Flag injection**: In `build.rs`, set `RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals"` only when `needs_atomics` is true. Also pass `-Zbuild-std=std,panic_abort`.
- **Cache isolation**: Include `needs_atomics` in the blake3 hash input so atomics and non-atomics builds get separate cache entries.
- **User experience**: Zero friction — just add `rayon` to cell deps, atomics are enabled automatically.

### 4. Thread Pool Initialization

In the Worker context (`executor-worker.js`), call `wasm-bindgen-rayon`'s `initThreadPool()` after loading a cell's WASM module. Thread count defaults to `navigator.hardwareConcurrency`.

### 5. Update Public Notebooks

Update Mandelbrot and Julia set notebooks to use `rayon::par_iter()` for pixel computation, rendering to the `Canvas` type. Add progress bar support.

---

## Compile-Time Impact

| Flag               | Compile Time                            | Runtime Impact                                  | Binary Size     |
| ------------------ | --------------------------------------- | ----------------------------------------------- | --------------- |
| `+atomics`         | ~5-10% slower codegen                   | ~2-10% overhead (atomic ops)                    | Slightly larger |
| `+bulk-memory`     | Negligible                              | **Faster** (native `memory.copy`/`memory.fill`) | Neutral         |
| `+mutable-globals` | Negligible                              | Negligible                                      | Negligible      |
| `-Zbuild-std`      | **+30-60s** (first build, cached after) | N/A                                             | N/A             |

**With conditional detection**: Normal cells are completely unaffected. Only cells with `rayon` in their deps pay the cost.

| Scenario                       | Current (~3-8s) | With Atomics (rayon cell) |
| ------------------------------ | --------------- | ------------------------- |
| Cold (first cell, std rebuild) | ~3-8s           | ~35-65s (one-time)        |
| Warm (std cached, cell miss)   | ~3-8s           | ~3.5-9s (+5-10%)          |
| Hot (cache hit)                | ~0s             | ~0s (unchanged)           |

---

## Constraints & Risks

- **COOP/COEP headers** restrict cross-origin resource loading. Monaco and external resources must be same-origin or include CORP headers.
- **`-Zbuild-std`** requires nightly Rust (already used by ironpad).
- **wasm-opt compatibility**: Need to verify that wasm-opt `-O3` works with atomics-enabled WASM output.
- **Sub-Workers of Workers**: rayon spawns sub-Workers from the main cell Worker. Some browsers may have limitations on nested Workers.
- **`Worker.terminate()` interaction**: If the user cancels a rayon-enabled cell, the main Worker is terminated — but the rayon sub-Workers may linger. Need to verify they're cleaned up.

---

## References

- [wasm-bindgen-rayon](https://github.com/nickhobbs94/nickhobbs94.github.io/tree/master)
- [SharedArrayBuffer security requirements (MDN)](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/SharedArrayBuffer#security_requirements)
- [COOP/COEP explainer](https://web.dev/cross-origin-isolation-guide/)

---

## Code References

| File                                          | Relevance                                                       |
| --------------------------------------------- | --------------------------------------------------------------- |
| `crates/ironpad-server/src/main.rs`           | Add COOP/COEP middleware (~line 61-73)                          |
| `crates/ironpad-cell/src/lib.rs`              | Add rayon prelude exports (lines 44-72)                         |
| `crates/ironpad-cell/Cargo.toml`              | Add wasm-bindgen-rayon dependency                               |
| `crates/ironpad-app/src/compiler/scaffold.rs` | Detect rayon in deps, `merge_dependencies()` (line 162)         |
| `crates/ironpad-app/src/compiler/build.rs`    | Conditionally set RUSTFLAGS (lines 63-74)                       |
| `crates/ironpad-app/src/compiler/cache.rs`    | Include atomics flag in cache key                               |
| `crates/ironpad-app/src/compiler/optimize.rs` | Verify wasm-opt compat with atomics                             |
| `public/executor-worker.js`                   | Call `initThreadPool()` after WASM load (future, from PRD-0013) |
