---
id: PRD-0027
title: "Consolidate executor.js and worker-executor.js (DRY)"
status: active
owner: "Aaron Roney"
created: 2026-03-21
updated: 2026-03-21

principles:
- "Single source of truth: all CellExecutor logic lives in one file"
- "Preserve the existing IIFE + global pattern (no ES module migration)"
- "Zero behavioral changes: the public API surface and runtime behavior must be identical"
- "Keep the diff reviewable: extract first, then clean up"

references:
- name: "PRD-0013: Web Worker Cell Execution"
  url: .prds/PRD-0013-web-worker-cell-execution.md

acceptance_tests:
- id: uat-001
  name: "No duplicated CellExecutor prototype methods across files"
  command: "grep -c 'CellExecutor.prototype' public/executor.js public/worker-executor.js public/executor-core.js | grep -v ':0$'"
  uat_status: verified
- id: uat-002
  name: "Main-thread executor loads, executes, ticks, and tickLives a cell"
  command: cargo make uat
  uat_status: unverified
- id: uat-003
  name: "Worker executor loads, executes, ticks, and tickLives a cell (with fallback)"
  command: cargo make uat
  uat_status: unverified

tasks:
- id: T-001
  title: "Create executor-core.js with all shared logic"
  priority: 1
  status: done
  notes: "Extract constants, _describeWasmTrap, all GPU state/helpers, CellExecutor constructor, and all prototype methods into executor-core.js. Parameterize loadBlob's global executor reference via a _globalRef property set on construction. The IIFE should expose the core via a configurable global (self.__IronpadExecutorCore or similar)."
- id: T-002
  title: "Rewrite executor.js as thin main-thread wrapper"
  priority: 1
  status: done
  notes: "Import executor-core.js via script ordering. Create CellExecutor with globalRef='window.IronpadExecutor'. Register progress_update and sim_emit host message handlers. Expose as window.IronpadExecutor. Should be ~40 lines."
- id: T-003
  title: "Rewrite worker-executor.js as thin worker wrapper"
  priority: 1
  status: done
  notes: "Import executor-core.js via importScripts. Expose CellExecutor constructor on self.CellExecutor. Set self._ironpadExecutor in loadBlob path. Should be ~15 lines."
- id: T-004
  title: "Verify executor-worker.js and executor-bridge.js still work"
  priority: 2
  status: done
  notes: "executor-worker.js imports worker-executor.js via importScripts — verify the import chain still works. executor-bridge.js dynamically loads executor.js as fallback — verify the script.src path and singleton setup still work."
- id: T-005
  title: "Manual QA: end-to-end cell execution"
  priority: 2
  status: todo
  notes: "Run the app and verify cells compile and execute in both worker and main-thread (fallback) modes. Test simulation tick and LiveView tick if applicable."
---

# Summary

Extract the ~1,100 lines of duplicated CellExecutor logic from `executor.js` and `worker-executor.js` into a shared `executor-core.js`, reducing both files to thin environment-specific wrappers.

# Problem

`executor.js` (1,193 lines) and `worker-executor.js` (1,178 lines) are ~95% identical. The only differences are:

1. **Global executor reference in WASM import shim strings**: `window.IronpadExecutor` vs `self._ironpadExecutor` (~20 substitution points in `loadBlob`).
2. **Worker-specific setup**: stashing the executor on `self._ironpadExecutor` so dynamically-imported ESM glue modules can reach it.
3. **Tail section**: `executor.js` creates a singleton, registers DOM-dependent host message handlers (`progress_update`, `sim_emit`), and exposes `window.IronpadExecutor`. `worker-executor.js` exports the raw constructor via `self.CellExecutor`.

Every bug fix or feature addition (e.g., the recent GPU shim routing change) must be applied identically to both files, which is error-prone and has already led to drift.

# Goals

1. Eliminate duplicated CellExecutor logic — single source of truth.
2. Keep both runtime paths (main-thread and Worker) working identically.
3. No behavioral or API changes to consumers (`executor-bridge.js`, `executor-worker.js`, or direct `executor.js` usage).

