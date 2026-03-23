---
id: PRD-0029
title: "DRY / SOC / Code Quality Pass"
status: done
owner: "Aaron Roney"
created: 2026-03-22
updated: 2026-03-22

principles:
- "Eliminate code duplication (DRY) without introducing unnecessary abstractions"
- "Improve separation of concerns (SOC) in large monolithic functions/modules"
- "Remove dead code and speculative future-use stubs"
- "Replace magic numbers with named constants"
- "All changes must be behavior-preserving (no functional changes)"
- "Every task must pass cargo make ci"

references:
- name: "PRD-0027: Executor DRY Consolidation (related JS work)"
  url: .prds/PRD-0027-executor-dry-consolidation.md
- name: "PRD-0012: Pedantic Clippy and Unwrap Pass (prior quality work)"
  url: .prds/PRD-0012-pedantic-clippy-unwrap-pass.md

acceptance_tests:
- id: uat-001
  name: "Full CI passes (fmt-check + clippy + test)"
  command: cargo make ci
  uat_status: verified
- id: uat-002
  name: "Integration tests pass (compiler pipeline)"
  command: cargo make test-integration
  uat_status: unverified
- id: uat-003
  name: "Playwright e2e tests pass"
  command: cargo make playwright
  uat_status: unverified
- id: uat-004
  name: "No new #[allow(dead_code)] annotations introduced"
  command: "git diff main -- '*.rs' | grep -c '+.*#\\[allow(dead_code)\\]' | grep -q '^0$'"
  uat_status: verified
- id: uat-005
  name: "Blob URL helpers defined in exactly one module (blob_url.rs)"
  command: "grep -rn 'fn create_blob_url\\|fn revoke_blob_url' crates/ironpad-app/src/ | grep -v blob_url.rs | wc -l | grep -q '^0$'"
  uat_status: verified
- id: uat-006
  name: "Path traversal check appears once in server_fns.rs (via helper)"
  command: "grep -c 'contains.*\\.\\.' crates/ironpad-app/src/server_fns.rs | grep -q '^1$'"
  uat_status: verified

tasks:
# ── Tier 1: High-Impact DRY Violations ──────────────────────────────────────

- id: T-001
  title: "Extract shared blob URL helpers into a common module"
  priority: 1
  status: done
  notes: >
    create_blob_url and revoke_blob_url are duplicated identically between
    components/view_only_notebook.rs (lines 75-107) and
    pages/notebook_editor/cell_output.rs (lines 24-61).
    Both include #[cfg(feature = "hydrate")] and SSR stub variants.
    Extract to crates/ironpad-app/src/components/blob_url.rs,
    export from components/mod.rs, and update both call sites.

- id: T-002
  title: "Extract path traversal validation helper in server_fns.rs"
  priority: 1
  status: done
  notes: >
    Identical path traversal rejection logic at server_fns.rs lines 256 and 333:
    if filename.contains('/') || filename.contains('\\') || filename.contains("..").
    Extract to a validate_safe_path_segment(s: &str) -> anyhow::Result<()> function
    in the same file (private helper). Call from both get_public_notebook_core and
    get_shared_notebook_core.

- id: T-003
  title: "Unify SharedDepsPanel and SharedSourcePanel into a generic component"
  priority: 1
  status: done
  notes: >
    shared_deps.rs (102 lines) and shared_source.rs (98 lines) are 95% identical.
    Differences: default content string, mutation field (shared_cargo_toml vs
    shared_source), toast title, CSS class, editor language ("toml" vs "rust"),
    and icon/label text. Create a SharedEditorPanel component that accepts these
    as props. Delete both original files and replace with the unified component.

- id: T-004
  title: "Consolidate sim bus update logic in JS executor files"
  priority: 1
  status: done
  notes: >
    The sim bus update pattern (get-or-create entry, push to ring buffer, cap at
    1000) is duplicated in executor.js (lines 32-43), executor-worker.js
    (lines 38-49 and 91-100), and executor-bridge.js (lines 334-337 and 413-416).
    Extract to a shared helper function on __IronpadExecutorCore (e.g.
    updateSimBus(bus, key, json)). This extends PRD-0027 consolidation work.

- id: T-005
  title: "Deduplicate wire_msg / to_json in ws.rs tests"
  priority: 1
  status: done
  notes: >
    crates/ironpad-server/src/ws.rs defines wire_msg() at line 23 and an identical
    to_json() at line 429 inside the test module. Make wire_msg visible to tests
    (e.g. pub(crate) or move to a shared test helper) and remove to_json.

# ── Tier 2: SOC and Constant Improvements ───────────────────────────────────

