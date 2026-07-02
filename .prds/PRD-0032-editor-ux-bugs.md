---
id: PRD-0032
title: "Editor UX bugs (E1-E11)"
status: active
owner: "Aaron Roney"
created: 2026-07-02
updated: 2026-07-02

depends_on:
- PRD-0031

principles:
- "A structural edit (add/delete/reorder) must not destroy other cells' visible state"
- "What the user sees must match what will be saved/shared"
- "Cancel means stop, never re-run"

references:
- name: "Review report — sections E1-E11 (editor) + root causes A-N"
  url: reviews/2026-07-02-codebase-review.md

acceptance_tests:
- id: uat-001
  name: "Adding/deleting/reordering a cell preserves other cells' status, output, and diagnostics"
  command: cargo make playwright
  uat_status: unverified
- id: uat-002
  name: "Renaming a notebook persists across reload"
  command: cargo make playwright
  uat_status: unverified
- id: uat-003
  name: "Switching to view mode forces an in-edit markdown cell back to rendered preview"
  command: cargo make playwright
  uat_status: unverified
- id: uat-004
  name: "Cancelling a running cell stops it (does not re-run on the main thread)"
  command: cargo make playwright
  uat_status: unverified
- id: uat-005
  name: "Share/Export/Download include the most recent unsaved edits and Share shows the URL"
  command: cargo make playwright
  uat_status: unverified

tasks:
- id: T-001
  title: "E1: Memoize the load gate so structural edits don't rebuild the whole cell tree"
  priority: 1
  status: done
  notes: "DONE (commit 7029c6c). Gate now `Memo::new(move |_| state.notebook.with(Option::is_some))` in a <Show> (mod.rs). BROWSER-VERIFIED: adding a cell preserves other cells' compile status/output/diagnostics; the inner <For key=id> updates the list incrementally."
- id: T-002
  title: "E5: Derive cell order badge [N] and label from live position, not the once-captured cell"
  priority: 1
  status: done
  notes: "DONE (commit 7029c6c). Order badge now a Signal::derive looking up this cell's order in state.cells by id (fallback to captured order mid-delete) — cell_item.rs. BROWSER-VERIFIED: badges update live on reorder ([0]<->[1]). Label reconciliation deferred (no focus guard to hook into; only affects remote/agent renames — tracked as a follow-up)."
- id: T-003
  title: "E2: Persist notebook title on rename"
  priority: 1
  status: done
  notes: "components/app_layout.rs:107-111: the title save Action is an empty stub and commit never bumps layout.save_generation, so the watcher at mod.rs:212-221 (which persists via NotebookUpdateMeta) never fires. Fix: commit_edit bumps layout.save_generation; verify NotebookUpdateMeta carries the new title through to persist_notebook."
- id: T-004
  title: "E3: Force markdown preview when entering view mode"
  priority: 1
  status: done
  notes: "components/markdown_cell.rs:46,56,67: editing is a private signal with no view-mode awareness, so a markdown cell left in edit state stays a broken remounted Monaco showing '#' after ◉. Fix: pass is_view_mode: Signal<bool> into MarkdownCell (from cell_item.rs:1316) and add Effect::new(move || if is_view_mode.get() { editing.set(false); })."
- id: T-005
  title: "E4: Commit markdown edits on blur"
  priority: 2
  status: done
  notes: "components/markdown_cell.rs:39-40,61-99: only the Monaco Escape action commits; clicking away leaves edit mode and the parent model never hears the change (contradicts the component doc). Fix: wire container blur/focusout to commit."
- id: T-006
  title: "E8: Cancel must not re-run the cell on the main thread"
  priority: 1
  status: done
  notes: "public/executor-bridge.js:199-218 (also tick 232-253, tickLive 266-287): terminate() rejects in-flight with AbortError, but execute()'s .catch treats any worker rejection as 'worker failed -> retry on main thread' and re-runs the same cell. Cancelling an infinite loop re-runs it unkillably and freezes the tab. Fix: detect workerError.name === 'AbortError' in the catch and rethrow instead of falling back."
- id: T-007
  title: "E7: Flush pending edits before Share/Export/Download; give Share visible feedback"
  priority: 2
  status: done
  notes: "mod.rs:522,626-633,646-665: none flush the 1s debounce (cell_item.rs:1028-1031) before serializing -> stale artifact. Share also shows nothing on success. Fix: bump state.save_generation and await persistence before serializing; show a dialog/toast with the share URL and reuse the CopyButton component (has 'Copied!' feedback)."
- id: T-008
  title: "E6: Widget changes must invalidate downstream cells in reactive mode"
  priority: 2
  status: done
  notes: "cell_output.rs:407-411: update_cell_output marks staleness by re-flagging existing cell_stale keys, but successful runs remove entries (cell_item.rs:615-617) so after a clean run the map is empty and slider/checkbox changes mark nothing -> downstream never re-executes. Fix: insert true for downstream Code cells by position (mirror model.rs mark_downstream_stale) using ctx.cells + the widget's cell_id."
- id: T-009
  title: "E9: Remove a deleted cell's id from the run-all queue"
  priority: 3
  status: done
  notes: "cell_item.rs:305-349 vs :116-139: delete_cell_fn never removes the id from run_all_queue, so deleting the running cell leaves it at queue[0] and remaining cells stick on 'Queued'. Fix: run_all_queue.update(|q| q.retain(|id| id != &cid)) in the delete cleanup."
- id: T-010
  title: "E10: Clear downstream local execution_result on upstream re-run"
  priority: 3
  status: todo
  notes: "cell_item.rs:487-506 clears page-level output maps but not the downstream CellItem's local execution_result, so its Output panel shows the old result (only the stale badge updates) until it re-runs. Fix: also reset the local execution_result for invalidated downstream cells."
