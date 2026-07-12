---
id: PRD-0046
title: "Live check-on-type for shared cells"
status: done
owner: "Aaron Roney"
created: 2026-07-12
updated: 2026-07-12

depends_on:
- PRD-0044
- PRD-0045

principles:
- "Shared cells are the only editable code surface without live feedback, and their blast radius is the whole notebook: a typo in one stales and breaks every code cell. They deserve feedback MOST, not least."
- "One assembly, one mapper: the check compiles the same effective_shared_source the editor and gate use, and line mapping derives from that single assembly, never a re-implementation."
- "Same non-blocking contract as PRD-0045: skip when busy, budgeted, generation-discarded. A shared-cell check must never make typing feel heavier."

acceptance_tests:
- id: uat-001
  name: "Typing an error into a shared cell paints an inline squiggle at the correct cell-local line without running anything; fixing clears it"
  command: cargo make playwright
  uat_status: verified
- id: uat-002
  name: "Full gate green with shared-cell live checks in place"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "shared_cell_line_offset in ironpad-common: given (notebook shared_source, cells, target cell id), return the 0-based line at which that cell's source starts inside effective_shared_source"
  priority: 1
  status: done
  notes: "Must live next to effective_shared_source and be derived from the same join logic ('\\n\\n' separators, None/Some semantics) so the two can never drift. Unit tests: no notebook-level source, notebook-level source present, multiple shared cells, target first/middle/last."
- id: T-002
  title: "check_cell for shared cells: scaffold with the full assembled shared.rs and a trivial cell body, keep only diagnostics whose primary span is src/shared.rs within the target cell's line range, remap to cell-local lines"
  priority: 1
  status: done
  notes: "Server side: reuse check_cell_core with a marker or a shared_target_cell_id field on CompileRequest (serde-default so old clients are unaffected). Diagnostics landing in OTHER shared cells or the notebook-level source are dropped for the live path (they surface on those surfaces or via consuming cells as today)."
- id: T-003
  title: "Client: dispatch live checks from shared-cell editors through the same debounce/generation machinery; markers render in the shared cell's Monaco instance"
  priority: 1
  status: done
  notes: "cell_item.rs currently skips shared cells in dispatch_live_check eligibility; replace the skip with the shared-target path. Warmth policy: shared source compiles with the notebook's merged manifest, so reuse manifest_has_custom_deps on (shared_cargo_toml, default cell manifest) plus the warm_manifests set."
- id: T-004
  title: "Tests: offset unit tests, remap unit tests (error in target cell vs sibling shared cell vs notebook-level source), e2e squiggle-in-shared-cell"
  priority: 2
  status: done
  notes: "e2e follows live-check.spec.ts conventions (hydration wait, save-debounce dirty-dot wait, IRONPAD_LIVE_CHECK_TIMEOUT_SECS=300 in the Playwright webServer env)."
---

# Summary

Extend PRD-0045's live check-on-type to shared cells: as you type in an amber cell, a debounced `cargo check` of the assembled `shared.rs` paints squiggles at the right lines of that cell, without running anything.

# Problem

Shared cells (PRD-0044) never execute, so they get no feedback until some consuming cell compiles, and when that happens the error arrives labeled "(in shared source)" with no line anchored in the cell the author is actually editing. Meanwhile editing a shared cell stales every code cell in the notebook, so the cost of an unnoticed typo is a notebook-wide wall of red. The blog-notebook content pass (v0.10.0) made shared cells a headline authoring surface; they are now the only code the editor stays silent about.

# Goals

1. Inline diagnostics in shared-cell editors, on the same debounce and with the same non-blocking guarantees as PRD-0045.
2. Correct cell-local line numbers, derived from the one true assembly (`effective_shared_source`).
3. No behavior change for consuming cells: their compiles and checks keep reporting shared-source errors exactly as today.

# Technical Approach

The assembly is deterministic: `effective_shared_source` joins the notebook-level shared source and each shared cell's source with `"\n\n"`, in notebook order. That determinism makes line mapping arithmetic, not parsing: the target cell's slice starts at a computable line offset (T-001). The server check scaffolds a micro-crate whose `shared.rs` is the full assembly and whose cell body is trivial (`String::new()`), runs the existing budgeted `check_micro_crate`, then keeps only diagnostics whose primary span lands in `src/shared.rs` within the target cell's line range, subtracting the offset to produce cell-local lines (T-002). The client reuses the whole PRD-0045 dispatch pipeline: 1s save-debounce tail, try-acquire skip, generation discard, markers through the shared diagnostics-to-markers path, rendered in the shared cell's read-write Monaco (T-003).

# Assumptions

- `effective_shared_source` remains the single assembly function (constitution: single source of truth); the offset helper lives beside it.
- The compile-lock keyed on cell id serializes shared-cell checks the same way it does normal cells.

# Constraints

- Diagnostics that land in a *different* shared cell or in the notebook-level shared source are dropped on the live path rather than mis-anchored; those surfaces keep today's behavior.
- A check fired inside the save-debounce window checks the previous model state (same contract as PRD-0044/0045).
- A TimedOut round paints nothing and waits for the next edit to retry (PRD-0045 contract); under heavy build contention the first squiggle can therefore take a few edits to appear. The e2e mirrors this with nudge-retries.
- Shared-cell source feeds feature detection, so a shared cell that mentions `std::simd` or the autodiff intrinsics checks under those heavier configurations, matching how consumers will actually build.

# References to Code

- `crates/ironpad-common/src/types.rs`: `effective_shared_source`, `manifest_has_custom_deps`, `CompileRequest`
- `crates/ironpad-app/src/server_fns.rs`: `check_cell_core`, `live_check_timeout`
- `crates/ironpad-app/src/compiler/diagnostics.rs`: `src/shared.rs` span handling ("in shared source" prefix)
- `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs`: `dispatch_live_check` eligibility (currently skips shared cells)
- `tests/e2e/live-check.spec.ts`: conventions for the new spec

# Non-Goals (MVP)

- Live checks for the notebook-level shared source panel (a separate, rarely-open editor surface)
- Cross-cell anchoring (showing a sibling shared cell's error inside the cell you are editing)
- Completions/hover changes (PRD-0045's index already serves shared cells)

# History

- 2026-07-12: Created after Aaron approved the follow-up during the v0.9.1/v0.10.0 session; identified as the highest-value gap in the pre-review sweep.
- 2026-07-12: All tasks done, UATs verified (643 unit, 10 integration, 50 Playwright incl. the new spec). Debugging the e2e surfaced an unrelated environmental bug, fixed alongside: the cache pressure valve measured only percentage-full and wiped caches on every server start on big dev disks; it now also requires < 20GB absolute headroom (prod's 5GB volume unaffected). Shipped in v0.11.0.
