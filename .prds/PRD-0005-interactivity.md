---
id: PRD-0005
title: "Marimo-Inspired Interactivity & UI Polish"
status: draft
owner: "Aaron Roney"
created: 2026-03-10
updated: 2026-03-10

principles:
- "Keep the existing ironpad aesthetic — enhance, don't replace"
- "Interactive elements should be as easy to use as Plot and Json"
- "Reactivity should feel instant — re-execute, never recompile"
- "Auto-run is opt-in via toggle; stale marking remains the default"
- "All new UI elements must work in both edit and view modes"

references:
- name: "marimo.io"
  url: https://marimo.io/
- name: "marimo GitHub"
  url: https://github.com/marimo-team/marimo
- name: "PRD-0004 (builtins/tests/refactor)"
  url: .prds/PRD-0004-builtins-tests-refactor.md

acceptance_tests:
- id: uat-001
  name: "Edit/view mode toggle switches between code-visible and code-hidden"
  command: cargo make uat
  uat_status: unverified
- id: uat-002
  name: "Notebook toolbar shows hamburger menu with Share/Export/Delete and gear with Shared Deps"
  command: cargo make uat
  uat_status: unverified
- id: uat-003
  name: "Running a cell with auto-run enabled executes all downstream code cells"
  command: cargo make uat
  uat_status: unverified
- id: uat-004
  name: "Per-cell run and menu buttons render on the right side of the cell"
  command: cargo make uat
  uat_status: unverified
- id: uat-005
  name: "View mode collapses code by default, showing only outputs"
  command: cargo make uat
  uat_status: unverified
- id: uat-006
  name: "ui::slider renders an interactive slider and produces a typed output value"
  command: cargo make uat
  uat_status: unverified
- id: uat-007
  name: "Changing a ui element value re-executes downstream cells without recompilation"
  command: cargo make uat
  uat_status: unverified

tasks:
- id: T-001
  title: "Toolbar reorganization: hamburger, gear, and close button"
  priority: 1
  status: todo
  notes: >
    Move notebook-level actions into two dropdown menus in the upper-right corner:
    (1) Hamburger (☰): Share, Export HTML, Delete (with confirmation).
    (2) Gear (⚙): Shared Deps toggle (opens existing SharedDepsPanel).
    Add an X button (rightmost) that navigates back to notebook list (use_navigate to /).
    Keep Run Stale as a visible standalone button (primary action).
    The title input stays in the center/left of the toolbar.
    New CSS classes: .ironpad-toolbar-right, .ironpad-toolbar-dropdown, .ironpad-toolbar-dropdown-menu.
    Files: mod.rs (toolbar section lines 258-383), main.scss (toolbar section lines 525-530).

- id: T-002
  title: "Edit/view mode toggle"
  priority: 1
  status: todo
  notes: >
    Add a toggle button in the bottom-left corner of the notebook editor (fixed position, like marimo).
    Two modes: Edit (pencil icon, default) and View (eye icon).
    Store as RwSignal<bool> (is_view_mode) in NotebookState.
    In view mode: code editors and tab bars are hidden, only output panels are visible.
    In edit mode: full editor experience (current behavior).
    The toggle should be clearly visible with a tooltip.
    New CSS classes: .ironpad-mode-toggle, .ironpad-cell--view-mode.
    Files: state.rs (add is_view_mode signal), mod.rs (toggle component), cell_item.rs (conditional rendering), main.scss.

- id: T-003
  title: "Per-cell action buttons on right side"
  priority: 1
  status: todo
  notes: >
    Reposition per-cell action buttons to the right side of the cell card, outside the cell body.
    Layout: cell card takes most of the width, action buttons sit in a narrow vertical strip to the right.
    Buttons (top to bottom): Run (▶), Cell Settings (⚙ — opens cargo.toml panel inline), Menu (⋯ — existing dropdown).
    The cell settings button toggles a cargo.toml editor panel below the code editor within the cell.
    Remove the run button and menu from the cell header; keep collapse, order badge, label, stale indicator, and status.
    Use flexbox: .ironpad-cell-row { display: flex; } with .ironpad-cell-card { flex: 1; } and .ironpad-cell-side-actions { width: 36px; }.
    The side actions should only be visible on hover or when the cell is active, to keep the UI clean.
    Files: cell_item.rs (restructure component), main.scss (new layout classes).

