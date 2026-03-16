# Multi-Core WASM Parallelism for ironpad Cells

Research notes and design exploration for adding multi-core parallelism to ironpad cells. This is a **future enhancement** that builds on the Web Worker cell execution foundation (see PRD-0013).

---

## Overview

Currently, WASM cells execute on a single thread. Compute-heavy algorithms (Mandelbrot, ray marching, simulations) can't use multiple CPU cores. This document explores two approaches:

1. **`wasm-bindgen-rayon`** (recommended) — Full rayon `par_iter` inside WASM via `SharedArrayBuffer` + atomics, with a pre-built std sysroot to eliminate compile-time overhead.
2. **Custom "rayon lite"** (alternative) — Message-passing worker pool using `postMessage`, avoiding all `SharedArrayBuffer`/COOP/COEP requirements at the cost of ergonomics.

After analysis, **Approach 1 (rayon with pre-built std) is recommended** because pre-building std with atomics in Docker eliminates the main practical objection (compile-time cost), and users get battle-tested `par_iter` with closures, captures, and `reduce` for free.

---

## OSS Landscape

Before detailing the approaches, a survey of existing WASM parallelism libraries:

| Library | Mechanism | Needs SharedArrayBuffer | Needs `-Zbuild-std` | Closures | Notes |
|---|---|---|---|---|---|
| **`wasm-bindgen-rayon`** | Shared memory threads | Yes | Yes | Yes | Full rayon API. Battle-tested. |
| **`wasm_thread`** | `std::thread::spawn` for WASM | Yes | Yes | Yes | Lower-level than rayon. Same prerequisites. |
| **`gloo-worker`** | `postMessage` typed workers | No | No | No (message-passing) | Designed for app-level (Yew/Leptos), not cell-internal use. |
| **`wasm_bindgen_futures`** | `spawn_local` | No | No | Yes | Concurrency only (single-thread event loop). No CPU parallelism. |

**Key finding**: Every library that provides *true* CPU parallelism in WASM requires the same stack: `SharedArrayBuffer` + COOP/COEP headers + `-Zbuild-std` with atomics. There is no existing OSS crate for "WASM par_iter without SharedArrayBuffer."

---

# Approach 1: `wasm-bindgen-rayon` with Pre-Built Std (Recommended)

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

## Prerequisites

- **Web Worker cell execution** (PRD-0013) must be complete first.
- **COOP/COEP headers** on all server responses (required for `SharedArrayBuffer`):
  - `Cross-Origin-Opener-Policy: same-origin`
  - `Cross-Origin-Embedder-Policy: require-corp`
  - `Cross-Origin-Resource-Policy: same-origin`
  - Add via Axum middleware in `crates/ironpad-server/src/main.rs`.
  - **Risk**: May break cross-origin resource loading (Monaco, external assets). Needs careful testing.

## Implementation Plan

### 1. COOP/COEP Headers

Add headers as Axum middleware. Test that all resources (Monaco editor, executor scripts, etc.) still load correctly under the stricter policy.

### 2. ironpad-cell Dependency

Add `wasm-bindgen-rayon` to `crates/ironpad-cell/Cargo.toml`. Export `rayon::prelude::*` in the cell prelude (`crates/ironpad-cell/src/lib.rs`, prelude module). Cells should be able to use `par_iter()` without extra imports.

### 3. Conditional Atomics in Compiler Pipeline

**Key insight**: Atomics compilation requires `-Zbuild-std=std,panic_abort` to rebuild the standard library with atomics support. Normal cells should **not** pay any cost for this.

**Approach**: Detect rayon usage at scaffold time and conditionally enable atomics.

- **Detection**: After `merge_dependencies()` in `scaffold.rs`, check `merged_deps.contains_key("rayon")`.
- **Flag injection**: In `build.rs`, set `RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals"` only when `needs_atomics` is true.
- **Cache isolation**: Include `needs_atomics` in the blake3 hash input so atomics and non-atomics builds get separate cache entries.
- **User experience**: Zero friction — just add `rayon` to cell deps, atomics are enabled automatically.

### 4. Pre-Built Std Sysroot (Eliminating the Compile-Time Penalty)

