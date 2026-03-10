---
id: PRD-0003
title: "Cell Output Enhancements & Editor UX"
status: draft
owner: Aaron Roney
created: 2026-03-10
updated: 2026-03-10

depends_on:
- PRD-0002

principles:
- "New display types follow the exact Html/Svg pattern: newtype in ironpad-cell, DisplayPanel variant, IntoPanels + TypeTag + From impls"
- "Markdown-to-HTML conversion happens client-side in the renderer, not in the cell — cells emit raw markdown strings"
- "Keyboard shortcuts must not conflict with Monaco editor's built-in bindings"
- "Export produces a self-contained HTML file with no external dependencies"
- "Table type uses structured data (headers + rows), not raw HTML — the renderer owns the markup"

references:
- name: "MegaPrd (architecture, design, all details)"
  url: "MegaPrd.md"
- name: "PRD-0001 (MVP first pass)"
  url: ".prds/PRD-0001-first-pass.md"
- name: "PRD-0002 (visual fixes)"
  url: ".prds/PRD-0002-visual-fixes.md"
- name: "pulldown-cmark (markdown rendering)"
  url: "https://docs.rs/pulldown-cmark/latest/pulldown_cmark/"

acceptance_tests:
- id: uat-001
  name: "Md('# Hello') produces rendered markdown HTML in cell output"
  command: cargo make test
  uat_status: unverified
- id: uat-002
  name: "Table::new with headers and rows renders a styled HTML table"
  command: cargo make test
  uat_status: unverified
- id: uat-003
  name: "Ctrl+Enter runs the focused cell"
  command: cargo make uat
  uat_status: unverified
- id: uat-004
  name: "Copy button on output panel copies text to clipboard"
  command: cargo make uat
  uat_status: unverified
- id: uat-005
  name: "Export button downloads a self-contained HTML file of the notebook"
  command: cargo make uat
  uat_status: unverified
- id: uat-006
  name: "cargo make ci passes with all changes"
  command: cargo make ci
  uat_status: unverified

tasks:
  # ── Feature 1: Md display type ─────────────────────────────────────────
  - id: T-001
    title: "Add Md newtype and DisplayPanel::Markdown variant to ironpad-cell"
    priority: 1
    status: done
    notes: >
      In crates/ironpad-cell/src/lib.rs:
      1. Add `pub struct Md(pub String);` newtype (like Html/Svg).
      2. Add `Markdown(String)` variant to `DisplayPanel` enum.
      3. Implement From<Md> for CellOutput, IntoPanels for Md, TypeTag for Md.
      4. Export Md from the prelude.
      5. Add unit tests for Md → CellOutput → JSON round-trip.

  - id: T-002
    title: "Render DisplayPanel::Markdown in frontend components"
    priority: 1
    status: done
    notes: >
      In BOTH rendering locations:
        - crates/ironpad-app/src/pages/notebook_editor.rs (CellOutputDisplay)
        - crates/ironpad-app/src/components/view_only_notebook.rs (ViewOnlyOutput)
      1. Add Markdown(String) variant to the local DisplayPanel enum.
      2. Render it by calling the existing render_markdown() from markdown_cell.rs,
         then display via inner_html with the ironpad-markdown-cell-preview CSS class.
      3. Ensure the markdown output gets the same styling as markdown cells.

  # ── Feature 2: Table display type ──────────────────────────────────────
  - id: T-003
    title: "Add Table type to ironpad-cell"
    priority: 1
    status: done
    notes: >
      In crates/ironpad-cell/src/lib.rs:
      1. Add `pub struct Table { pub headers: Vec<String>, pub rows: Vec<Vec<String>> }`.
      2. Add `Table { headers: Vec<String>, rows: Vec<Vec<String>> }` variant to DisplayPanel.
      3. Implement From<Table> for CellOutput, IntoPanels for Table, TypeTag for Table.
      4. Add Table::new(headers, rows) constructor.
      5. Export Table from the prelude.
      6. Add unit tests.

  - id: T-004
    title: "Render DisplayPanel::Table in frontend components"
    priority: 1
    status: done
    notes: >
      In both rendering locations (notebook_editor.rs and view_only_notebook.rs):
      1. Add Table variant to local DisplayPanel enum.
      2. Render as a styled HTML table using the project's existing CSS variables
         (--ip-bg-surface, --ip-border, --ip-text-primary, etc.).
      3. Add SCSS styles for .ironpad-output-table in main.scss — should match the
         visual style of tables in markdown cells.

  # ── Feature 3: Keyboard shortcuts ──────────────────────────────────────
  - id: T-005
    title: "Add Ctrl+Enter to run cell in notebook editor"
    priority: 2
    status: done
    notes: >
      In notebook_editor.rs, when creating Monaco editor instances:
      1. Register a keybinding via window.IronpadMonaco.addAction() for Ctrl+Enter
         (or Cmd+Enter on Mac) that triggers the cell's run action.
      2. The Monaco editor component already supports addAction — look at how
         Escape is bound in markdown_cell.rs for reference.
      3. Also add Shift+Enter to run cell and advance focus to next cell.
      4. Ensure these don't conflict with Monaco's built-in Ctrl+Enter (which
         inserts a line below by default — override it).

  # ── Feature 4: Copy output button ──────────────────────────────────────
  - id: T-006
    title: "Add copy-to-clipboard button on cell output panels"
    priority: 2
    status: done
    notes: >
      In both notebook_editor.rs and view_only_notebook.rs output rendering:
      1. Add a small copy button (📋 or similar) in the output panel header/corner.
      2. On click, copy the panel's text content to clipboard via
         navigator.clipboard.writeText() (web_sys or wasm_bindgen).
      3. Show brief "Copied!" feedback (CSS transition, not a toast — we removed
         toasts in PRD-0002 T-005 to avoid focus stealing).
      4. For Text panels, copy the raw text. For Html/Svg/Markdown/Table, copy
         the source markup. For Table, copy as tab-separated values.
      5. Add CSS for the copy button in main.scss — small, subtle, top-right corner.

  # ── Feature 5: Export notebook as HTML ─────────────────────────────────
  - id: T-007
    title: "Add export-to-HTML button on notebook editor toolbar"
    priority: 3
    status: done
    notes: >
      In notebook_editor.rs toolbar:
      1. Add an "Export HTML" button.
      2. On click, generate a self-contained HTML document that includes:
         - All cell source code in styled <pre> blocks
         - All markdown cells rendered as HTML
         - Inline CSS (subset of main.scss — dark theme, code styling, table styling)
         - No JavaScript, no external dependencies
      3. Trigger a browser download via Blob URL + <a download="notebook.html">.
      4. This is a client-side operation — no server call needed.
      5. The exported HTML should look like a read-only version of the notebook.
      6. Cell outputs (if any have been executed) should be included if available.