- id: T-004
  title: "Auto-run downstream cells (toggleable)"
  priority: 2
  status: todo
  notes: >
    Add a toggle in the notebook toolbar (visible, next to Run Stale) for "Auto-run" mode.
    When auto-run is ON: after a cell completes successfully, automatically enqueue all subsequent
    Code cells into run_all_queue (reuse the existing queue mechanism).
    When auto-run is OFF (default): current behavior — downstream cells are marked stale but not executed.
    Store as RwSignal<bool> (auto_run) in NotebookState.
    Persist the toggle state in the notebook metadata or localStorage.
    The auto-run trigger should happen in the execution completion handler in cell_item.rs (around line 573-586).
    Files: state.rs (add auto_run signal), mod.rs (toolbar toggle), cell_item.rs (execution completion handler).

- id: T-005
  title: "Code collapsed by default in view mode"
  priority: 2
  status: todo
  notes: >
    When is_view_mode is true, all cell code sections should be collapsed (hidden).
    Only the output panel, cell label, and status badge remain visible.
    The collapse toggle button should still be available so users can peek at code if needed.
    When switching from view mode back to edit mode, restore the user's previous collapse states.
    Implementation: store pre-view-mode collapse states in a HashMap, apply view mode overrides,
    restore on exit. The collapse transition CSS already exists (.ironpad-cell-body--collapsed).
    Depends on T-002 (edit/view mode toggle).
    Files: cell_item.rs (collapse logic), state.rs (collapse state storage).

- id: T-006
  title: "Interactive UI elements in ironpad-cell"
  priority: 2
  status: todo
  notes: >
    Add a new ui module to ironpad-cell with 6 widget builders:
    - ui::slider(min, max) -> CellOutput (default: min, output type: f64)
    - ui::dropdown(options: &[&str]) -> CellOutput (default: first option, output type: String)
    - ui::checkbox(label) -> CellOutput (default: false, output type: bool)
    - ui::text_input(placeholder) -> CellOutput (default: "", output type: String)
    - ui::number(min, max) -> CellOutput (default: min, output type: f64)
    - ui::switch(label) -> CellOutput (default: false, output type: bool)

    Each builder returns CellOutput with:
    - bytes: bincode-serialized default value
    - panels: vec![DisplayPanel::Interactive { kind: "slider"|"dropdown"|..., config: serde_json::Value }]
    - type_tag: the output type name ("f64", "String", "bool")

    Add DisplayPanel::Interactive { kind: String, config: String } variant to the enum.
    The config is a JSON string containing widget-specific params (min, max, options, label, placeholder, etc.).

    Auto-import via prelude: `use ironpad_cell::prelude::*` already imports everything; add `pub mod ui` to prelude.

    Each builder should support chaining: ui::slider(1.0, 10.0).step(0.5).label("Speed").default_value(5.0).

    Unit tests for each widget type: verify default value serialization, display panel output, type tags.
    Files: crates/ironpad-cell/src/ui.rs (new), crates/ironpad-cell/src/lib.rs (DisplayPanel variant, prelude).

- id: T-007
  title: "Frontend rendering of interactive elements"
  priority: 2
  status: todo
  notes: >
    In cell_output.rs, add rendering for DisplayPanel::Interactive.
    Parse the config JSON and render the appropriate HTML widget:
    - Slider: <input type="range" min="..." max="..." step="..." value="...">
    - Dropdown: <select><option>...</option></select>
    - Checkbox: <input type="checkbox"> with label
    - Text Input: <input type="text" placeholder="...">
    - Number: <input type="number" min="..." max="..." value="...">
    - Switch: <label class="ironpad-switch"><input type="checkbox">...</label>

    Each widget fires a callback on value change that:
    1. Serializes the new value with bincode (via a JS helper or inline WASM utility)
    2. Updates cell_outputs[cell_id].bytes with the new serialized value
    3. Triggers reactivity (T-008)

    The widget value display should also be rendered (e.g., slider shows current value next to it).

    Need to add bincode serialization capability to the frontend. Options:
    (a) Compile a tiny WASM helper that exposes bincode_serialize_f64, bincode_serialize_string, etc.
    (b) Implement bincode v2 fixed-int encoding in JS (simpler for primitives: f64 is just 8 LE bytes,
        bool is 1 byte, String is varint length + UTF-8 bytes).
    Recommend option (b) for MVP — bincode2 with fixed-int encoding is simple for primitive types.

    Add to export.rs DisplayPanel enum as well for HTML export (render as static values).
    Also add to view_only_notebook.rs for public/shared notebook rendering (read-only display).

    Depends on T-006.
    Files: cell_output.rs, export.rs, view_only_notebook.rs, public/executor.js or new public/bincode.js, main.scss.