- id: T-011
  title: "E11: Don't show a success badge on a failed cell; surface Run-All abort"
  priority: 2
  status: done
  notes: "Confirmed live in view-only notebooks: a failed cell rendered '✓ 1001ms' next to its compile-error panel, and Run All stopped partway with no top-level indication (button never disabled, status bar unchanged). Fix: gate the success badge on actual success; reflect Run-All stop/abort in the toolbar/status."
- id: T-012
  title: "K: Guard Shift+Enter advance when the cell isn't found"
  priority: 3
  status: done
  notes: "cell_item.rs:789-795: position(...).unwrap_or(0) then focuses cells[my_idx+1..]; if the cell was removed it wrongly focuses index 1. Fix: let Some(my_idx) = ... else { return; };"
---

# Summary

The editor is where users spend all their time, and it has the highest-impact bugs from the review: a structural edit wipes every other cell's visible state, title renames vanish, view mode breaks in-edit markdown cells, and cancelling a runaway cell re-runs it and freezes the tab. This epic fixes the confirmed editor bugs (E1-E11) plus the latent badge/label bug that the E1 fix exposes.

# Problem

Root cause for the biggest one (E1): the load gate at `mod.rs:260` tracks the whole `notebook` signal, so any tracked structural mutation rebuilds the entire `<NotebookContent/>` subtree — disposing every `CellItem` and its local status/output/diagnostics signals, and re-registering listeners and SortableJS each time. The page-level output maps survive, which is why toggling view mode "restores" the output. The remaining items are independent editor correctness bugs surfaced by the same review.

# Goals

1. Structural edits (add/delete/reorder) leave other cells' visible state intact.
2. Title renames persist; badges/labels stay correct after reorder.
3. View mode always renders markdown; markdown commits on blur.
4. Cancel stops a cell; Share/Export/Download reflect the latest edits; Share confirms with a URL.
5. Reactive mode re-runs downstream cells when a widget changes.

# Technical Approach

See each task's `notes` for the exact `file:line` and fix. Land **T-001 + T-002 together** (the memo gate unmasks the stale-badge bug). T-001, T-003, T-004, T-006 are the highest-value user-facing fixes; T-008-T-012 are correctness follow-ups.

# Assumptions

- PRD-0031 has landed so cells actually run (needed to verify output-preservation and cancel behavior in the browser).
- `NotebookState` page-level context (cell_outputs, cell_stale, cell_display_texts) is the right home for surviving state.

# Constraints

- The `<For key=id>` keying in `mod.rs:823` must remain stable-id based; the badge fix derives display order separately.
- No change to the persisted notebook format.

# References to Code

- `crates/ironpad-app/src/pages/notebook_editor/mod.rs` (load gate, keydown, sortable, share/export/download)
- `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs`, `cell_output.rs`
- `crates/ironpad-app/src/components/markdown_cell.rs`, `app_layout.rs`
- `public/executor-bridge.js` (cancel/terminate path)

# Non-Goals (MVP)

- Undo/redo for cell operations.
- Preserving manual collapse changes made in view mode (finding N — deferred polish).

# History

(Entries appended during implementation go below this line.)

## 2026-07-02 — Implemented 11/12 tasks (T-010 deferred)
Executed as 7 units on branch `fix/prd-0032-editor-ux-bugs` (subagent-driven for the larger units; coordinator-direct for the small mechanical ones), each gate-verified (`cargo make clippy` + `cargo make test` 491 pass).
- **T-001/T-002** (7029c6c): memoized load gate + live-derived order badge. **Browser-verified**: adding/reordering cells preserves other cells' output; badges update live.
- **T-003** (017c4e4): title rename now bumps `save_generation` (persists via the editor save watcher); Escape reverts. **Browser-verified**: rename survives reload.
- **T-004/T-005** (90b2bde): `is_view_mode` prop + effect forces markdown preview on view-mode; `on:focusout` commits on blur. **Browser-verified**: editing a markdown cell then switching to view mode renders it.
- **T-006** (59fcab3): AbortError guard in execute/tick/tickLive `.catch` so cancel doesn't re-run the cell on the main thread. (Note: the Share success toast + clipboard copy already existed — only the flush was missing.)
- **T-007** (98e518f): flush pending cell edits before Share/Export/Download via a `save_generation` bump + an awaited yield, then re-read/serialize. (Timing heuristic — documented.)
- **T-008** (13e6896): widget changes mark strictly-downstream Code cells stale by position (was only flipping existing keys, empty after a clean run).
- **T-009/T-011/T-012** (2de3714): drop deleted cell from run_all_queue; gate the view-only `✓` timing badge on no-error; guard Shift+Enter advance against a not-found cell.
- **Verification**: Units 1-3 (the reactivity-dependent, highest-value fixes) browser-verified via Playwright. Units 4-7 are `cargo make clippy`/`test`-green and diff-reviewed; full browser acceptance (uat-004/005, widget-downstream, forced-failure badge) pending a committed `cargo make playwright` run. UAT statuses left `unverified` (no committed automated test yet — manual MCP verification only).
- **Deferred:** T-010 (E10, minor) — clearing a downstream cell's *local* execution_result on upstream re-run needs a per-CellItem effect with regression risk; the ⟳ stale badge already signals staleness. Tracked for a follow-up.
- Also fixed a cross-cutting repo bug during verification: `cargo make install-tools` installed the latest `wasm-bindgen-cli` while the workspace is locked to 0.2.114, breaking `cargo make dev/build/playwright`. Pinned the CLI in `Makefile.toml` (commit 4344b82).
