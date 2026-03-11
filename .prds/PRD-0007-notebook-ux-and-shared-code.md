---
id: PRD-0007
title: "Notebook UX: Run All, Shared Code, Button Widget & Auto-Run"
status: active
owner: "Aaron Roney"
created: 2026-03-11
updated: 2026-03-11

depends_on:
- PRD-0005

principles:
- "Shared code should follow the same UX pattern as shared deps"
- "Buttons replace auto-reactivity — widget changes are inert until explicitly triggered"
- "Public/shared notebooks should be runnable with zero clicks"
- "Each feature should work in both the editor and view-only notebook contexts"

references:
- name: "PRD-0005 (interactivity — widgets, auto-run, view mode)"
  url: .prds/PRD-0005-interactivity.md
- name: "PRD-0006 (QoL — auto-run default, view mode polish)"
  url: .prds/PRD-0006-qol-improvements.md
- name: "Scaffold module"
  url: crates/ironpad-app/src/compiler/scaffold.rs
- name: "Cell runtime"
  url: crates/ironpad-cell/src/lib.rs
- name: "Widget UI definitions"
  url: crates/ironpad-cell/src/ui.rs
- name: "Widget rendering (editor)"
  url: crates/ironpad-app/src/pages/notebook_editor/cell_output.rs
- name: "View-only notebook component"
  url: crates/ironpad-app/src/components/view_only_notebook.rs

acceptance_tests:
- id: uat-001
  name: "Run All button in the editor toolbar executes all code cells top-to-bottom"
  command: cargo make uat
  uat_status: unverified
- id: uat-002
  name: "Run All button in the view-only notebook executes all code cells top-to-bottom"
  command: cargo make uat
  uat_status: unverified
- id: uat-003
  name: "Shared source panel is accessible from the gear menu, like shared deps"
  command: cargo make uat
  uat_status: unverified
- id: uat-004
  name: "Functions defined in shared source are callable from any cell via shared::"
  command: cargo make uat
  uat_status: unverified
- id: uat-005
  name: "Shared source is included in CompileRequest and scaffolded as src/shared.rs"
  command: cargo make uat
  uat_status: unverified
- id: uat-006
  name: "Widget value changes no longer auto-trigger downstream cell re-execution"
  command: cargo make uat
  uat_status: unverified
- id: uat-007
  name: "ui::button widget renders a clickable button in the output panel"
  command: cargo make uat
  uat_status: unverified
- id: uat-008
  name: "Clicking a button widget re-executes its cell and all downstream code cells"
  command: cargo make uat
  uat_status: unverified
- id: uat-009
  name: "Public and shared notebook pages auto-execute all code cells on load"
  command: cargo make uat
  uat_status: unverified
- id: uat-010
  name: "All changes pass cargo make ci"
  command: cargo make ci
  uat_status: verified

tasks:
- id: T-001
  title: "Add Run All button to notebook editor toolbar"
  priority: 1
  status: done
  notes: >
    Add a ▶▶ Run All button to the notebook toolbar (mod.rs NotebookContent).
    On click, collect all Code cell IDs in order and set state.run_all_queue.
    This is identical to the existing Ctrl+Shift+Enter logic — just needs a button.

- id: T-002
  title: "Add Run All button to ViewOnlyNotebook toolbar"
  priority: 1
  status: done
  notes: >
    The ViewOnlyNotebook component needs a Run All button in its toolbar
    (next to the Fork button). It must implement sequential execution: compile
    each code cell, execute it, store output in cell_outputs, then proceed to
    the next. Can reuse the same pattern as the editor's run_all_queue, but
    since ViewOnlyNotebook doesn't use NotebookState, it needs its own
    signal-based queue or an on-mount sequential runner.

- id: T-003
  title: "Add shared_source field to IronpadNotebook and CompileRequest"
  priority: 1
  status: done
  notes: >
    Add `shared_source: Option<String>` to IronpadNotebook (ironpad-common/types.rs),
    CompileRequest (ironpad-common/types.rs), and NotebookState (state.rs).
    Use `#[serde(default, skip_serializing_if = "Option::is_none")]` for backward
    compatibility. Thread it through the compile_cell server function the same
    way shared_cargo_toml is threaded.

