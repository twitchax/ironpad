---
id: PRD-0004
title: "Cell Builtins, Test Coverage & Editor Refactor"
status: active
owner: Aaron Roney
created: 2026-03-10
updated: 2026-03-10

depends_on:
- PRD-0003

principles:
- "Plot/Json helpers live in ironpad-cell — they must compile to wasm32-unknown-unknown"
- "Plot wraps plotters SVGBackend; Json wraps serde_json — both already used in seed notebooks"
- "Refactoring notebook_editor.rs must not change any visible UI behavior"
- "New Playwright tests should cover real compilation + execution, not just DOM presence"
- "Build speed optimizations go in shared_cargo_toml so they apply to all cells in a notebook"

references:
- name: "MegaPrd (architecture, design, all details)"
  url: "MegaPrd.md"
- name: "PRD-0003 (cell output enhancements)"
  url: ".prds/PRD-0003-cell-output-enhancements.md"
- name: "plotters crate docs"
  url: "https://docs.rs/plotters/latest/plotters/"
- name: "Playwright docs"
  url: "https://playwright.dev/docs/intro"

acceptance_tests:
- id: uat-001
  name: "Plot::line produces SVG output in cell execution"
  command: cargo make test
  uat_status: verified
- id: uat-002
  name: "Json(value) produces syntax-highlighted pretty-printed JSON display"
  command: cargo make test
  uat_status: verified
- id: uat-003
  name: "Seed notebooks include profile.release build speed optimizations in shared_cargo_toml"
  command: "grep -l 'opt-level' public/notebooks/*.ironpad | wc -l"
  uat_status: verified
- id: uat-004
  name: "notebook_editor.rs is under 800 lines after refactor"
  command: "wc -l crates/ironpad-app/src/pages/notebook_editor/mod.rs"
  uat_status: verified
- id: uat-005
  name: "New ironpad-app unit tests cover compiler scaffold, cache, and diagnostics"
  command: cargo make test
  uat_status: verified
- id: uat-006
  name: "New Playwright tests cover cell execution and output validation"
  command: cargo make playwright
  uat_status: unverified
- id: uat-007
  name: "cargo make ci passes with all changes"
  command: cargo make ci
  uat_status: verified