# Technical Approach

```
┌─────────────────────────────────────────────────────┐
│              executor-core.js (~1,100 lines)         │
│  Constants, _describeWasmTrap, GPU state/helpers,    │
│  CellExecutor constructor + all prototype methods    │
│  Parameterized by _globalRef for loadBlob shims      │
│  Exposes: self.__IronpadExecutorCore = { CellExecutor }│
└────────────────────┬────────────────┬────────────────┘
                     │                │
    ┌────────────────▼──┐    ┌───────▼──────────────────┐
    │  executor.js (~40) │    │ worker-executor.js (~15)  │
    │  <script> on page  │    │ importScripts in Worker   │
    │  globalRef =       │    │ globalRef =               │
    │   "window.Ironpad  │    │  "self._ironpadExecutor"  │
    │    Executor"       │    │ Expose self.CellExecutor  │
    │  Register DOM      │    └───────────────────────────┘
    │   handlers         │
    │  window.Ironpad    │
    │   Executor = exec  │
    └────────────────────┘
```

**Parameterization**: `CellExecutor` accepts a `globalRef` string (e.g., `"window.IronpadExecutor"`). The `loadBlob` method uses `this._globalRef` when building the WASM import shim string templates, instead of hardcoding the global reference. This is the only point of divergence in the core logic.

**Worker setup**: `worker-executor.js` sets `self._ironpadExecutor = executor` after construction, matching the current behavior where the dynamically-imported ESM glue module reaches the executor via the Worker global.

# Assumptions

- The IIFE + `importScripts` pattern remains the module system for `public/` JS files.
- `executor-bridge.js` loads `executor.js` via `<script>` tag for main-thread fallback — the script must still create `window.IronpadExecutor` as a side effect.
- `executor-worker.js` loads `worker-executor.js` via `importScripts` — the script must still expose `self.CellExecutor`.

# Constraints

- No ES module migration (would require build tooling changes and `<script type="module">` adoption).
- Cannot change the public API surface of `window.IronpadExecutor` or `self.CellExecutor`.
- `executor-core.js` must be loadable in both `window` and Worker contexts (no DOM references).

# References to Code

- `public/executor.js` — main-thread executor (1,193 lines)
- `public/worker-executor.js` — Worker-safe executor (1,178 lines)
- `public/executor-worker.js` — Worker entry point, imports `worker-executor.js` via `importScripts`
- `public/executor-bridge.js` — main-thread bridge that proxies to Worker, falls back to `executor.js`

# Non-Goals (MVP)

- Extracting the duplicated `progress_update` / `sim_emit` host message handler registrations shared between `executor.js` and `executor-bridge.js` (~30 lines each). These are environment-specific wiring, not core logic.
- ES module migration for the `public/` JS files.
- Refactoring `executor-bridge.js` fallback patterns (repeated try/catch with main-thread retry).

# History
(Entries appended during implementation go below this line.)

## 2026-03-21 -- Batch Execution (T-001, T-002, T-003, T-004)
- **Tasks completed**: T-001, T-002, T-003, T-004
- **Changes**:
  - T-001: Created `executor-core.js` (1,171 lines) with all shared CellExecutor logic, parameterized by `globalRef` for WASM import shim strings.
  - T-002: Rewrote `executor.js` as 46-line thin wrapper — creates singleton with `globalRef="window.IronpadExecutor"`, registers `progress_update` and `sim_emit` handlers.
  - T-003: Rewrote `worker-executor.js` as 20-line thin wrapper — imports core via `importScripts`, exposes `self.CellExecutor` with default `globalRef="self._ironpadExecutor"` and auto-stashes instance.
  - T-004: Fixed `executor-bridge.js` fallback path to load `executor-core.js` before `executor.js`. Verified all integration points.
- **Test results**: UAT-001 verified (prototype methods only in executor-core.js). `node --check` passes all files.
- **UATs verified**: uat-001
- **UATs deferred**: uat-002, uat-003 (require `cargo make uat` / live browser testing — T-005 manual QA)
- **Constitution compliance**: No violations
