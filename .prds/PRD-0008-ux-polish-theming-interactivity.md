---
id: PRD-0008
title: "UX Polish: Theming, Drag-Drop, Plot Interactivity, Progress Bar & Host Messaging"
status: done
owner: "Aaron Roney"
created: 2026-03-11
updated: 2026-03-11

depends_on:
- PRD-0007

principles:
- "CSS variables for all colors — theme switching is a variable swap, not a rewrite"
- "Host messaging is generic — progress bar is the first consumer, not the last"
- "The executor is the active dispatcher — DOM updates are imperative, not event-based"
- "Plot interactivity is opt-in via builder methods, SVG-native (no JS dependencies)"
- "Collapsible sections default to collapsed — reduce noise, reveal on demand"

references:
- name: "Main SCSS"
  url: style/main.scss
- name: "executor.js"
  url: public/executor.js
- name: "Plot module"
  url: crates/ironpad-cell/src/plot.rs
- name: "Cell runtime"
  url: crates/ironpad-cell/src/lib.rs
- name: "Widget UI"
  url: crates/ironpad-cell/src/ui.rs
- name: "Cell output panel (editor)"
  url: crates/ironpad-app/src/pages/notebook_editor/cell_output.rs
- name: "Cell item (editor)"
  url: crates/ironpad-app/src/pages/notebook_editor/cell_item.rs
- name: "Notebook editor"
  url: crates/ironpad-app/src/pages/notebook_editor/mod.rs
- name: "View-only notebook"
  url: crates/ironpad-app/src/components/view_only_notebook.rs

acceptance_tests:
- id: uat-001
  name: "Run All button has a styled appearance (gradient/icon, not plain text)"
  command: cargo make uat
  uat_status: unverified
- id: uat-002
  name: "Cells can be reordered via drag-and-drop using a drag handle"
  command: cargo make uat
  uat_status: unverified
- id: uat-003
  name: "Light mode toggle switches all colors via CSS variable overrides"
  command: cargo make uat
  uat_status: unverified
- id: uat-004
  name: "Theme preference persists across page reloads via localStorage"
  command: cargo make uat
  uat_status: unverified
- id: uat-005
  name: "Monaco editor switches between dark/light themes when mode toggles"
  command: cargo make uat
  uat_status: unverified
- id: uat-006
  name: "No 'deprecated parameters' console warning in CellExecutor.loadBlob"
  command: cargo make uat
  uat_status: unverified
- id: uat-007
  name: "Plot tooltips show data point values on hover (SVG <title> elements)"
  command: cargo make uat
  uat_status: unverified
- id: uat-008
  name: "Plot.tooltip(true) enables tooltips; default is off for backward compat"
  command: cargo make uat
  uat_status: unverified
- id: uat-009
  name: "Progress bar widget renders and updates in real-time during cell execution"
  command: cargo make uat
  uat_status: unverified
- id: uat-010
  name: "host_message FFI is available to cells and dispatched by executor.js"
  command: cargo make uat
  uat_status: unverified
- id: uat-011
  name: "Raw output section is collapsed by default and expandable"
  command: cargo make uat
  uat_status: unverified
- id: uat-012
  name: "All changes pass cargo make ci"
  command: cargo make ci
  uat_status: unverified

tasks:
# ── P1: Quick wins ──────────────────────────────────────────────────────────

- id: T-019
  title: "Style the Run All button"
  priority: 1
  status: done
  notes: >
    In style/main.scss, add proper styles for `.ironpad-run-all-button`:
    gradient background using --ip-accent, rounded corners, hover/active
    states, an icon (▶▶ or play icon), padding, font styling. Also style
    the view-only `.run-all-button`. The button should feel like a primary
    action button — prominent but not garish.

- id: T-020
  title: "Make raw output collapsible (collapsed by default)"
  priority: 1
  status: done
  notes: >
    In cell_output.rs (editor) and view_only_notebook.rs: wrap the raw
    output hex dump section in an HTML <details>/<summary> element.
    Default to closed. Summary text: "Raw output (N bytes)". The hex dump
    <pre> goes inside. Style the <details> to match the existing theme.

- id: T-021
  title: "Fix deprecated wasm-bindgen init parameters"
  priority: 1
  status: done
  notes: >
    In public/executor.js line 59, change:
      `var wasm = await mod.default(wasmBytes);`
    to:
      `var wasm = await mod.default({ module_or_path: wasmBytes });`
    This fixes the console warning about deprecated initialization
    parameters. The wasm-bindgen init function now expects a single
    options object instead of positional parameters.