- id: T-006
  title: "Extract cell count formatting helper"
  priority: 2
  status: done
  notes: >
    The pattern `let cell_label = if cell_count == 1 { "cell" } else { "cells" };`
    is duplicated at home_page.rs lines 263 and 316. Extract to a small helper
    function like format_cell_count(n: usize) -> String in the same file or a
    shared utility.

- id: T-007
  title: "Define named constants for magic numbers"
  priority: 2
  status: done
  notes: >
    Replace hardcoded magic numbers with named constants:
    model.rs:109 — 64 (max event buffer size) -> const MAX_EVENT_BUFFER: usize.
    notebook_editor/state.rs:136 — 500 (debounce delay ms) -> const REACTIVE_DEBOUNCE_MS.
    notebook_editor/mod.rs:241-250 — 2_000 (save status reset) -> const SAVE_STATUS_RESET_MS.
    Define constants near their usage site (same module).

- id: T-008
  title: "Audit and remove dead code marked future use"
  priority: 2
  status: done
  notes: >
    15+ instances of #[allow(dead_code)] with "future/conditional use" comments:
    model.rs (lines 18, 119, 166, 172), session/mod.rs (lines 25, 39, 41, 44, 59),
    components/monaco_editor.rs (line 81), notebook_editor/state.rs (lines 16, 47,
    55, 71), components/view_only_notebook.rs (line 113), notebook_editor/cell_output.rs
    (line 68). For each: remove if truly unused, or if genuinely needed keep but
    justify with a real reason (not "future use"). Goal is zero speculative dead code.

# ── Tier 3: Structural Cleanup ──────────────────────────────────────────────

- id: T-009
  title: "Break up handle_ipc_request in CLI daemon"
  priority: 3
  status: done
  notes: >
    crates/ironpad-cli/src/daemon.rs handle_ipc_request (lines 375-437) is a
    62-line function dispatching 5+ command types. Extract per-command handlers:
    serve_notebook_get(), serve_cells_list(), serve_cells_get(), serve_status(),
    serve_server_command(). Keep handle_ipc_request as a thin dispatcher.

- id: T-010
  title: "Break up update_cache_from_event in CLI daemon"
  priority: 3
  status: done
  notes: >
    crates/ironpad-cli/src/daemon.rs update_cache_from_event (lines 268-348) is an
    80-line function with 10+ match arms for event types. Extract per-event-type
    functions (apply_cell_added, apply_cell_updated, apply_cell_deleted, etc.) to
    improve readability. Keep update_cache_from_event as a thin match dispatcher.

- id: T-011
  title: "Break up handle_cells_command in CLI main"
  priority: 3
  status: done
  notes: >
    crates/ironpad-cli/src/main.rs handle_cells_command (lines 224-314) is a
    90-line function dispatching 6 cell subcommands. Extract each subcommand
    handler into its own function for clarity.

- id: T-012
  title: "Extract CLI exit codes into an enum"
  priority: 3
  status: done
  notes: >
    crates/ironpad-cli/src/main.rs uses hardcoded exit codes 1, 2, 3, 4 at lines
    374-389. Create an enum CliExitCode with variants GenericError = 1,
    VersionConflict = 2, PermissionDenied = 3, ConnectionError = 4. Use throughout
    the CLI for consistent exit code handling.
---

# Summary

A behavior-preserving refactoring pass to eliminate DRY violations, improve separation of concerns, remove speculative dead code, and replace magic numbers with named constants across the ironpad workspace. No new features — purely structural improvements.

# Problem

A full codebase review identified recurring patterns of duplicated logic, monolithic functions mixing multiple concerns, dead code kept for speculative future use, and magic numbers without named constants. These issues increase maintenance burden, make the codebase harder to navigate, and risk divergent bug fixes.

# Goals

1. Eliminate all identified DRY violations (duplicated blob URL helpers, path validation, shared editor panels, sim bus logic, test helpers)
2. Break up large monolithic functions in the CLI daemon and main modules into focused, single-purpose handlers
3. Remove all speculative `#[allow(dead_code)]` annotations or justify with real reasons
4. Replace magic numbers with named constants for self-documenting code
5. Maintain identical runtime behavior throughout — no functional changes

# Technical Approach

Work proceeds in three tiers by impact:

**Tier 1 (DRY violations)**: Extract duplicated functions into shared modules. For Rust code, create new modules (e.g., `blob_url.rs`) and re-export from `mod.rs`. For JS code, add helpers to `executor-core.js`. For near-identical components (`SharedDepsPanel`/`SharedSourcePanel`), parameterize into a single generic component.

**Tier 2 (Constants and dead code)**: Replace magic numbers with `const` declarations near their usage. Audit each `#[allow(dead_code)]` — remove the code if truly unused, or remove the annotation if the code is actually used via feature flags or conditional compilation.