tasks:
  # ── Feature 1: Plot helper ─────────────────────────────────────────────
  - id: T-001
    title: "Add Plot helper to ironpad-cell"
    priority: 1
    status: done
    notes: >
      Add an ergonomic Plot API to ironpad-cell that wraps plotters SVGBackend.
      Target API:
        Plot::line(&[(f64,f64)]).title("My Chart").x_label("X").y_label("Y")
        Plot::bar(&[("label", value)]).title("Bar Chart")
        Plot::scatter(&[(f64,f64)]).title("Scatter")
      Each produces an Svg(String) when converted to CellOutput.
      Implementation: add a `plot` module in ironpad-cell with a builder pattern.
      plotters must be a dependency of ironpad-cell (wasm32-compatible — plotters
      already supports this via SVGBackend). Add plotters with default-features = false
      and the "svg_backend" feature.
      Include unit tests (non-wasm, just verify SVG string output).
      Export Plot from the prelude.

  - id: T-002
    title: "Add Json display type to ironpad-cell"
    priority: 1
    status: done
    notes: >
      Add a `Json` newtype that pretty-prints any serde_json::Value with syntax
      highlighting via HTML spans.
      pub struct Json(pub serde_json::Value);
      Converts to DisplayPanel::Html with a styled <pre> block containing
      color-coded JSON (keys in accent color, strings in green, numbers in blue,
      booleans/null in purple). Use inline styles referencing the ironpad CSS
      variable names as hex colors (since cell output HTML doesn't have access
      to the CSS custom properties directly — use the actual color values from
      the palette: #e94560 for keys, #40c080 for strings, #40a0f0 for numbers,
      #b080f0 for booleans/null).
      Also support Json::from_str(s) for raw JSON string input.
      Add to prelude. Add unit tests.

  # ── Feature 2: Build speed optimizations in seed notebooks ─────────────
  - id: T-003
    title: "Add WASM build speed optimizations to seed notebook shared_cargo_toml"
    priority: 1
    status: done
    notes: >
      Update all 3 seed notebooks in public/notebooks/ to include build speed
      optimizations in their shared_cargo_toml field:
        [profile.release]
        opt-level = 1
        lto = false
        codegen-units = 16
      The scaffold already supports forwarding [profile.release] sections from
      shared_cargo_toml (see compiler/scaffold.rs). For notebooks that currently
      have null shared_cargo_toml (tutorial.ironpad), set it to just the profile
      section. For notebooks with existing shared deps (welcome, async-http),
      append the profile section.
      Verify the JSON remains valid after editing.

  # ── Feature 3: Editor refactor ─────────────────────────────────────────
  - id: T-004
    title: "Extract CellItem and sub-components from notebook_editor.rs"
    priority: 2
    status: done
    notes: >
      notebook_editor.rs is currently 2,614 lines with CellItem alone at 1,361
      lines. Extract into separate modules under a new directory:
        crates/ironpad-app/src/pages/notebook_editor/
          mod.rs          — re-exports, NotebookEditorPage, NotebookContent
          cell_item.rs    — CellItem component (the big one)
          cell_output.rs  — CellOutputPanel, CompileResultPanel
          shared_deps.rs  — SharedDepsPanel
          skeleton.rs     — NotebookEditorSkeleton, CellSkeleton
          state.rs        — NotebookState struct and helpers
      The existing notebook_editor.rs becomes notebook_editor/mod.rs.
      UI must remain identical — this is purely structural.
      Run cargo make ci after to verify no regressions.

  # ── Feature 4: Test coverage ───────────────────────────────────────────
  - id: T-005
    title: "Add unit tests for ironpad-app compiler modules"
    priority: 2
    status: done
    notes: >
      The compiler modules in crates/ironpad-app/src/compiler/ already have some
      tests but coverage can be improved. Add tests for:
        - scaffold.rs: edge cases in dependency merging, extra section extraction
        - cache.rs: concurrent access, expiry behavior
        - diagnostics.rs: more rustc error format variations
      Add #[cfg(test)] mod tests in each file. Target: at least 5 new test
      functions across the compiler modules.

  - id: T-006
    title: "Add Playwright e2e tests for cell execution and output"
    priority: 2
    status: done
    notes: >
      Currently only 4 basic Playwright smoke tests exist. Add tests for:
        - tests/e2e/execution.spec.ts: Create notebook, add cell with trivial
          Rust code (e.g., `42`), click Run, verify output appears with "42"
        - tests/e2e/public-notebooks.spec.ts: Navigate to a public notebook,
          verify cells render, verify fork button works and navigates to new
          private notebook
        - tests/e2e/keyboard.spec.ts: Open notebook, focus cell editor, press
          Ctrl+Enter, verify compilation starts
      Each test file should have 1-3 focused tests. Use existing test patterns
      from tests/e2e/ for setup/teardown.

---

# Summary

Four workstreams: ergonomic cell builtins (`Plot` and `Json` display types),
WASM build speed optimizations in seed notebooks, a structural refactor of the
2,600-line notebook editor into focused modules, and expanded test coverage
(compiler unit tests + Playwright e2e tests for execution and output).

---

# Problem

1. **Cell builtins are too low-level**: Users must hand-wire plotters SVGBackend
   boilerplate for charts and manually format JSON for display. An ergonomic
   `Plot::line(data)` and `Json(value)` API would dramatically lower the barrier.

2. **Seed notebooks compile slowly**: The 3 built-in notebooks lack
   `[profile.release]` build speed optimizations (opt-level=1, lto=false,
   codegen-units=16). First-time users experience unnecessarily slow compilation.

3. **notebook_editor.rs is 2,614 lines**: The CellItem component alone is 1,361
   lines. This makes the file hard to navigate, review, and modify safely.

4. **Test coverage gaps**: ironpad-app has zero unit tests. Only 4 Playwright
   smoke tests exist, none of which test actual cell execution or output rendering.

---

# Goals

1. `Plot::line(&data).title("Chart")` produces an SVG chart in one line of cell code
2. `Json(value)` pretty-prints syntax-highlighted JSON in cell output
3. Seed notebooks compile faster out of the box with profile optimizations
4. notebook_editor.rs is split into focused modules (each under ~500 lines)
5. New compiler unit tests and Playwright e2e tests for execution and output

---

# Technical Approach

## Plot helper

Add a `plot` module to ironpad-cell with a builder-pattern API:
- `Plot::line(data)` / `Plot::bar(data)` / `Plot::scatter(data)` constructors
- `.title()`, `.x_label()`, `.y_label()`, `.size(w, h)` chainable methods
- `From<Plot> for CellOutput` renders to SVG via plotters `SVGBackend::with_string`
- plotters added as a dependency with `default-features = false, features = ["svg_backend"]`
- Dark-theme defaults: transparent background, light-colored axes/text (#eaeaea)

## Json display type

Simple newtype wrapping `serde_json::Value`:
- Pretty-prints with 2-space indentation
- Wraps in `<pre>` with inline `<span style="color:...">` for syntax highlighting
- Colors from ironpad palette (keys=#e94560, strings=#40c080, numbers=#40a0f0, bools=#b080f0)
- `From<Json> for CellOutput` produces `DisplayPanel::Html`

## Build speed optimizations

Add `[profile.release]` block to each seed notebook's `shared_cargo_toml`:
```toml
[profile.release]
opt-level = 1
lto = false
codegen-units = 16
```
The scaffold already supports this — it extracts extra sections from shared_cargo_toml.

## Editor refactor

Convert `notebook_editor.rs` (single file) into a `notebook_editor/` module directory.
Extract components by responsibility. Preserve all imports and public API.

## Test coverage

- Compiler unit tests: add `#[cfg(test)]` modules to scaffold.rs, cache.rs, diagnostics.rs
- Playwright tests: new spec files for cell execution, public notebooks, keyboard shortcuts

---

# Assumptions

- plotters compiles to wasm32-unknown-unknown with svg_backend feature (confirmed: welcome notebook already uses it)
- Playwright test infrastructure is already configured (confirmed: 4 existing tests)
- The scaffold's extra-section extraction handles [profile.release] correctly (confirmed: existing unit tests)

---

# Constraints

- ironpad-cell must remain wasm32-unknown-unknown compatible
- plotters dependency must use default-features = false to avoid non-wasm backends
- Editor refactor must not change any visible behavior or CSS class names
- New Playwright tests must work with `cargo make playwright`

---

# References to Code

- **ironpad-cell API**: `crates/ironpad-cell/src/lib.rs` (1,721 lines, 44 tests)
- **HTTP module**: `crates/ironpad-cell/src/http.rs` (pattern for new modules)
- **Scaffold extra sections**: `crates/ironpad-app/src/compiler/scaffold.rs:60-100, 189-230`
- **notebook_editor.rs**: `crates/ironpad-app/src/pages/notebook_editor.rs` (2,614 lines, 9 components)
- **Seed notebooks**: `public/notebooks/{welcome,async-http,tutorial}.ironpad`
- **Existing Playwright tests**: `tests/e2e/{sanity,home,notebook,seed}.spec.ts`
- **Playwright config**: `playwright.config.ts`

---

# Non-Goals (MVP)

- Interactive chart editing or zoom/pan in Plot output
- Custom color themes for Json syntax highlighting
- Cell drag-and-drop reordering (cut from this PRD)
- Auto-import suggestions (cut from this PRD)
- Server-side test execution for Playwright (tests use client-side execution only)

---

# History

(Entries appended during implementation go below this line.)

- **2026-03-10**: All 6 tasks implemented via parallel fleet agents. CI passes with 190 tests (up from 157). UATs 1-5, 7 verified; uat-006 (Playwright) requires live server.

---