# ── P2: Light/dark theme ────────────────────────────────────────────────────

- id: T-022
  title: "Add light theme CSS variables"
  priority: 2
  status: done
  notes: >
    In style/main.scss, add a `[data-theme="light"]` selector block that
    overrides ALL --ip-* CSS variables with light-appropriate values:
    light backgrounds (#f5f6fa, #ffffff, #eef0f5), dark text (#1a1a2e,
    #333, #666), adjusted accent, borders, etc. Also override the Thaw
    component variables. Ensure ALL colors in the file use CSS variables
    (audit for any remaining hardcoded colors and convert them).

- id: T-023
  title: "Add theme toggle button to notebook toolbar"
  priority: 2
  status: done
  notes: >
    Add a 🌙/☀ toggle button in the upper-right area of the notebook
    toolbar (mod.rs). On click: toggle `data-theme` attribute on
    <html> element between absent (dark default) and "light". Persist
    choice to localStorage key "ironpad-theme". On page load, read
    localStorage and apply. Also tell Monaco editor to switch theme
    via window.IronpadMonaco JS bridge (if available). Add the toggle
    to ViewOnlyNotebook toolbar as well.

- id: T-024
  title: "Theme initialization script"
  priority: 2
  status: done
  notes: >
    Add a small inline <script> in the HTML head (or a public JS file)
    that reads localStorage("ironpad-theme") and sets data-theme on
    <html> before first paint to avoid FOUC (flash of unstyled content).
    This must run synchronously before CSS is evaluated.

# ── P2: Drag and drop ──────────────────────────────────────────────────────

- id: T-025
  title: "Add SortableJS dependency"
  priority: 2
  status: done
  notes: >
    Add SortableJS to the project. Options: (a) npm install sortablejs
    and include via a <script> tag from node_modules or a CDN, or
    (b) vendor the minified JS into public/. Prefer the npm approach
    with a bundled or direct script include. The library should be
    available as window.Sortable.

- id: T-026
  title: "Add drag handle to cell side actions"
  priority: 2
  status: done
  notes: >
    In cell_item.rs, add a drag handle element (⠿ or ☰ icon) to the
    .ironpad-cell-side-actions div. It should appear above or below the
    existing run/gear/menu buttons. Give it a CSS class like
    .ironpad-drag-handle and style it (cursor: grab, etc.). This element
    will be the SortableJS handle.

- id: T-027
  title: "Initialize SortableJS on the cell list"
  priority: 2
  status: done
  notes: >
    In mod.rs (NotebookContent), after the cell list renders, initialize
    SortableJS on the container div that holds the <For> loop of cells.
    Configuration: handle=".ironpad-drag-handle", animation=150,
    ghostClass for a visual placeholder. On sort end: read the new order
    from SortableJS, update state.notebook cell ordering, and call
    persist_notebook. Must be hydrate-gated. Use an Effect that runs
    once on mount to initialize. Disable in view mode.

# ── P2: Plot interactivity ──────────────────────────────────────────────────

- id: T-028
  title: "Add tooltip builder methods to Plot"
  priority: 2
  status: done
  notes: >
    In plot.rs, add a `tooltips` field (bool, default false) and a
    `.tooltips(enabled: bool)` builder method to Plot. When enabled,
    the render functions should add SVG <title> elements to each data
    point/bar element with the data values. For line/scatter: add
    <title> to each circle/point with "(x, y)" text. For bar: add
    <title> to each rect with "(label, value)" text. This uses native
    SVG tooltips — no JS needed. Also consider adding a
    `.point_labels(enabled: bool)` method that renders data values as
    SVG <text> elements directly on the chart.

- id: T-029
  title: "Add CSS hover effects for plot elements"
  priority: 2
  status: done
  notes: >
    In style/main.scss, add CSS rules for SVG elements inside
    .ironpad-output-svg (or wherever plots render): on hover, increase
    opacity or add a glow/highlight effect to circles, rects, and
    polyline segments. Use CSS transitions for smoothness. This gives
    visual feedback even without JS.

- id: T-030
  title: "Update plot example notebooks"
  priority: 3
  status: done
  notes: >
    Update the charts-with-plot.ironpad example notebook to demonstrate
    the new tooltip and point_labels features. Add a cell showing
    `.tooltips(true)` and another showing `.point_labels(true)`.

# ── P2: Host messaging & progress bar ──────────────────────────────────────

- id: T-031
  title: "Add host_message FFI to ironpad-cell"
  priority: 2
  status: done
  notes: >
    In ironpad-cell/src/lib.rs, declare a host-imported FFI function:
      extern "C" { fn ironpad_host_message(ptr: *const u8, len: u32); }
    Add a safe Rust wrapper: `pub fn host_message(msg: &str)` that
    serializes the pointer/length and calls the FFI. This is the generic
    channel for cells to communicate with the host during execution.
    Gate behind `#[cfg(target_arch = "wasm32")]` with a no-op fallback
    for native/test builds.

- id: T-032
  title: "Implement host_message import in executor.js"
  priority: 2
  status: done
  notes: >
    In public/executor.js, when loading a wasm-bindgen module, provide
    `ironpad_host_message` as an import. The function reads the JSON
    string from WASM memory, parses it, and dispatches based on the
    "type" field. First handler: "progress_update" — finds the DOM
    element for the referenced progress bar and calls its update method.
    The import must be threaded through the wasm-bindgen init path.
    Design the import mechanism to work with wasm-bindgen's
    `__wbindgen_init_externref_table` pattern.

- id: T-033
  title: "Add progress bar widget to ironpad-cell"
  priority: 2
  status: done
  notes: >
    In ironpad-cell/src/ui.rs, add a ProgressBar builder:
      pub struct ProgressBar { label: Option<String>, initial: f64, id: String }
    The `id` is auto-generated (e.g., uuid or counter). The progress bar
    produces DisplayPanel::Interactive { kind: "progress", config } with
    the id in config. It also provides an `.update(value: f64)` method
    on the CellInput side — when a downstream cell has a progress bar
    in its input, calling `.update(value)` sends a host_message like:
      {"type":"progress_update","id":"<progress-id>","value":10.0}
    The update method should be on a ProgressHandle type returned by
    deserializing the upstream output. Add to prelude.

- id: T-034
  title: "Render progress bar widget in editor and view-only"
  priority: 2
  status: done
  notes: >
    In cell_output.rs and view_only_notebook.rs, add rendering for
    kind="progress" widgets. Render as an HTML progress bar element
    with a data-progress-id attribute. The DOM element should have a
    method or data attribute that executor.js can use to update the
    value. Use a CSS-styled progress bar (div with inner fill div)
    rather than <progress> for better styling control. Include the
    label and a percentage display.

- id: T-035
  title: "Wire executor progress_update handler to DOM"
  priority: 2
  status: done
  notes: >
    In executor.js, implement the progress_update message handler:
    when a host_message with type="progress_update" arrives, find the
    DOM element with data-progress-id matching the message id, and
    update its width/value. This is imperative — the executor directly
    manipulates the DOM element, the progress bar doesn't listen for
    events. Handle the case where the element doesn't exist yet (cell
    not rendered) gracefully.