The naive approach to `-Zbuild-std` incurs a 30-60s one-time penalty rebuilding the Rust standard library. This can be **completely eliminated** by pre-building std with atomics and baking it into the Docker image (or caching it locally).

#### Core Idea

Instead of passing `-Zbuild-std` at cell-compile time (which triggers an expensive std rebuild), **pre-compile std with atomics during Docker image build** and reuse the cached artifacts at runtime via a shared target directory.

#### Directory Layout

```
cache/
  cargo-home/                    # shared registry (already exists)
  targets/
    {session}/                   # per-session target dir for normal cells (already exists)
    atomics-shared/              # shared target dir for atomics cells (NEW)
                                 # contains pre-built std + incremental cell artifacts
  atomics-sysroot/               # (alternative) standalone pre-built sysroot
```

#### Docker Build: Pre-Compile Std Once

Extend the existing warmup pattern in `docker/Dockerfile` to also build a dummy crate with atomics flags, which causes cargo to compile and cache std with atomics support:

```dockerfile
# In the runtime stage, after toolchain setup:
RUN rustup component add rust-src

# Pre-build std with atomics for wasm32-unknown-unknown.
# Uses the warmup crate as a vehicle — the important artifact is the cached std.
COPY crates/ironpad-cell /tmp/warmup-atomics/crates/ironpad-cell
COPY docker/warmup-Cargo.toml /tmp/warmup-atomics/Cargo.toml
RUN mkdir -p /tmp/warmup-atomics/src && echo "" > /tmp/warmup-atomics/src/lib.rs \
    && cd /tmp/warmup-atomics \
    && CARGO_HOME=/ironpad/cache/cargo-home \
       CARGO_TARGET_DIR=/ironpad/cache/targets/atomics-shared \
       RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals" \
       cargo build \
         -Z build-std=std,panic_abort \
         --target wasm32-unknown-unknown \
         --release 2>/dev/null || true \
    && rm -rf /tmp/warmup-atomics
```

After this, `/ironpad/cache/targets/atomics-shared/` contains the compiled std with atomics. All subsequent cell builds that use this target dir skip the std rebuild entirely.

#### Cell Compile Time: Two Target Dir Paths in `build.rs`

```rust
// In build.rs — choose target dir based on atomics requirement:

fn target_dir(cache_dir: &Path, session_id: &str, needs_atomics: bool) -> PathBuf {
    if needs_atomics {
        // All atomics cells share one target dir with pre-built std.
        cache_dir.join("targets").join("atomics-shared")
    } else {
        // Normal cells get per-session incremental builds.
        cache_dir.join("targets").join(session_id)
    }
}
```

When `needs_atomics` is true:
1. Set `RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals"`.
2. Point `CARGO_TARGET_DIR` at the shared atomics target dir (pre-warmed by Docker).
3. **Do not pass `-Zbuild-std`** — std is already compiled in that target dir.
4. Cell compilation proceeds at normal speed (~3-9s) because only the cell crate itself needs to compile.

#### Compile-Time Impact (with Pre-Built Std)

| Cell Type | Target Dir | RUSTFLAGS | `-Zbuild-std` | First Compile |
|---|---|---|---|---|
| Normal | `targets/{session}` | (none) | No | ~3-8s |
| Atomics/rayon | `targets/atomics-shared` | `+atomics,+bulk-memory,+mutable-globals` | **No** (pre-built) | ~3-9s |

**The 30-60s std rebuild penalty is completely eliminated.** Rayon cells compile at essentially the same speed as normal cells.

| Flag               | Compile Time                            | Runtime Impact                                  | Binary Size     |
| ------------------ | --------------------------------------- | ----------------------------------------------- | --------------- |
| `+atomics`         | ~5-10% slower codegen                   | ~2-10% overhead (atomic ops)                    | Slightly larger |
| `+bulk-memory`     | Negligible                              | **Faster** (native `memory.copy`/`memory.fill`) | Neutral         |
| `+mutable-globals` | Negligible                              | Negligible                                      | Negligible      |
| `-Zbuild-std`      | **0s** (pre-built in Docker)            | N/A                                             | N/A             |

#### Local Development

For local dev (non-Docker), add a one-time warmup task:

```toml
# In Makefile.toml:
[tasks.warmup-atomics]
description = "Pre-build std with atomics for wasm32 (one-time setup for rayon cells)"
command = "cargo"
args = ["build", "-Zbuild-std=std,panic_abort", "--target", "wasm32-unknown-unknown", "--release"]
env = { "RUSTFLAGS" = "-C target-feature=+atomics,+bulk-memory,+mutable-globals", "CARGO_TARGET_DIR" = "${IRONPAD_CACHE_DIR}/targets/atomics-shared" }
```

On first run, this takes 30-60s. After that, the atomics sysroot is cached and all rayon cells compile at normal speed. The cache persists across `cargo clean` since it's in the ironpad cache dir, not the project target dir.

**Toolchain pinning**: The pre-built std must match the exact Rust version. In Docker this is handled by pinning `rust:1.93.0`. Locally, if `rustc --version` changes, the atomics target dir should be invalidated and rebuilt.

### 5. Thread Pool Initialization

In the Worker context (`executor-worker.js`), call `wasm-bindgen-rayon`'s `initThreadPool()` after loading a cell's WASM module. Thread count defaults to `navigator.hardwareConcurrency`.

### 6. Update Public Notebooks

Update Mandelbrot and Julia set notebooks to use `rayon::par_iter()` for pixel computation, rendering to the `Canvas` type. Add progress bar support.

## Constraints & Risks

- **COOP/COEP headers** restrict cross-origin resource loading. Monaco and external resources must be same-origin or include CORP headers. This is the main risk, but COOP/COEP is increasingly standard for sites serving WASM, and Monaco can be tested up front.
- **wasm-opt compatibility**: Need to verify that wasm-opt `-O3` works with atomics-enabled WASM output.
- **Sub-Workers of Workers**: rayon spawns sub-Workers from the main cell Worker. Supported in Chrome/Firefox/Safari but worth validating.
- **`Worker.terminate()` interaction**: If the user cancels a rayon-enabled cell, the main Worker is terminated — but the rayon sub-Workers may linger. Need to verify they're cleaned up.
- **Shared atomics target dir concurrency**: If two rayon cells compile simultaneously, they share the same `CARGO_TARGET_DIR`. Cargo handles concurrent builds via file locks, so this should work but may serialize builds. Acceptable since rayon cells are less common.

---

# Approach 2: Custom "Rayon Lite" via Message-Passing Workers (Alternative)

## Motivation

If COOP/COEP headers prove problematic (e.g., they break Monaco or external resource loading), a message-passing approach avoids all `SharedArrayBuffer` requirements. The tradeoff is reduced ergonomics and a custom implementation to maintain.

## Architecture

The cell's WASM module is instantiated N times in N workers. Work is scattered via `postMessage`, results gathered — no shared memory needed.

```
Cell WASM (running in executor worker)
  │
  │  par_map(square, &data).await
  │  ↓
  │  host_message({ type: "parallel_map", fn: "square", chunks: [...] })
  │
  ▼
executor-bridge.js (main thread, coordinator)
  │
  ├─→ Worker 1: same WASM module → ironpad_par_square(chunk_0) → result_0
  ├─→ Worker 2: same WASM module → ironpad_par_square(chunk_1) → result_1
  ├─→ Worker N: same WASM module → ironpad_par_square(chunk_N) → result_N
  │
  ▼
  Collects results → host_message back to cell → par_map() returns Vec<U>
```

## User-Facing API

### Tier 1 — Attribute Macro + `par_map`

```rust
use ironpad_cell::prelude::*;

// Macro generates an additional WASM export: `ironpad_par_square`
// with signature (ptr, len) -> (ptr, len) using bincode ser/de.
#[parallel]
fn square(x: f64) -> f64 {
    x * x
}

// Cell must be async to use par_map (yields to JS event loop for dispatch).
let data: Vec<f64> = (0..1_000_000).map(|i| i as f64).collect();
let results = par_map(&data, square).await;
```

### Tier 2 — Context Parameter (Closure Workaround)

```rust
#[parallel]
fn compute(x: f64, ctx: &MyContext) -> f64 {
    x * ctx.offset
}

let ctx = MyContext { offset: 42.0 };
let results = par_map_with_ctx(&data, compute, &ctx).await;
```