- id: T-008
  title: "Reactivity: re-execute downstream on UI element change"
  priority: 3
  status: todo
  notes: >
    When an interactive element's value changes (T-007 callback fires):
    1. Update cell_outputs[cell_id] with new serialized bytes (already done in T-007)
    2. Mark all downstream Code cells as stale
    3. If auto_run is ON (T-004), enqueue all downstream Code cells for re-execution
    4. If auto_run is OFF, just show stale indicators (user clicks Run Stale or individual run)

    Re-execution uses the existing execution pipeline: downstream cells are already compiled,
    so execute_cell in executor.js is called with the updated inputs — no recompilation needed.
    The run_all_queue mechanism already handles sequential execution.

    Key implementation detail: the downstream cells' inputs change because cell_outputs[upstream_cell_id].bytes
    changed. The CellInputs wire format assembly (cell_item.rs lines 414-452) already reads from
    cell_outputs for all preceding Code cells, so re-execution will automatically pick up the new value.

    Edge case: if a downstream cell hasn't been compiled yet (never run), it can't be re-executed.
    In this case, mark it stale but don't attempt execution.

    Depends on T-004, T-006, T-007.
    Files: cell_item.rs (reactivity trigger), state.rs (stale marking helper), cell_output.rs (callback wiring).
---

# Summary

Add marimo-inspired interactivity features to ironpad: a cleaner toolbar layout, edit/view mode toggle,
auto-run downstream cells, per-cell right-side action buttons, interactive UI elements (slider, dropdown,
checkbox, text input, number input, switch), and reactive re-execution when UI element values change.

# Problem

ironpad currently has a flat toolbar with all notebook-level buttons inline, no distinction between
editing and viewing, no auto-execution of downstream cells, and no interactive UI elements. These
limitations make the notebook feel static compared to modern notebook environments like marimo.

Users want to:
1. Present clean, output-focused views of their notebooks
2. Have downstream cells update automatically when upstream cells change
3. Build interactive dashboards with sliders, dropdowns, and other controls
4. Have a more organized toolbar that scales as features grow

# Goals

1. Reorganize the notebook toolbar into logical groupings (actions, settings, navigation)
2. Add edit/view mode toggle for presentation-ready notebooks
3. Enable automatic downstream execution with a toggleable auto-run mode
4. Move per-cell buttons to the right side for a cleaner cell layout (like marimo)
5. Implement 6 interactive UI widgets as easy-to-use builder functions
6. Enable reactive re-execution when interactive elements change values

# Technical Approach

## Toolbar Reorganization (T-001)

Current toolbar is a flat row: `[Shared Deps] [Run Stale] [Share] [Export] [Delete]`.

New layout: `[Title input ...] [Run Stale] [Auto-Run toggle] [☰ hamburger] [⚙ gear] [✕ close]`

- **Hamburger (☰)**: Share, Export HTML, Delete — infrequent "file" actions
- **Gear (⚙)**: Shared Deps — configuration/settings
- **Close (✕)**: Navigate back to `/` — always visible

Dropdowns use a shared `ToolbarDropdown` mini-component with outside-click-to-close.

## Edit/View Mode (T-002, T-005)

A fixed-position toggle button in the bottom-left corner. Uses a `RwSignal<bool>` on `NotebookState`.

In view mode:
- Cell code editors, tab bars, and cargo.toml panels are hidden
- Cell headers show only: label, order badge, status
- Output panels are prominently displayed
- Side action buttons are hidden
- Collapse toggle still available for peeking at code

Switching back to edit mode restores previous collapse states.

## Per-Cell Right-Side Buttons (T-003)

Change the cell layout from:

```
┌─────────────────────────────────────────┐
│ [▾] [0] [label...] [stale] [status] [▶] [⋯] │
│ [code editor]                              │
│ [output]                                   │
└─────────────────────────────────────────┘
```

To:

```
┌─────────────────────────────────────┐ ┌──┐
│ [▾] [0] [label...] [stale] [status] │ │▶ │
│ [code editor]                        │ │⚙ │
│ [output]                             │ │⋯ │
└─────────────────────────────────────┘ └──┘
```