# ── P3: Polish & examples ──────────────────────────────────────────────────

- id: T-036
  title: "Add progress bar example notebook"
  priority: 3
  status: done
  notes: >
    Create a public/notebooks/progress-bar.ironpad example that
    demonstrates: (1) a cell creating a progress bar, (2) a subsequent
    cell updating it in a loop during execution. Update index.json.

---

# Summary

This PRD covers seven UX improvements: (1) styled Run All button, (2) drag-and-drop cell reordering via SortableJS, (3) light/dark theme toggle with CSS variables, (4) fix deprecated wasm-bindgen init, (5) SVG-native plot interactivity (tooltips, hover effects), (6) progress bar widget with real-time updates via a generic host messaging FFI, and (7) collapsible raw output.

---

# Problem

Several UX friction points remain after PRD-0007:

1. **Run All button is unstyled** — plain text button with no visual prominence. Doesn't look like a primary action.

2. **Cell reordering is cumbersome** — only Move Up/Down via the ⋯ menu. No drag-and-drop despite this being a standard notebook interaction.

3. **No light mode** — dark theme only. Some users prefer light mode, especially in bright environments. The CSS variable foundation already exists but there's no light theme defined or toggle mechanism.

4. **Console warning** — `using deprecated parameters for the initialization function; pass a single object instead` in `CellExecutor.loadBlob` when initializing wasm-bindgen modules.

5. **Static plots** — SVG charts have zero interactivity. No tooltips, no hover feedback. Other notebook products (Observable, Jupyter+Plotly) make data exploration a first-class feature.

6. **No real-time cell communication** — cells run to completion with no way to send intermediate updates to the host. This prevents progress indicators, streaming output, and other live feedback patterns.

7. **Raw output always visible** — hex dump is always expanded, adding noise to the output panel. Most users don't need to see raw bytes.

---

# Technical Approach

## Run All Button Styling (T-019)

