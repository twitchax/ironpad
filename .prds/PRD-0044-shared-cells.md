---
id: PRD-0044
title: "Shared cells: narrate the shared.rs library inline"
status: done
owner: "Aaron Roney"
created: 2026-07-11
updated: 2026-07-11

principles:
- "The compiler pipeline does not change: the client assembles notebook-level shared source + shared cells (in order) into the CompileRequest's shared_source; caching, detection, and scaffolding all work unchanged."
- "One assembly definition (ironpad-common::effective_shared_source) used by the editor, the view-only runner, and the public-notebook check gate — or validation drifts from production."
- "Shared cells never execute: no run button, no output, skipped by run-all and reactive scheduling, empty piping slot (like markdown). Their errors surface through consuming cells."
- "serde-default boolean flag, not a new cell type: every pre-existing .ironpad file parses unchanged in both directions."

acceptance_tests:
- id: uat-001
  name: "Editor: mark a cell shared, a later cell calls shared::fn and runs; shared cell shows amber chrome and no run button"
  command: cargo make playwright
  uat_status: verified
- id: uat-002
  name: "Notebook gate compiles public notebooks whose shared code lives in shared cells"
  command: cargo make test-integration
  uat_status: verified
- id: uat-003
  name: "Full gate green with the feature and the content pass in place"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "Data model + assembly: IronpadCell.shared flag, effective_shared_source in ironpad-common, protocol/CLI threading"
  priority: 1
  status: done
  notes: "CellUpdate/NewCell/Event::CellUpdated/CellManifest gain the flag; cells.add/cells.update accept a shared arg in the CLI translator."
- id: T-002
  title: "Editor: toggle in cell menu, amber chrome + badge, no run/gear, excluded from run-all/cascade/reactive queues, effective shared source in compile requests"
  priority: 1
  status: done
  notes: "Shared edits (source or flag) mark ALL code cells stale — shared source is global, downstream-only staling under-invalidates."
- id: T-003
  title: "View-only/embed: ViewOnlySharedCell renderer, queue exclusion, effective shared source"
  priority: 1
  status: done
  notes: "Read-only Monaco with shared chrome; expand_code controls default collapse like code cells."
- id: T-004
  title: "Notebook gate parity: effective shared source + shared cells skipped as compile targets"
  priority: 2
  status: done
  notes: "Empty piping slot preserved so cellN indices stay positional."
- id: T-005
  title: "Playwright e2e: toggle -> downstream cell consumes shared::fn -> output; view-only shared chrome"
  priority: 2
  status: done
  notes: "The spec waits for the 1s save debounce (the tab dirty dot) before running — shared.rs is assembled from the model, and typing-then-running inside the window compiles the previous shared source."
- id: T-006
  title: "Content pass: cannon + autodiff companion move load-bearing shared code into shared cells with exposition; shared-code.ironpad becomes the showcase"
  priority: 3
  status: done
  notes: "simd-lanes deliberately stays in-cell: shared source feeds feature detection, and a shared cell mentioning std::simd would opt the scalar baseline into simd128."
---

# Summary

A cell can be marked `shared`: it renders inline among the other cells with amber chrome, never executes, and its source is appended to the notebook's `shared.rs` (after the notebook-level shared source). Notebooks can then walk readers through the load-bearing parts of their shared code instead of hiding them in one collapsed appendix.

# Problem

For notebooks like the cannon (PRD-0041), the most important code in the whole piece — the simulator carrying the `#[autodiff_reverse]` attribute — lives in the notebook-level shared source, visible only as a collapsed blob after the cells. The narrative cannot point at it, and readers reasonably conclude the notebook is hiding the thing it claims to show.

# Goals

1. Shared code participates in the narrative: interleaved with markdown, visibly distinct, in reading order.
2. Zero compiler changes; caching and feature detection (autodiff/rayon/simd) work unchanged because they already read the merged shared source.
3. Full round-trip: editor toggle, view-only/embed rendering, agent protocol, import/export, gate validation.

# Technical Approach

`IronpadCell` gains `#[serde(default)] shared: bool`. `ironpad_common::effective_shared_source(notebook_shared, cells)` concatenates the notebook-level shared source with every shared cell's source in `order`, blank-line separated, preserving the `None`/`Some` distinction the cache keys on. The editor's compile path, the view-only runner, and the check gate all call it. Shared cells are excluded from every run queue (run-all, run-all-below, cascade, reactive, autorun) and hold an empty piping slot so `cellN` indices stay positional. Model staling widens to all code cells when a shared cell's source or flag changes, since shared source is a global compile input.

# Assumptions

- Rust item order is compilation-irrelevant, so cell order only affects generated `shared.rs` readability.

# Constraints

- Feature detection reads shared source: a shared cell mentioning `std::simd`/`std::autodiff`/rayon opts every cell in the notebook in. Deliberate for cannon (autodiff is notebook-wide); disqualifying for simd-lanes (the scalar baseline must stay unflagged).
- Source edits propagate to the model on the editor's existing 1s save debounce; a run issued inside that window compiles against the previous shared source. The late save marks all code cells stale (self-healing, and reactive mode re-runs them), matching the persistence contract that already governs refresh-during-typing.
- A shared cell's own `cargo_toml` is inert (deps for shared code belong in the notebook-level shared Cargo.toml); the editor hides the per-cell cargo affordances for shared cells.

# References to Code

- `crates/ironpad-common/src/types.rs` — flag + `effective_shared_source`
- `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs` — toggle, chrome, request assembly
- `crates/ironpad-app/src/components/view_only_notebook.rs` — `ViewOnlySharedCell`
- `crates/ironpad-app/src/compiler/mod.rs` — gate parity
- `style/main.scss` — `--ip-shared` / `--ip-shared-tint` amber chrome

# Non-Goals (MVP)

- Executing/check-compiling a shared cell on demand
- Per-shared-cell dependency manifests
- Diagnostics mapped back into shared-cell editors (errors surface via consuming cells, as with notebook-level shared source today)

# History

- 2026-07-11: Created; T-001..T-004 implemented (data model, editor, view-only, gate).
- 2026-07-11: All tasks done, UATs verified (ci 632, notebook gate green over the restructured cannon/autodiff/shared-code, Playwright 48 incl. the shared-cells spec). Shipped as v0.8.0.