The macro generates an export that takes `(chunk_ptr, chunk_len, ctx_ptr, ctx_len)`. The context is serialized once and broadcast to all workers.

## Why Async Is Required

The cell's `cell_main` needs to wait for worker results. Two options exist:

- **Option A — Async cell + `.await`** (recommended): Fits naturally with existing async cell support. The cell yields to the JS event loop, the bridge dispatches to workers, and the cell resumes when results arrive. Works today with existing host messaging + async scaffold.
- **Option B — Synchronous blocking via `Atomics.wait`**: Would need `SharedArrayBuffer` between the cell worker and the coordinator — defeating the purpose.

## Comparison with Rayon

| Capability | Rayon (Approach 1) | Rayon Lite (Approach 2) |
|---|---|---|
| `SharedArrayBuffer` | Yes | **No** |
| COOP/COEP headers | Yes | **No** |
| `-Zbuild-std` | Yes (pre-built) | **No** |
| Closures with captures | **Yes** | No — functions only |
| Shared mutable state | **Yes** | No — pure map/reduce |
| `par_iter().map().filter().reduce()` | **Yes** | No — `par_map` only |
| Data transfer overhead | Shared memory (zero-copy) | `postMessage` serialization |
| Implementation effort | Low (existing library) | **High** (custom macro + JS pool) |
| Maintenance burden | Low (upstream rayon) | **High** (custom framework) |

## Implementation Components

| Component | Effort |
|---|---|
| `#[parallel]` proc macro (new crate) | Medium — code generation for WASM exports |
| `par_map` / `par_map_with_ctx` runtime | Low — host_message + async receive |
| JS worker pool in executor-bridge.js | Medium — pool management, scatter-gather |
| Integration tests | Medium — need WASM + browser environment |

---

# Decision & Recommendation

**Go with Approach 1 (rayon + pre-built std)**. The pre-built std sysroot eliminates the compile-time penalty — the main practical objection. Users get full `par_iter` with closures, captures, and `reduce`. COOP/COEP headers are the main risk, but they're increasingly standard and testable up front.

**Fallback**: If COOP/COEP breaks Monaco or critical external resources, Approach 2 is a viable fallback but should be considered a last resort given the maintenance burden and ergonomic constraints.

---

## References

- [wasm-bindgen-rayon](https://github.com/nickhobbs94/nickhobbs94.github.io/tree/master)
- [SharedArrayBuffer security requirements (MDN)](https://developer.mozilla.org/en-US/docs/Web/JavaScript/Reference/Global_Objects/SharedArrayBuffer#security_requirements)
- [COOP/COEP explainer](https://web.dev/cross-origin-isolation-guide/)
- [wasm_thread crate](https://crates.io/crates/wasm_thread)
- [gloo-worker (Gloo project)](https://github.com/nickhobbs94/nickhobbs94.github.io/tree/master)

---

## Code References

| File                                          | Relevance                                                       |
| --------------------------------------------- | --------------------------------------------------------------- |
| `crates/ironpad-server/src/main.rs`           | Add COOP/COEP middleware (~line 61-73)                          |
| `crates/ironpad-cell/src/lib.rs`              | Add rayon prelude exports (lines 44-72)                         |
| `crates/ironpad-cell/Cargo.toml`              | Add wasm-bindgen-rayon dependency                               |
| `crates/ironpad-app/src/compiler/scaffold.rs` | Detect rayon in deps, `merge_dependencies()` (line 162)         |
| `crates/ironpad-app/src/compiler/build.rs`    | Conditionally set RUSTFLAGS + target dir routing                |
| `crates/ironpad-app/src/compiler/cache.rs`    | Include atomics flag in cache key                               |
| `crates/ironpad-app/src/compiler/optimize.rs` | Verify wasm-opt compat with atomics                             |
| `docker/Dockerfile`                           | Add atomics std warmup stage in runtime image                   |
| `docker/warmup-Cargo.toml`                    | Reused for atomics warmup build                                 |
| `Makefile.toml`                               | Add `warmup-atomics` task for local dev                         |
| `public/executor-worker.js`                   | Call `initThreadPool()` after WASM load (future, from PRD-0013) |