- id: T-004
  title: "Scaffold shared source as src/shared.rs in micro-crate"
  priority: 1
  status: done
  notes: >
    In scaffold.rs: accept an optional shared_source parameter. When present,
    write the source to `{crate_dir}/src/shared.rs` and add `mod shared;` to
    the generated lib.rs preamble (before user code). This lets user cells call
    `shared::my_function()`. Update preamble_lines count accordingly.
    Update scaffold_micro_crate signature and all callers.

- id: T-005
  title: "Add Shared Source UI panel in the editor"
  priority: 2
  status: done
  notes: >
    Mirror the SharedDepsPanel pattern (shared_deps.rs): add a SharedSourcePanel
    component with a Monaco editor for shared Rust code. Accessible from the
    gear (⚙) dropdown, same as shared deps. Persist shared_source into the
    notebook on save. Wire the shared_source signal into CompileRequest
    construction in cell_item.rs.

- id: T-006
  title: "Remove auto-reactivity from widget value changes"
  priority: 2
  status: done
  notes: >
    In cell_output.rs update_cell_output(): remove the auto_run check and the
    downstream run_all_queue enqueue logic. Widget value changes should still
    update the cell's output bytes (so downstream cells use the new value when
    they next run), but should NOT automatically trigger downstream execution.
    Also remove the auto_run toggle from the gear menu and the auto_run field
    from NotebookState (or set it to permanently false).

- id: T-007
  title: "Add ui::button widget to ironpad-cell"
  priority: 2
  status: done
  notes: >
    Add a Button builder to ironpad-cell/src/ui.rs following the same pattern
    as Slider, Dropdown, etc. A button has a label, produces
    DisplayPanel::Interactive { kind: "button", config } and doesn't contribute
    meaningful output bytes (use empty bytes or a unit-type marker). Add it to
    the prelude if not already covered by the ui module re-export.

- id: T-008
  title: "Render button widget in editor cell output panel"
  priority: 2
  status: done
  notes: >
    In cell_output.rs: add a render_button() case in InteractiveWidget. The
    button renders as a clickable <button> element. On click, it enqueues the
    current cell's downstream Code cells into run_all_queue for execution.
    The button does NOT re-run its own cell — it triggers downstream execution
    using the cell's current output bytes (which reflect slider/dropdown/etc.
    widget state). This avoids resetting widget values.

- id: T-009
  title: "Render button widget in ViewOnlyNotebook"
  priority: 2
  status: done
  notes: >
    Extend ViewOnlyInteractiveWidget in view_only_notebook.rs to handle
    kind="button". In view-only mode, clicking the button should trigger
    sequential re-execution of downstream cells. This may require threading a
    run_all_queue signal through the view-only components.

- id: T-010
  title: "Auto-execute all cells on public/shared notebook page load"
  priority: 2
  status: done
  notes: >
    In ViewOnlyNotebook: after the component mounts and hydrates, automatically
    trigger sequential execution of all code cells (equivalent to clicking
    Run All). Use a one-shot Effect or on-mount callback. Should work for both
    PublicNotebookPage and SharedNotebookPage since both use ViewOnlyNotebook.

- id: T-011
  title: "Include shared_source in view-only notebook compile requests"
  priority: 2
  status: done
  notes: >
    The ViewOnlyNotebook component constructs CompileRequest in run_cell.
    Thread the notebook's shared_source and shared_cargo_toml into these
    requests so that view-only execution has access to shared code.
    The shared_source is already on IronpadNotebook — just needs to be
    extracted and passed through.

- id: T-012
  title: "Update public example notebooks to demonstrate new features"
  priority: 3
  status: done
  notes: >
    Add or update public notebooks in public/notebooks/ to showcase:
    (1) shared source usage (defining a function in shared code, calling from cells),
    (2) button widget usage (slider + button combo triggering downstream re-execution).
    Update index.json if new notebooks are added.

- id: T-013
  title: "Tests for shared source scaffolding"
  priority: 2
  status: done
  notes: >
    Add unit tests in scaffold.rs for the shared source feature:
    (1) shared source written to src/shared.rs,
    (2) mod shared; appears in generated lib.rs,
    (3) preamble_lines is correct when shared source is present,
    (4) absent shared source doesn't produce shared.rs or mod shared.
    Add an integration test that compiles a cell using shared source.

