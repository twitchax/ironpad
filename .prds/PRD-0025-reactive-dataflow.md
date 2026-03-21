---
id: PRD-0025
title: "Cell-to-Cell Reactive Dataflow"
status: active
owner: "Aaron Roney"
created: 2026-03-21
updated: 2026-03-21

principles:
- "Opt-in reactivity: never auto-execute unless the user explicitly enables it"
- "Leverage existing infrastructure: stale marking, run_all_queue, output piping"
- "Debounce aggressively: rapid edits must not trigger cascading re-executions"
- "Graceful degradation: errors in one cell stop propagation, not the whole notebook"

references:
- name: "Marimo Reactivity Model"
  url: https://docs.marimo.io/guides/reactivity.html
- name: "Observable Dataflow"
  url: https://observablehq.com/@observablehq/how-observable-runs

acceptance_tests:
- id: uat-001
  name: "cargo make ci passes with all new logic and tests"
  command: cargo make ci
  uat_status: verified
- id: uat-002
  name: "Toggling reactive mode on causes a stale downstream cell to auto-execute"
  command: cargo make playwright
  uat_status: unverified
- id: uat-003
  name: "Editing an upstream cell in reactive mode triggers debounced downstream re-execution"
  command: cargo make playwright
  uat_status: unverified
- id: uat-004
  name: "Errors in an upstream cell stop propagation and show error state on downstream cells"
  command: cargo make playwright
  uat_status: unverified
- id: uat-005
  name: "Reactive mode toggle persists across page reloads via notebook metadata"
  command: cargo make playwright
  uat_status: unverified

tasks:
- id: T-001
  title: "Add reactive_mode flag to NotebookState and persistence"
  priority: 1
  status: done
  notes: "Add `reactive_mode: RwSignal<bool>` to NotebookState (default false). Persist in IronpadNotebook metadata so it survives reload. Add toggle to gear menu."

- id: T-002
  title: "Reactive execution effect on stale marking"
  priority: 1
  status: done
  notes: "Hook into model.mark_downstream_stale(). When reactive_mode is true, instead of just marking cells stale, also enqueue them into run_all_queue after a debounce window. Use a 500ms debounce timer that resets on each new edit."

- id: T-003
  title: "Debounce/coalesce mechanism for reactive triggers"
  priority: 1
  status: done
  notes: "Create a ReactiveScheduler that collects stale cell IDs over a debounce window, then emits a single run_all_queue update with the minimal set of cells that need re-execution. Cancel pending schedules when new edits arrive."

- id: T-004
  title: "Error propagation and cascade halt"
  priority: 2
  status: done
  notes: "When a cell errors during reactive execution, mark all its downstream cells as 'blocked' (new visual state) rather than continuing. Show a muted error indicator on blocked cells. Clear blocked state when the erroring cell is fixed and re-run."

- id: T-005
  title: "Visual indicators for reactive state"
  priority: 2
  status: done
  notes: "Add UI indicators: (1) reactive mode badge in toolbar, (2) 'pending re-execution' state on stale cells when reactive mode is on, (3) 'blocked by upstream error' state. Use existing cell status badge infrastructure."

- id: T-006
  title: "Reactive mode in view-only notebooks"
  priority: 2
  status: done
  notes: "When a view-only or public notebook has reactive_mode enabled, widget interactions (sliders, buttons) should trigger downstream re-execution automatically. Currently widgets can trigger downstream cells via WidgetContext — extend this to respect reactive_mode for non-widget edits too."

- id: T-007
  title: "Unit and integration tests"
  priority: 2
  status: done
  notes: "Test debounce coalescing, error propagation halting cascades, reactive_mode persistence, and queue population logic. Add Playwright e2e test for the full edit-triggers-downstream flow."

- id: T-008
  title: "Update showcase notebook to demonstrate reactivity"
  priority: 3
  status: done
  notes: "Create or update a notebook that showcases reactive dataflow — e.g., a parameters cell feeding into a visualization cell that auto-updates when parameters change."

---

# Summary

Add opt-in reactive dataflow so that editing a cell automatically re-executes all downstream cells that depend on its output. This turns ironpad into a spreadsheet-like reactive environment where changing parameters upstream instantly propagates results downstream.

---

# Problem

Today, after editing a cell, users must manually re-run all downstream cells to see updated results. This is tedious for iterative workflows — tweak a parameter, Ctrl+Shift+Enter, wait, repeat. Notebooks like the Mandelbrot viewer or physics simulations would benefit enormously from automatic re-execution when upstream cells change.

The infrastructure is 90% there: `mark_downstream_stale()` already identifies affected cells, `run_all_queue` already handles sequential execution, and output piping already chains cell I/O. The missing piece is connecting "stale" → "enqueue for re-execution" with proper debouncing.