---

# Summary

Five enhancements to the cell output system and editor UX: a `Md` display type
for rendering markdown from cell code, a structured `Table` display type,
keyboard shortcuts for running cells, a clipboard copy button on output panels,
and a notebook-to-HTML export feature.

---

# Problem

Cell output is currently limited to plain text, raw HTML, and SVG. Users who want
to produce formatted output must hand-write HTML strings, which is tedious and
error-prone. Common use cases — rendering data tables, formatted documentation
with computed values, quick summaries — would benefit from higher-level display
primitives. Additionally, the editor lacks basic productivity shortcuts (run cell
with keyboard) and output interaction (copy to clipboard, export).

---

# Goals

1. `Md("# heading\n\ntext")` renders styled markdown in cell output — no HTML authoring needed
2. `Table::new(headers, rows)` renders a clean data table without manual `<table>` markup
3. Ctrl+Enter runs the focused cell; Shift+Enter runs and advances
4. One-click copy for any cell output panel
5. Export a notebook as a self-contained, styled HTML file

---

# Technical Approach

## Md display type

Follows the exact `Html`/`Svg` pattern in `ironpad-cell`:
- `pub struct Md(pub String)` newtype
- `DisplayPanel::Markdown(String)` variant
- Standard trait impls: `IntoPanels`, `TypeTag`, `From<Md> for CellOutput`
- **Client-side rendering**: the Leptos components call `render_markdown()` (pulldown-cmark,
  already used for markdown cells) on the raw string, then render via `inner_html`
- No new dependencies in ironpad-cell

## Table display type

- `pub struct Table { headers: Vec<String>, rows: Vec<Vec<String>> }`
- `DisplayPanel::Table { headers, rows }` variant (structured, not HTML)
- Renderer generates `<table>` HTML with project CSS classes
- Serialized as JSON in the display panel array — headers + rows travel as structured data

## Keyboard shortcuts

- Register via `window.IronpadMonaco.addAction()` on each editor instance
- Ctrl+Enter → run cell (reuse existing `run_cell` action callback)
- Shift+Enter → run cell + move focus to next cell's editor
- Override Monaco's default Ctrl+Enter (insert line below)

## Copy button

- Small button rendered in the output panel header
- Uses `navigator.clipboard.writeText()` via wasm-bindgen
- CSS-only "Copied!" feedback (opacity transition on a sibling span)

## Export to HTML

- Client-side only: serialize notebook state into a styled HTML string
- Inline a minimal CSS subset (dark theme variables + relevant classes)
- Download via `Blob` → `URL.createObjectURL` → `<a download=...>.click()`

---

# Assumptions

- pulldown-cmark is already available in ironpad-app (used by markdown_cell.rs)
- The Monaco bridge's `addAction` API supports keybinding registration (confirmed in existing code)
- `navigator.clipboard` API is available in target browsers (all modern browsers)

---

# Constraints

- ironpad-cell targets `wasm32-unknown-unknown` — no pulldown-cmark dependency there (markdown rendering is client-side only)
- DisplayPanel is serialized as JSON between WASM and JS — new variants must be JSON-compatible
- Keyboard shortcuts must not break Monaco's core editing experience

---

# References to Code

- **Display types**: `crates/ironpad-cell/src/lib.rs` — `Html`, `Svg`, `DisplayPanel`, `IntoPanels`, `TypeTag`, `From` impls
- **Cell output rendering (editor)**: `crates/ironpad-app/src/pages/notebook_editor.rs` — `CellOutputDisplay` component
- **Cell output rendering (view-only)**: `crates/ironpad-app/src/components/view_only_notebook.rs` — `ViewOnlyOutput` component
- **Markdown rendering**: `crates/ironpad-app/src/components/markdown_cell.rs` — `render_markdown()` function
- **Monaco bridge**: `public/monaco/bridge.js` — `addAction()`, `focus()` methods
- **Prelude exports**: `crates/ironpad-cell/src/lib.rs` — `pub mod prelude`

---

# Non-Goals (MVP)

- LaTeX / math rendering in markdown output
- Interactive / editable tables in output
- Custom CSS themes for exported HTML
- Keyboard shortcut customization UI
- Rich text editor for markdown (WYSIWYG)

---

# History

(Entries appended during implementation go below this line.)

---
