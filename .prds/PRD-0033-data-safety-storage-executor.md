---
id: PRD-0033
title: "Data safety: storage & executor robustness (ST1-ST3, EX1-EX5)"
status: active
owner: "Aaron Roney"
created: 2026-07-02
updated: 2026-07-02

depends_on:
- PRD-0031

principles:
- "One bad record must never hide the user's other notebooks"
- "A failed async op surfaces an error — it never leaves the UI hung"
- "Executor state is per-cell; one cell's cleanup must not corrupt another's"

references:
- name: "Review report — sections ST1-ST3 (storage), EX1-EX5 (executor)"
  url: reviews/2026-07-02-codebase-review.md

acceptance_tests:
- id: uat-001
  name: "A malformed IndexedDB record is skipped; other notebooks still list"
  command: cargo make test
  uat_status: unverified
- id: uat-002
  name: "A save that hits an IndexedDB error surfaces a UI error instead of hanging on 'Saving...'"
  command: cargo make playwright
  uat_status: unverified
- id: uat-003
  name: "A worker that dies mid-execute rejects the pending cell (cell doesn't hang on 'Running')"
  command: cargo make playwright
  uat_status: unverified

tasks:
- id: T-001
  title: "ST1: One malformed record must not hide all notebooks"
  priority: 1
  status: done
  notes: "storage/client.rs:36-39: list_notebooks does from_value::<Vec<IronpadNotebook>>(val).unwrap_or_default() -> one non-matching record errors the whole Vec -> empty list, all notebooks appear lost. Fix: iterate the JS array and from_value each element, skipping+logging failures."
- id: T-002
  title: "ST2: Surface IndexedDB rejections instead of silently killing the future"
  priority: 1
  status: done
  notes: "storage/client.rs:9-31: js_list/get/save/delete/search/export/import externs have no catch; a rejected IDB promise (QuotaExceededError on save, blocked open) becomes an uncaught throw that aborts the awaiting task -> UI stuck on 'Saving...' forever. Fix: add catch, return Result<JsValue, JsValue>, surface errors from wrappers (state.rs:166, home_page.rs:85, view_only_notebook.rs:170). Also replace .expect() at client.rs:52 with error propagation."
- id: T-003
  title: "ST3: storage.js hygiene — close DB on error, handle onblocked, robust sort, no caller mutation"
  priority: 2
  status: done
  notes: "public/storage.js: wrap each method's db.close() in try/finally (:52-102) so it isn't skipped on the error path; add req.onblocked=reject to openDb (:20-34); guard NaN sort on bad updated_at (:58-62, use || 0); clone before mutating notebook.updated_at (:85,132-136)."
- id: T-004
  title: "EX1: A dead worker must reject all in-flight requests (and respawn)"
  priority: 1
  status: done
  notes: "executor-bridge.js:55-58: _worker.onerror only console.errors; _pending is never rejected and the worker isn't re-armed, so every outstanding loadBlob/execute/tick Promise hangs -> the Rust JsFuture (executor.rs:116,139,210) never resolves and the cell stays Compiling/Running forever. Fix: in onerror reject all _pending (like terminate) and respawn the worker."
- id: T-005
  title: "EX2: Main-thread fallback executor must use its own global, not the bridge's"
  priority: 1
  status: done
  notes: "executor.js:12 + executor-bridge.js:104-105: fallback is new CellExecutor('window.IronpadExecutor'), but the bridge re-claims that global, so at cell runtime the sim/GPU/_dispatchHostMessage FFI shims call the BridgeExecutor (which lacks them) -> '_simRead is not a function' and dropped progress/sim_emit. Fix: give the fallback a stable distinct global (e.g. window.__IronpadFallback) and pass it as globalRef."
- id: T-006
  title: "EX3: Memoize the fallback loader to prevent double-load / global corruption"
  priority: 2
  status: done
  notes: "executor-bridge.js:87-123: _ensureMainExecutor returns a fresh Promise each call, guarding only on the resolved this._mainExecutor. Two overlapping fallbacks both inject executor-core.js+executor.js and both do the capture/restore dance (101-105), potentially leaving window.IronpadExecutor pointing at a fallback with no terminate(). Fix: memoize the in-flight promise (this._mainExecutorPromise)."
- id: T-007
  title: "EX4: Scope GPU handles and pending readbacks per cell/execution"
  priority: 2
  status: todo
  notes: "executor-core.js:824,903 + 342-345,457-524: _gpuHandles/_gpuNextHandle/_pendingGpuReadbacks are shared across all executes; every execute/tick ends with _gpuCleanupHandles(all keys), destroying other cells' in-use buffers; a trapped cell's queued readback (806-815) leaks into the next result. Fix: scope handles + pending readbacks per cell/execution; clear pending readbacks in the trap catch blocks (806-811, 889-894)."