Pure CSS task. Add `.ironpad-run-all-button` styles with:
- Gradient background: `linear-gradient(135deg, var(--ip-accent), var(--ip-accent-hover))`
- Rounded corners, padding, white text, hover/active states
- Subtle box-shadow for depth
- Icon + text layout

## Collapsible Raw Output (T-020)

Replace the raw output `<div>` wrapper with `<details>/<summary>`. The `<summary>` shows "Raw output (N bytes)" and the `<pre>` hex dump is inside. Apply in both cell_output.rs (editor) and view_only_notebook.rs.

## Fix Deprecated Init (T-021)

One-line change in executor.js: pass `{ module_or_path: wasmBytes }` instead of bare `wasmBytes` to the wasm-bindgen init function.

## Light/Dark Theme (T-022, T-023, T-024)

**CSS**: Add `[data-theme="light"]` block overriding all `--ip-*` variables. Audit for hardcoded colors.

**Toggle**: Button in toolbar toggles `data-theme` attribute on `<html>`. Persists to localStorage. Also switches Monaco editor theme.

**FOUC prevention**: Tiny inline script before CSS loads that reads localStorage and sets the attribute.

## Drag-and-Drop (T-025, T-026, T-027)

Add SortableJS (npm). Add a drag handle (⠿) to each cell's side actions. Initialize SortableJS on the cell container after mount. On sort end, update notebook cell order and persist. Disabled in view mode.

## Plot Interactivity (T-028, T-029, T-030)

**SVG-native tooltips**: When `.tooltips(true)` is set, each data element (circle, rect, line point) gets a `<title>` child element with the data value. Browsers render this as a native tooltip on hover.

**Point labels**: When `.point_labels(true)` is set, render `<text>` elements at each data point showing the value.

**CSS hover effects**: Add `:hover` styles for SVG plot elements (opacity change, glow) for visual feedback.

All opt-in via builder methods, off by default for backward compatibility.

## Host Messaging & Progress Bar (T-031–T-035)

This is the most architecturally significant feature. Four layers:

### Layer 1: FFI (T-031)
Cell runtime declares `extern "C" { fn ironpad_host_message(ptr, len); }` — a generic channel for sending JSON messages to the host during execution.

### Layer 2: Executor dispatch (T-032, T-035)
executor.js provides `ironpad_host_message` as a WASM import. On call, it reads the JSON from WASM memory, parses it, and dispatches by `type` field. First handler: `progress_update` — finds DOM element by `data-progress-id` and updates it imperatively.

### Layer 3: Widget API (T-033)
`ui::progress_bar()` produces an Interactive panel with kind="progress". The key innovation: when a downstream cell deserializes the upstream output, it gets a `ProgressHandle` that has an `.update(value)` method. This method calls `host_message({"type":"progress_update","id":"...","value":...})` under the hood.

### Layer 4: Rendering (T-034)
Progress bar renders as a styled div with `data-progress-id` attribute. The executor's progress_update handler finds this element and sets the fill width.

### Wire format
```json
{"type": "progress_update", "id": "progress-abc123", "value": 42.0}
```

## Collapsible Raw Output (T-020)

Wrap in `<details>/<summary>`, collapsed by default. Both editor and view-only.

---

# Assumptions

- SortableJS works well with Leptos's `<For>` rendering (DOM nodes are stable, keyed by cell ID).
- SVG `<title>` elements provide adequate tooltip UX (native browser rendering).
- wasm-bindgen's import mechanism allows injecting custom imports (ironpad_host_message) alongside the generated glue code.
- The `data-progress-id` DOM query approach is sufficient for the executor to find progress bars (no race conditions with rendering).

---

# Constraints

- Plot interactivity is SVG-native only — no JavaScript interactivity layer for plots in this PRD.
- Host messaging is fire-and-forget (cell → host); no response channel from host → cell.
- SortableJS adds ~15KB (minified) to the frontend bundle.
- Light theme requires manual color tuning — automated inversion would look poor.
- The progress bar update mechanism requires wasm-bindgen path (not legacy raw WASM).

---

# Non-Goals (MVP)

- Zoom/pan on plots (would require JS layer)
- Click-to-select data points on plots
- Bidirectional host messaging (host → cell during execution)
- Real-time streaming text output during cell execution (only progress bar updates)
- Mobile-optimized drag/drop (SortableJS has touch support but we won't test/optimize for mobile)
- Theme scheduling (auto-switch based on time of day)
- Custom user themes beyond light/dark

---

# History

(Entries appended during implementation go below this line.)

---
