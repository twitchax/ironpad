---
id: PRD-0013
title: "Web Worker Cell Execution"
status: done
owner: "Aaron Roney"
created: 2026-03-14
updated: 2026-03-14

principles:
- "All cell execution moves off the UI thread into a Web Worker"
- "Minimize user-facing API changes: cell code should work the same, just non-blocking"
- "DOM access stays on the main thread; Worker communicates via postMessage bridge"
- "The bridge maintains the same window.IronpadExecutor shape — Rust FFI bindings stay stable"

references:
- name: "MDN Web Workers API"
  url: https://developer.mozilla.org/en-US/docs/Web/API/Web_Workers_API
- name: "Rayon multi-core research (future)"
  url: docs/rayon-multi-core-research.md

acceptance_tests:
- id: uat-001
  name: "All existing notebooks compile and execute successfully with Worker-based execution"
  command: cargo make uat
  uat_status: unverified
- id: uat-002
  name: "UI remains responsive during long-running cell execution (no event loop blocking)"
  command: cargo make playwright
  uat_status: unverified
- id: uat-003
  name: "Cell terminate/cancel works: running cell can be stopped by user"
  command: cargo make playwright
  uat_status: unverified
- id: uat-004
  name: "Progress bar updates work correctly via Worker postMessage bridge"
  command: cargo make playwright
  uat_status: unverified
- id: uat-005
  name: "CI passes: all unit tests and integration tests green"
  command: cargo make ci
  uat_status: unverified

tasks:
- id: T-001
  title: "Extract executor core logic into worker-executor.js"
  priority: 1
  status: done
  notes: "Refactor public/executor.js: split into (1) worker-executor.js with WASM load/execute logic (no DOM access), and (2) a slim main-thread bridge. The worker file will run inside a Web Worker context. Remove all window/DOM references from the worker half (e.g., progress_update DOM manipulation at lines 397-410)."
- id: T-002
  title: "Create postMessage bridge (main thread side)"
  priority: 1
  status: done
  notes: "Create public/executor-bridge.js that exposes the same IronpadExecutor API (loadBlob, execute, onHostMessage, isLoaded) on window.IronpadExecutor, but delegates to the Worker via postMessage/onmessage. Use Promises with a request ID map for async call-response. Handle progress_update messages by updating DOM on the main thread."
- id: T-003
  title: "Create Web Worker bootstrap (worker side)"
  priority: 1
  status: done
  notes: "Create public/executor-worker.js that imports/includes worker-executor.js, listens for postMessage commands (loadBlob, execute, hostMessage), and posts results back. Forward host messages (like progress_update) from WASM to main thread via postMessage."
- id: T-004
  title: "Update executor.rs Rust FFI bindings"
  priority: 1
  status: done
  notes: "Update crates/ironpad-app/src/components/executor.rs to work with the new bridge API. The JS interface (loadBlob, execute, isLoaded) should remain the same from Rust's perspective since the bridge maintains the same window.IronpadExecutor shape. Verify init_executor() still works."
- id: T-005
  title: "Add cell termination support"
  priority: 2
  status: done
  notes: "Add a terminate/cancel mechanism: Worker.terminate() kills the running cell, then respawn a fresh Worker. Expose a cancel button in the UI (or wire into existing run button toggle). Update executor.rs with an abort/cancel binding. Consider using a single shared Worker that is torn down and recreated on cancel."
- id: T-006
  title: "Update HTML to load bridge instead of executor.js"
  priority: 1
  status: done
  notes: "Update the script tag(s) that load executor.js to load the new bridge file instead. The Worker will be spawned by the bridge. Ensure Monaco and Storage scripts are unaffected."
- id: T-007
  title: "Test: all notebooks run correctly via Worker"
  priority: 1
  status: done
  notes: "Run cargo make uat to verify all existing public notebooks still compile and execute. Manually verify UI responsiveness during long-running cells (e.g., Mandelbrot). Verify progress bar updates work through the Worker bridge."

---

# Summary

Move cell execution off the UI thread by running WASM cells in a Web Worker. This unblocks the UI during long computations and enables cell termination. Future multi-core support via rayon can build on this foundation (see [docs/rayon-multi-core-research.md](../../docs/rayon-multi-core-research.md)).

---

# Problem

