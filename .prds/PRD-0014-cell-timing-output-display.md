---
id: PRD-0014
title: "Cell Timing & Output Display Polish"
status: active
owner: "Aaron Roney"
created: 2026-03-14
updated: 2026-03-15

principles:
- "Show users the information they need to understand performance at a glance"
- "Output display should be consistent across editor, view-only, and public notebook pages"
- "Don't break existing cell output behavior — enhance it"

references:
- name: "Leptos reactivity docs"
  url: https://leptos.dev

acceptance_tests:
- id: uat-001
  name: "Cell badge shows both compile and runtime timing"
  command: cargo make playwright
  uat_status: unverified
- id: uat-002
  name: "Compile result text includes runtime timing"
  command: cargo make playwright
  uat_status: unverified
- id: uat-003
  name: "Public notebook page has collapsible cell outputs"
  command: cargo make playwright
  uat_status: unverified
- id: uat-004
  name: "Large outputs are scrollable with max-height on all pages"
  command: cargo make playwright
  uat_status: unverified
- id: uat-005
  name: "CI passes"
  command: cargo make ci
  uat_status: verified

tasks:
- id: T-001
  title: "Add runtime timing to cell status badge"
  priority: 1
  status: done
  notes: "In cell_item.rs, the success badge currently shows '✓ {compile_ms:.0}ms'. Change to '✓ {compile_ms:.0} + {runtime_ms:.0}ms'. The execution_time_ms is already tracked in ExecutionResult (types.rs). Add an RwSignal<Option<f64>> for runtime_ms alongside the existing compile_time_ms signal (cell_item.rs:52). Set it after execute_cell returns (cell_item.rs:584/621). Update the status text at lines 1223-1228."
- id: T-002
  title: "Add runtime timing to compile result panel text"
  priority: 1
  status: done
  notes: "In cell_output.rs:67-71, the compile summary reads '✓ Compiled (189.7 KB, 122ms, cached)'. Update format to include runtime: '✓ Compiled (189.7 KB, 122ms compile, 45ms runtime, cached)'. The CompileResultPanel component needs to accept an additional runtime_ms prop. Thread the runtime_ms signal from cell_item.rs into the panel."
- id: T-003
  title: "Add collapsible outputs to public notebook page"
  priority: 1
  status: done
  notes: "view_only_notebook.rs renders outputs without collapse/expand. Port the collapse toggle from the notebook editor (cell_item.rs:449-475, CSS class ironpad-cell-body--collapsed) into view_only_notebook.rs. Each cell output should start expanded but have a ▸/▾ toggle. Reuse existing ironpad-cell-collapse-btn and ironpad-cell-body--collapsed CSS."
- id: T-004
  title: "Add max-height and scroll to output panels on all pages"
  priority: 1
  status: done
  notes: "In main.scss, add max-height + overflow-y:auto to output containers. Currently only .ironpad-output-hex-dump has max-height (200px). Add to: .ironpad-output-display-text, .ironpad-output-body, and .view-only-output equivalents. Use a reasonable max-height (e.g., 400px or 500px). SVG and Canvas outputs should NOT be constrained (they have their own sizing). Only text, HTML, and table outputs need the scroll constraint."

---

# Summary

Improve cell output display with runtime timing information and consistent output behavior across all notebook views.

---

# Problem

1. **Missing runtime timing**: Cell badges show compile time but not execution time. Users can't tell if a cell is slow to compile or slow to execute.
2. **No output collapse on public pages**: The public notebook viewer shows all outputs expanded with no toggle, making long notebooks unwieldy.
3. **No output scroll constraint**: Large outputs (big arrays, long text) expand indefinitely, pushing subsequent cells off-screen. Only hex dump has a max-height.

---

# Goals

1. Show both compile and runtime timing in cell badges and compile summary text.
2. Add collapsible outputs to the public notebook page (matching editor behavior).
3. Add max-height with scroll to text/HTML/table outputs across all pages.

---

# Technical Approach

### Runtime Timing (T-001, T-002)

`ExecutionResult.execution_time_ms` already exists. Thread it through:

```
cell_item.rs: run_trigger → execute_cell → capture execution_time_ms → set runtime_ms signal
                                                                          ↓
cell_item.rs: status badge text → "✓ {compile} + {runtime}ms"
cell_output.rs: CompileResultPanel → "✓ Compiled (KB, Xms compile, Yms runtime, cached)"
```

### Collapsible Outputs (T-003)

Port the existing collapse pattern from the editor:
- Add `RwSignal<bool>` for `output_collapsed` per cell in view_only_notebook.rs
- Render the ▸/▾ toggle button
- Apply `ironpad-cell-body--collapsed` CSS class conditionally

### Scrollable Outputs (T-004)

Add CSS rules:
```scss
.ironpad-output-display-text,
.ironpad-output-html,
.view-only-output-text,
.view-only-output-html {
    max-height: 500px;
    overflow-y: auto;
}
```

Exclude SVG/Canvas (they have inherent sizing).

---

# Assumptions

- `ExecutionResult.execution_time_ms` is reliably populated (it's set via `js_sys::Date::now()` delta).
- The existing collapse CSS classes work for view-only outputs without modification.

---

# Constraints

- The compile result panel is rendered server-side for SSR — runtime timing must be set client-side after execution.
- Max-height must not break SVG rendering (plotters SVGs scale to container width).

---

# References to Code

| File | Role | Key Lines |
|---|---|---|
| `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs` | Cell execution + status badge | compile_time_ms (52), status text (1223-1228), execute_cell calls (548-621) |
| `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs` | Compile result panel | "✓ Compiled" format (67-71) |
| `crates/ironpad-app/src/components/view_only_notebook.rs` | Public/shared notebook renderer | Output rendering (544-617), no collapse toggle |
| `crates/ironpad-common/src/types.rs` | ExecutionResult struct | execution_time_ms field |
| `style/main.scss` | Output styling | .ironpad-output-* classes, hex dump max-height (1479) |

---

# Non-Goals (MVP)

- Per-panel collapse (collapsing individual display panels within a cell's output)
- Resizable output panels (drag to resize)
- Persisting collapse state across page reloads
- Timing charts or performance profiling UI

---

# History

(Entries appended during implementation go below this line.)

2026-03-15: All 4 tasks implemented. Runtime timing in badge and compile panel, collapsible outputs on view-only page, max-height scroll on text/HTML/table outputs. CI passes (307 tests).

---