- id: T-014
  title: "Tests for button widget"
  priority: 2
  status: done
  notes: >
    Add unit tests in ironpad-cell for the button widget builder:
    (1) produces DisplayPanel::Interactive with kind="button",
    (2) default label renders correctly in config JSON.
    Optionally add an integration test that compiles a cell returning a button.

---

# Summary

This PRD covers four related notebook UX improvements: (1) a Run All button for both the editor and view-only notebooks, (2) notebook-level shared Rust source code callable from any cell, (3) a button widget that explicitly triggers downstream cell re-execution (replacing auto-reactivity), and (4) automatic execution of public/shared notebooks on page load.

---

# Problem

Currently, ironpad has several friction points:

1. **No Run All button**: Users must use the hidden `Ctrl+Shift+Enter` shortcut or run cells one-by-one. The view-only notebook has no run-all mechanism at all.

2. **No code sharing between cells**: Each cell compiles to an isolated WASM module. Functions defined in one cell can't be called from another, forcing users to duplicate code or put everything in a single cell.

3. **Auto-reactivity is unreliable**: Widget value changes are supposed to auto-trigger downstream re-execution, but the reactivity is broken in some cases. More fundamentally, automatic re-execution on every slider drag is undesirable for cells with network calls or heavy computation.

4. **Public notebooks require manual execution**: Visitors to public notebook pages see source code but no output — they must click Run on each cell individually. This is a poor showcase experience.

---

# Goals

1. Surface the existing run-all capability as a visible button in both editor and view-only contexts.
2. Enable code reuse across cells via a notebook-level shared Rust module (like shared deps).
3. Replace unreliable auto-reactivity with explicit button-triggered execution.
4. Make public/shared notebooks auto-execute on load for a zero-click experience.

---

# Technical Approach

## Run All Button

The editor already has `run_all_queue: RwSignal<Vec<String>>` and the `Ctrl+Shift+Enter` handler that populates it. Adding a toolbar button is trivial — wire it to the same logic.

For ViewOnlyNotebook, implement a similar queue-based sequential runner. The component already has `cell_outputs: RwSignal<HashMap<String, CellOutputData>>` for piping. Add a `run_all_queue: RwSignal<Vec<String>>` and drive execution from it.

## Shared Source Code

Follows the exact pattern of shared Cargo.toml:

```
IronpadNotebook.shared_source: Option<String>
    → CompileRequest.shared_source: Option<String>
        → scaffold_micro_crate(shared_source: Option<&str>)
            → writes src/shared.rs
            → adds `mod shared;` to lib.rs preamble
```

Users write shared functions/types in the shared source panel, then call them from any cell with `shared::my_function()`.

**UI**: A new SharedSourcePanel component (identical pattern to SharedDepsPanel), accessible from the gear menu. Uses a Monaco editor with `language="rust"`.

## Button Widget & Removing Auto-Reactivity

**Remove auto-reactivity**: Strip the `auto_run` check and downstream `run_all_queue` enqueue from `update_cell_output()`. Widget value changes still update `cell_outputs` bytes (preserving the value for the next execution), but don't trigger anything.

**Button widget**: New `ui::button(label)` builder in ironpad-cell that produces `DisplayPanel::Interactive { kind: "button", config }`. The button doesn't produce meaningful output bytes — it's a trigger, not a data source.

On click, the button enqueues all downstream Code cells into `run_all_queue`. It does NOT re-run its own cell (which would reset widget state). The downstream cells pick up the current widget output bytes as their input.

A typical pattern would be:

```rust
// Cell 1: Configuration widgets
ui::widgets((
    ui::slider(0.0, 100.0).label("Speed"),
    ui::dropdown(&["red", "green", "blue"]).label("Color"),
    ui::button("Run Simulation"),
))
```

(Note: multi-widget tuple support depends on existing `Into<CellOutput>` impls.)

## Auto-Execute Public/Shared Notebooks

Add an on-mount effect in ViewOnlyNotebook that populates the run-all queue with all code cell IDs. This triggers sequential compilation + execution automatically when the page loads.

---

# Assumptions