- id: T-008
  title: "EX5: Rayon glue global race, cross-attributed panic text, racy blob revoke, dead code, tick trap message"
  priority: 3
  status: todo
  notes: "executor-core.js:685 (self.__ironpadRayonGlue single mutable global clobbered when two rayon cells load concurrently — thread glue per-load through the initThreadPool closure); worker-executor.js:16,91-139 (_lastPanicMessage shared global cross-attributes panic text between interleaved executes — correlate to request id); executor-core.js:636-637 (sub-worker blob URL revoked before the module-worker fetch — revoke in the ready handshake); executor-bridge.js:125-139 (_executeOnMainThread dead code — remove); executor-core.js:993,1039 (tick error uses e.message -> 'undefined' for non-Error throws — use _describeWasmTrap for parity)."
---

# Summary

Storage and the executor have data-loss and stuck-state gaps: one malformed IndexedDB record hides *all* notebooks, IndexedDB rejections silently kill the save future (UI hangs on "Saving…"), a dead worker strands every in-flight cell, and the main-thread fallback calls the wrong global so any fallback cell using sim/GPU traps. This epic makes both layers fail safely and keep executor state per-cell.

# Problem

The common thread is unhandled failure: `unwrap_or_default()` on a whole-Vec deserialize, `catch`-less async externs, an `onerror` that only logs, and shared mutable executor globals that leak or collide across concurrent/interleaved cells. Each turns a recoverable condition into silent data loss or a permanently stuck UI.

# Goals

1. Notebook listing tolerates individual bad records.
2. Every storage async op returns a `Result` and surfaces errors to the UI.
3. A worker death or fatal error rejects the affected requests and recovers.
4. The fallback executor is self-contained and correct for sim/GPU cells.
5. GPU/rayon/panic state is scoped per cell, not shared globally.

# Technical Approach

Storage fixes are in Rust (`storage/client.rs`) + JS (`public/storage.js`). Executor fixes are in `public/executor-bridge.js`, `executor-core.js`, `worker-executor.js`. Prioritize T-001, T-002, T-004, T-005 (data-loss / hard-hang); T-006-T-008 are robustness under concurrency. See each task's `notes` for exact `file:line`.

# Assumptions

- PRD-0031 has landed (needed to run cells and reach the fallback/worker paths in the browser).
- `imports.env` and the executor global naming are stable (coordinated with PRD-0031).

# Constraints

- `storage/client.rs` is library code — propagate `Result`, no `unwrap`/`expect` (constitution).
- Changing the storage extern signatures to return `Result` ripples through wrappers; update all call sites.

# References to Code

- `crates/ironpad-app/src/storage/client.rs`
- `public/storage.js`, `public/executor-bridge.js`, `public/executor-core.js`, `public/worker-executor.js`
- Call sites: `pages/notebook_editor/state.rs:166`, `pages/home_page.rs:85`, `components/view_only_notebook.rs:170`

# Non-Goals (MVP)

- Migrating IndexedDB schema versions / a formal migration framework.
- Serializing all worker requests (per-request correlation is enough for the panic-attribution fix).

# History

(Entries appended during implementation go below this line.)

## 2026-07-03 — Units 1-3 + dead-code done; deep executor-core concurrency deferred
Branch `fix/prd-0033-data-safety`. Storage + executor-bridge robustness landed (the high-value data-safety fixes):
- **T-001** (c179f13): list/search deserialize per-element — one malformed IndexedDB record no longer hides all notebooks.
- **T-002/T-003** (44dc3a1): all 7 storage externs now `#[wasm_bindgen(catch)]` → rejected IndexedDB promises become `Err` instead of aborting the awaiting future; `save_notebook` returns `Result` and its 3 call sites log on error (fork/create guard navigation on Ok); storage.js gets try/finally close, `onblocked`, NaN-safe sort, clone-before-mutate. Verified: wasm-target clippy + ssr clippy + 491 tests + node --check.
- **T-004/T-005/T-006** (9b1cdbe): worker `onerror` rejects all in-flight requests + respawns (no more hung cells); the main-thread fallback owns a distinct `window.__IronpadFallback` global matching its FFI globalRef (fixes fallback sim/GPU cells trapping); `_ensureMainExecutor` is memoized against concurrent double-load. node --check + review.
- **T-008 (partial)** (a5fe311): removed dead `_executeOnMainThread`.

**Deferred (need dedicated treatment):**
- **T-007 (EX4, GPU per-cell scoping)** and the remaining **T-008 (EX5)** items — rayon-glue-global threading, worker panic-message correlation, sub-worker blob-URL revoke timing, tick-trap message parity. These restructure shared state in the critical 1224-line `executor-core.js`/worker execution path and can only be exercised by WebGPU/rayon concurrency, which isn't drivable in this environment. Doing them hastily risks regressing cell execution; they warrant a focused pass with real concurrency verification. Impact is bounded (EX4 needs concurrent GPU cells; the EX5 items are MINOR/POLISH per the review).