Currently, `public/executor.js` runs as a singleton on `window.IronpadExecutor` on the main UI thread. When a cell executes heavy computation (e.g., Mandelbrot rendering, ray marching), the entire browser tab freezes — no scrolling, no clicking, no progress updates. There is also no way to cancel a running cell.

---

# Goals

1. **UI responsiveness**: Cell execution never blocks the main thread — UI remains interactive during computation.
2. **Cell cancellation**: Users can terminate a running cell at any time.
3. **Backward compatibility**: Existing cell code works unchanged — the Worker migration is invisible to cell authors.

---

# Technical Approach

### Architecture

```
Main Thread                          Web Worker
┌─────────────────────┐              ┌─────────────────────────┐
│ executor-bridge.js   │  postMessage │ executor-worker.js       │
│                      │ ──────────> │                           │
│ window.IronpadExec.  │             │  worker-executor.js       │
│  .loadBlob(...)      │             │   .loadBlob(...)          │
│  .execute(...)       │  postMessage│   .execute(...)           │
│  .terminate()    NEW │ <────────── │   .onHostMessage(...)     │
│                      │             │                           │
│ DOM updates:         │             │  WASM modules loaded +    │
│  progress bar        │             │  executed here (off-thread)│
│  host messages       │             │                           │
└─────────────────────┘              └─────────────────────────┘
```

### Key Changes

1. **executor.js → three files**:
   - `worker-executor.js`: Core WASM load/execute logic (no DOM, no `window`). Extracted from current `executor.js`.
   - `executor-worker.js`: Worker entry point. Imports `worker-executor.js`, listens for postMessage commands.
   - `executor-bridge.js`: Main-thread bridge. Exposes `window.IronpadExecutor` API, delegates to Worker via postMessage.

2. **postMessage protocol**: Request/response pattern with unique request IDs. Message types: `loadBlob`, `execute`, `hostMessage`, `result`, `error`.

3. **Progress updates**: WASM calls `ironpad_host_message` → Worker receives it → Worker posts `hostMessage` to main thread → bridge updates DOM.

4. **Termination**: `Worker.terminate()` kills execution instantly. Bridge respawns a fresh Worker. Pending Promises are rejected with an `AbortError`.

---

# Assumptions

- Modern browsers support Web Workers universally (baseline since IE10).
- The current `executor.js` singleton pattern can be cleanly split — no deep entanglements with `window` beyond the progress_update handler and the `IronpadExecutor` mount point.

---

# Constraints

- **Worker.terminate()** is a hard kill with no cleanup — WASM memory is discarded. This is acceptable since cells are ephemeral.
- **No `window` or DOM in Workers**: All DOM-touching code (progress bar updates, host message display) must stay on the main thread side of the bridge.
- **Script loading**: The Worker needs access to `worker-executor.js`. Since Workers can't share `<script>` tags, the bridge must spawn the Worker with the correct script URL.

---

# References to Code

| File | Role | Key Lines/Functions |
|---|---|---|
| `public/executor.js` | Current executor singleton | `loadBlob` (88-160), `execute` (166-175), `progress_update` handler (397-410), wasm-bindgen glue injection |
| `crates/ironpad-app/src/components/executor.rs` | Rust FFI bindings to executor.js | `js::load_blob` (10-18), `js::execute` (25-32), `init_executor` (68-78), `execute_cell` (125-141) |
| `crates/ironpad-cell/src/lib.rs` | Cell runtime + prelude | Prelude exports (44-72), `host_message` FFI (19-33) |

---

# Non-Goals (MVP)

- **Multi-core parallelism via rayon**: Deferred to future work. See [docs/rayon-multi-core-research.md](../../docs/rayon-multi-core-research.md).
- **Transferable objects optimization**: Using `Transferable` for zero-copy ArrayBuffer transfer (nice perf win, not required for correctness).
- **Multiple concurrent Workers**: Running multiple cells in parallel via separate Workers (single shared Worker is sufficient).
- **Streaming output**: Streaming cell output progressively during execution (current batch model is fine).
- **Worker pool**: Pre-warming or pooling Workers for faster startup.
- **Fallback for no-Worker environments**: All target browsers support Workers.
- **Server-side rendering of Worker-dependent code**: Workers are client-only; SSR path is unaffected.

---

# History

(Entries appended during implementation go below this line.)

- **2026-03-14**: All tasks (T-001 through T-007) implemented. CI passes (307 tests, 0 failures). PRD status moved to active.

---