- The existing `run_all_queue` pattern in the editor is reliable and can be adapted for ViewOnlyNotebook.
- `mod shared;` in the lib.rs preamble correctly resolves to `src/shared.rs` in the micro-crate.
- Users will understand that shared source is a Rust module accessed via `shared::`.
- The button widget only needs to trigger downstream cells; it does not need to pass event data.

---

# Constraints

- Each cell remains an isolated WASM module — no cross-module function calls. Shared code is compiled into each cell independently.
- Shared source changes require recompilation of all cells (same as shared deps changes).
- The button widget requires the `WidgetContext` signals to enqueue downstream cells, which means it only works in contexts that provide those signals (editor, enhanced view-only).

---

# References to Code

- **Run All queue**: `crates/ironpad-app/src/pages/notebook_editor/state.rs:55` (`run_all_queue`)
- **Ctrl+Shift+Enter handler**: `crates/ironpad-app/src/pages/notebook_editor/mod.rs:99-111`
- **View mode auto-run**: `crates/ironpad-app/src/pages/notebook_editor/mod.rs:275-290`
- **Scaffold lib.rs generation**: `crates/ironpad-app/src/compiler/scaffold.rs:303-377`
- **Scaffold micro-crate**: `crates/ironpad-app/src/compiler/scaffold.rs:26-56`
- **Shared Cargo.toml merging**: `crates/ironpad-app/src/compiler/scaffold.rs:151-186`
- **SharedDepsPanel (UI pattern)**: `crates/ironpad-app/src/pages/notebook_editor/shared_deps.rs`
- **Widget builders**: `crates/ironpad-cell/src/ui.rs`
- **Widget rendering (editor)**: `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs:346-711`
- **Widget rendering (view-only)**: `crates/ironpad-app/src/components/view_only_notebook.rs:474-570`
- **Auto-reactivity logic**: `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs:301-344`
- **ViewOnlyNotebook**: `crates/ironpad-app/src/components/view_only_notebook.rs`
- **CompileRequest type**: `crates/ironpad-common/src/types.rs:10-22`
- **IronpadNotebook type**: `crates/ironpad-common/src/types.rs:123-149`

---

# Non-Goals (MVP)

- Cross-cell WASM function calls (linking modules at the WASM level)
- Caching compiled WASM blobs in .ironpad files
- Caching cell outputs in .ironpad files (deferred to future PRD)
- Multi-widget tuple syntax (e.g., `ui::widgets((slider, dropdown, button))`) — cells can return a single widget for now; users compose with multiple cells
- Debouncing widget value changes (not needed since auto-reactivity is removed)
- Shared source syntax highlighting / diagnostics in the shared source panel (Monaco's Rust mode gives basic highlighting)

---

# History

(Entries appended during implementation go below this line.)

## 2026-03-11 — Fleet Execution (T-001 through T-014)

- **Tasks completed**: T-001, T-002, T-003, T-004, T-005, T-006, T-007, T-008, T-009, T-010, T-011, T-012, T-013, T-014
- **Changes**:
  - T-001: Run All (▶▶) button in editor toolbar, triggers sequential execution via run_all_queue
  - T-002: Run All button in ViewOnlyNotebook with queue-based sequential execution
  - T-003: shared_source field added to IronpadNotebook, CompileRequest, NotebookState
  - T-004: Scaffold writes src/shared.rs, adds `mod shared;` to preamble, adjusts preamble_lines
  - T-005: SharedSourcePanel component (Monaco editor, gear menu entry)
  - T-006: Removed auto_run field and all auto-reactivity logic from widgets
  - T-007: Button widget (`ui::button()`) in ironpad-cell with builder pattern
  - T-008: render_button in cell_output.rs, triggers downstream cell re-execution on click
  - T-009: Button widget rendering in ViewOnlyNotebook with downstream execution
  - T-010: Auto-execute all code cells on public/shared notebook page load (one-shot Effect)
  - T-011: shared_source threaded through view-only compile requests
  - T-012: New public notebooks: shared-code.ironpad, interactive-button.ironpad
  - T-013: 4 new scaffold tests for shared source
  - T-014: 4 new button widget tests
- **Test results**: cargo make ci passed — 233/233 tests, clippy clean, fmt clean
- **UATs verified**: uat-010 (cargo make ci passes)
- **UATs deferred**: uat-001 through uat-009 (require running app or playwright)
- **Constitution compliance**: No violations

---