Side buttons appear on hover or when cell is active. `⚙` toggles an inline cargo.toml editor.

## Auto-Run Downstream (T-004)

Toggleable mode stored in `NotebookState.auto_run: RwSignal<bool>`.

When a cell completes successfully and auto_run is true:
1. Collect all subsequent Code cell IDs
2. Filter to only cells that have been compiled at least once (have a cached WASM blob)
3. Push them into `run_all_queue`
4. Existing queue processing handles sequential execution

When auto_run is false: current behavior (mark stale, user runs manually).

## Interactive UI Elements (T-006, T-007, T-008)

### Cell-Side (ironpad-cell)

New `ui` module with builder functions:

```rust
// User writes:
ui::slider(1.0, 10.0).step(0.5).label("Speed")

// Produces CellOutput {
//   bytes: bincode::encode(1.0f64),   // default value
//   panels: vec![DisplayPanel::Interactive {
//     kind: "slider".into(),
//     config: r#"{"min":1.0,"max":10.0,"step":0.5,"label":"Speed","default":1.0}"#.into(),
//   }],
//   type_tag: Some("f64"),
// }
```

New `DisplayPanel::Interactive { kind: String, config: String }` variant.

### Frontend Rendering

Each widget type maps to HTML form elements. On value change:

1. Serialize new value to bincode bytes (JS implementation for primitives)
2. Update `cell_outputs[cell_id]` with new bytes
3. Mark downstream cells stale
4. If auto_run: enqueue downstream cells for execution

### Bincode JS Implementation

For MVP, implement bincode2 fixed-int encoding for primitives in JS:
- `f64` → 8 bytes little-endian
- `bool` → 1 byte (0x00 or 0x01)
- `String` → varint length prefix + UTF-8 bytes

This avoids needing a separate WASM helper module.

### Reactivity Flow

```
User moves slider
  → JS callback fires
  → Serialize new f64 value to bincode bytes
  → Update cell_outputs[slider_cell_id].bytes
  → Mark downstream cells stale
  → If auto_run ON: push downstream cell IDs to run_all_queue
  → Queue processor executes each cell with updated CellInputs
  → Downstream outputs update reactively
```

No recompilation needed — the WASM modules are already cached.

# Assumptions

- Leptos `RwSignal` updates will trigger reactive re-renders for downstream cell outputs
- bincode2 fixed-int encoding for f64/bool/String is stable and simple enough for JS implementation
- The existing `run_all_queue` mechanism can handle both user-initiated and auto-triggered runs
- WASM modules remain cached after first compilation (existing cache behavior)

# Constraints

- Interactive elements only produce primitive types (f64, bool, String) — no complex structs for MVP
- Reactivity is linear (all subsequent cells), not DAG-based (no variable-level dependency tracking)
- Auto-run only works for cells that have been compiled at least once
- JS bincode implementation only covers the 3 primitive types needed for the 6 widgets

# References to Code

- **Toolbar**: `mod.rs` lines 258-383
- **Per-cell controls**: `cell_item.rs` lines 1126-1300 (header), 1214-1297 (actions/menu)
- **Execution flow**: `cell_item.rs` lines 290-654
- **Queue processing**: `cell_item.rs` lines 578-586 (queue advancement)
- **Stale tracking**: `cell_item.rs` lines 454-473 (invalidation), 573-576 (stale clear)
- **State signals**: `state.rs` lines 34-66 (NotebookState)
- **Display rendering**: `cell_output.rs` lines 90-215
- **DisplayPanel enum**: `ironpad-cell/src/lib.rs` lines 158-172
- **CellOutput builder**: `ironpad-cell/src/lib.rs` lines 227-318
- **Executor**: `public/executor.js` lines 88-99 (execute), 233-270 (result reading)
- **Cell body CSS**: `main.scss` lines 789-801 (collapse transition)

# Non-Goals (MVP)

- **DAG-based reactivity**: No static analysis of variable references; reactivity is positional (all subsequent cells)
- **Complex type UI elements**: No struct/enum/Vec widget builders — primitives only
- **Persistent view mode**: View mode is session-only, not saved to notebook metadata
- **Cell drag and drop**: Reordering cells via drag — deferred to future PRD
- **Mobile / responsive layout**: Side buttons may not work well on narrow screens — future polish

# History
(Entries appended during implementation go below this line.)