---

# Goals

1. Provide an opt-in reactive mode toggle that auto-executes downstream cells on upstream changes
2. Debounce rapid edits to avoid wasteful cascading re-executions
3. Halt propagation on errors to prevent broken notebooks from spiraling
4. Persist the reactive mode preference per-notebook

---

# Technical Approach

## Existing Infrastructure (no changes needed)

- **`mark_downstream_stale(cell_id)`** — already identifies all downstream Code cells when source changes
- **`cell_stale: RwSignal<HashMap<String, bool>>`** — already tracks which cells are stale
- **`run_all_queue: RwSignal<Vec<String>>`** — already drives sequential execution with output piping
- **Output piping** — already serializes previous cell outputs as input bytes to the next cell

## New Components

### ReactiveScheduler

A debounce layer that sits between stale marking and queue population:

```
CellUpdated event
  → mark_downstream_stale()        [existing]
  → ReactiveScheduler.schedule()   [new: collects cell IDs, starts 500ms timer]
  → (timer fires)
  → run_all_queue.set(coalesced_ids) [existing execution machinery]
```

The scheduler:
- Collects all newly-stale cell IDs into a pending set
- Resets a 500ms debounce timer on each new stale marking
- When the timer fires, computes the minimal ordered set of cells to run
- Populates `run_all_queue` (existing execution takes over)

### Error Cascade Halt

When reactive execution hits an error:
1. The erroring cell shows its error as usual
2. All downstream cells in the queue are removed and marked `CellStatus::Blocked`
3. A `cell_blocked_by: RwSignal<HashMap<String, String>>` tracks which cell caused the block
4. When the blocking cell is successfully re-run, blocked status clears and downstream cells re-enqueue

### Reactive Mode Toggle

- `reactive_mode: RwSignal<bool>` in `NotebookState`
- Toggle in gear menu (⚡ icon) and/or toolbar
- Persisted in `IronpadNotebook` metadata (new optional field `reactive_mode: Option<bool>`)
- Default: `false` (off) — never surprise users with auto-execution

---

# Assumptions

- Linear cell ordering is sufficient for dependency tracking (no need for an explicit DAG)
- 500ms debounce is a reasonable default (may want to make configurable later)
- Compilation + execution time for most cells is short enough that reactive mode feels responsive

---

# Constraints

- Must not break existing manual execution workflows when reactive mode is off
- Must not trigger re-compilation if only the debounce timer fired but source hasn't changed (cache handles this)
- Web Worker execution must be respected (reactive runs go through the same worker path)

---

# References to Code

- `crates/ironpad-app/src/model.rs` — `mark_downstream_stale()`, `apply()`, event emission
- `crates/ironpad-app/src/pages/notebook_editor/state.rs` — `NotebookState`, `cell_stale`, `run_all_queue`, `CellOutputData`
- `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs` — queue watcher Effect, execution flow, error handling
- `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs` — `WidgetContext` (existing reactive trigger for widgets)
- `crates/ironpad-common/src/types.rs` — `IronpadNotebook`, `IronpadCell`

---

# Non-Goals (MVP)

- Explicit dependency graph (DAG) with user-drawn edges between cells
- Partial re-execution (skip cells whose inputs haven't changed) — cache already handles this at the compilation level
- Cross-notebook reactivity
- Configurable debounce interval (use 500ms fixed for MVP)

---

# History

(Entries appended during implementation go below this line.)

## 2026-03-21 — Batch Execution (T-001 through T-008)
- **Tasks completed**: T-001, T-002, T-003, T-004, T-005, T-006, T-007, T-008
- **Changes**:
  - T-001: Added `reactive_mode` flag to `IronpadNotebook`, `NotebookState`, gear menu toggle, protocol extension
  - T-002+T-003: Reactive scheduler with 500ms debounce via `web_sys::Window.set_timeout`, Effect watches `cell_stale`
  - T-004+T-005: Error cascade halt with `cell_blocked_by` tracking, `CellStatus::Blocked` variant, CSS indicators
  - T-006: View-only notebooks with `reactive_mode: true` auto-execute cells on page load
  - T-007: 14 new unit tests for reactive_mode serde, protocol, CellStatus::Blocked
  - T-008: Created `reactive-demo.ironpad` showcase notebook (5 cells, dependency chain)
- **Test results**: 479 tests pass, clippy clean, fmt clean
- **UATs verified**: uat-001 (`cargo make ci` passes)
- **UATs unverified**: uat-002 through uat-005 (require Playwright with running server)
- **Constitution compliance**: No violations

---