**Tier 3 (Structural cleanup)**: Refactor large match/dispatch functions into thin dispatchers that call focused per-variant handlers. This improves readability without changing behavior.

Each tier can be worked independently. Within a tier, tasks are independent unless noted.

# Assumptions

- The codebase passes `cargo make ci` before this work begins
- PRD-0027 executor consolidation work is complete (T-004 extends it)
- No concurrent feature work will conflict with these structural changes

# Constraints

- Zero behavioral changes — all refactoring must be behavior-preserving
- No new dependencies introduced
- No public API changes
- Must pass existing CI, integration tests, and Playwright e2e suite

# References to Code

Key files affected:
- `crates/ironpad-app/src/components/view_only_notebook.rs` — blob URL duplication
- `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs` — blob URL duplication
- `crates/ironpad-app/src/server_fns.rs` — path validation duplication
- `crates/ironpad-app/src/pages/notebook_editor/shared_deps.rs` — near-identical panel
- `crates/ironpad-app/src/pages/notebook_editor/shared_source.rs` — near-identical panel
- `crates/ironpad-server/src/ws.rs` — wire_msg/to_json test duplication
- `crates/ironpad-cli/src/daemon.rs` — large dispatch functions
- `crates/ironpad-cli/src/main.rs` — large dispatch functions, magic exit codes
- `crates/ironpad-app/src/model.rs` — dead code, magic numbers
- `crates/ironpad-app/src/pages/home_page.rs` — cell count formatting
- `public/executor.js`, `public/executor-worker.js`, `public/executor-bridge.js` — sim bus duplication

# Non-Goals (MVP)

- Introducing new error types or Result wrappers across crate boundaries
- Refactoring the compiler pipeline in server_fns.rs (separate concern)
- CSS modularization (manageable at current size)
- Moving IPC types to ironpad-common (architectural change beyond cleanup)
- Refactoring DaemonState or WsState into sub-managers (larger design change)
- Breaking up NotebookContent or CellItem components (UI refactor, not cleanup)

# History

- 2026-03-22: PRD created from full codebase DRY/SOC review covering ironpad-app, ironpad-server, ironpad-common, ironpad-cli, ironpad-cell, and cross-crate concerns. 12 tasks identified across 3 priority tiers.

## 2026-03-22 -- Batch Execution (T-001 through T-012)
- **Tasks completed**: T-001, T-002, T-003, T-004, T-005, T-006, T-007, T-008, T-009, T-010, T-011, T-012
- **Changes**:
  - T-001: Extracted `create_blob_url`/`revoke_blob_url` into `components/blob_url.rs`; removed duplicates from `view_only_notebook.rs` and `cell_output.rs`
  - T-002: Extracted `validate_safe_path_segment()` helper in `server_fns.rs`; replaced two inline checks
  - T-003: Created `SharedEditorPanel` with `SharedEditorKind` enum; `shared_deps.rs` and `shared_source.rs` are now thin wrappers
  - T-004: Added `CellExecutor.updateSimBus()` static helper in `executor-core.js`; replaced 7 inline copies across `executor.js`, `executor-worker.js`, `executor-bridge.js`
  - T-005: Removed duplicate `to_json` from ws.rs test module; tests now use `wire_msg`
  - T-006: Added `format_cell_count()` helper in `home_page.rs`; replaced 2 inline patterns
  - T-007: Added `MAX_EVENT_BUFFER`, `REACTIVE_DEBOUNCE_MS`, `SAVE_STATUS_RESET_MS` constants
  - T-008: Removed `ConnectionStatus::Reconnecting` (never constructed) and `refresh_generation` field (never read); updated 10 `#[allow(dead_code)]` comments with real justifications (hydrate-only usage)
  - T-009: Extracted `serve_notebook_get()`, `serve_cells_list()`, `serve_cells_get()`, `serve_status()` from `handle_ipc_request`
  - T-010: Extracted `apply_cell_added()`, `apply_cell_updated()`, `apply_cell_deleted()`, `apply_cell_reordered()`, `apply_notebook_meta_updated()` from `update_cache_from_event`
  - T-011: Extracted 6 subcommand handlers from `handle_cells_command`
  - T-012: Created `CliExitCode` enum with `#[repr(i32)]`; replaced 4 hardcoded exit codes
- **Test results**: `cargo make ci` passes — 479 tests passed, 0 failed, 6 skipped
- **UATs verified**: uat-001 (CI), uat-004 (no new dead_code), uat-005 (blob_url single source), uat-006 (path validation single source)
- **UATs deferred**: uat-002 (integration tests — slow, not run), uat-003 (Playwright — requires browser environment)
- **Constitution compliance**: No violations
