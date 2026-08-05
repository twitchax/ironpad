---
id: PRD-0055
title: "SOC refactors: CellItem pipeline extraction, executor-core.js split, main.rs split"
status: active
owner: "Aaron Roney"
created: 2026-08-05
updated: 2026-08-05

principles:
- "Behavior-preserving, byte-for-byte where observable: no UI change, no wire change, no cache-key change, no new features ride along."
- "One refactor per commit, each gated independently (ci + Playwright; test-integration where compiler-adjacent). Never two splits in flight at once."
- "The seam is the deliverable: extraction should mint testable units (pure functions, narrow structs) where the inline code had none — that is how the Unpublish bug class dies."
- "Workflow logic buried in view markup does not get the same review the extracted functions do (fanout review, PRD-0054 history). These three files are where that risk still lives."

references:
- name: "Fanout review findings (2026-08-04): mod.rs SOC medium (fixed, sharing.rs), CellItem/executor-core/main.rs SOC lows (this PRD)"
  url: crates/ironpad-app/src/pages/notebook_editor/cell_item.rs
- name: "Precedent: sharing.rs extraction (wave 1)"
  url: crates/ironpad-app/src/pages/notebook_editor/sharing.rs
- name: "Precedent: notebook_ops shared model/daemon appliers (wave 2)"
  url: crates/ironpad-common/src/notebook_ops.rs

acceptance_tests:
- id: uat-001
  name: "CellItem pipeline extraction: full suite green, no behavioral diff (compile/execute/queue/stale/session-event flows identical)"
  command: cargo make uat
  uat_status: verified
- id: uat-002
  name: "executor-core.js split: all cell execution paths (bindgen, raw, rayon, JSPI, GPU, sim) work across window and worker contexts"
  command: cargo make uat
  uat_status: verified
- id: uat-003
  name: "main.rs split: server boots identically (routes, middleware, sweepers, cache valve); relay + auth integration tests green"
  command: cargo make uat
  uat_status: unverified

tasks:
- id: T-001
  title: "Extract the compile/execute pipeline from CellItem"
  priority: 1
  status: done
  notes: "cell_item.rs is 2,073 lines with the full pipeline (CompileRequest build, blob-cache probe, compile_cell, session-event emission, WASM load+execute, output/stale/blocked/queue bookkeeping) inline in the component's run effect. Extract to pages/notebook_editor/pipeline.rs: a CellRunCtx struct bundling the per-cell signals + free async fns for the stages. CellItem keeps rendering and wiring only. Where a stage is pure (request assembly, outcome classification), make it a pure fn with unit tests. Do NOT unify with the viewer's flow in this pass — shrink the diff surface first; shared helpers (assemble_cell_inputs, probe_local, abort_run_cascade) already exist and the extraction should consume them, not re-wrap them."
- id: T-002
  title: "Split executor-core.js by concern"
  priority: 2
  status: done
  notes: "1,480 lines mixing the WebGPU runtime, BMP/image encoding, wasm-bindgen glue rewriting (env import table, rayon inline-worker codegen), and the cell ABI/executor class. Split into files loaded in both contexts (window script tags via versioned(); worker via importScripts in executor-worker*.js) — verify the load mechanism for each context FIRST and keep load order explicit. Candidate cut: executor-gpu.js (device/buffers/dispatch/readback), executor-glue.js (env table + glue rewrite + rayon codegen), executor-core.js (executor class + ABI + sim bus). The env-import sync test in compiler/build.rs must keep passing (update its file target if the table moves). If the split yields a natural shared home for the bridge's sim-bus mirror, take it; otherwise leave the documented duplication."
- id: T-003
  title: "Split main.rs bootstrap into modules"
  priority: 3
  status: todo
  notes: "845 lines wiring tracing setup, config conversion, DB open, cache valve, static-asset middleware, the share-blobs/OG/oembed/crawl routes, WS routes, auth nesting, sweeper task, and serve. Extract cohesive units into bin-local modules (mod middleware, mod router_assembly or similar) keeping main() as a readable table of contents. is_embeddable_path and friends carry their tests along. No route or header behavior changes; the versioned()/cache-header invariants (CLAUDE.md pitfall 6) must survive verbatim."
- id: T-004
  title: "Docs + PRD close"
  priority: 4
  status: todo
  notes: "CLAUDE.md hot-edit paths and DEVELOPMENT.md architecture sections updated to the new module map; PRD history entries per task; no version bump/deploy in this PRD (ships with whatever release follows)."
---

# Summary

The three structural splits the fanout review confirmed but wave 3 deliberately skipped: the compile/execute pipeline embedded in `CellItem`, the four-concern `executor-core.js`, and the everything-bootstrap `main.rs`. Pure refactors — the features stacked behind them (persisted view-mode outputs, `/embed/mutable`, `/local` history) land on the new seams afterwards.

# Problem

The review's concrete evidence: workflow logic written inline where structure said it should not be is where the shipped bugs were (the inline Unpublish flow was the one serialize path missing the flush discipline). `cell_item.rs` embeds the entire compile/execute pipeline in a UI component (2,073 lines), `executor-core.js` mixes WebGPU, image encoding, glue codegen, and the cell ABI in one untyped file (1,480 lines), and `main.rs` wires ten subsystems in one function (845 lines). Each is a place where the next change gets less review than it needs.

# Goals

1. `CellItem` renders; a `pipeline` module runs cells. Pure stages become unit-testable.
2. `executor-core.js` becomes three files with one concern each, loading identically in window and worker contexts.
3. `main.rs` reads as a table of contents.
4. Zero behavior change, proven by the existing suites (947 tests across unit/integration/Playwright).

# Technical Approach

One task per commit, in priority order, full gate between. Extraction consumes the seams the review waves already minted (`assemble_cell_inputs`, `probe_local`/`store_unless_served`, `abort_run_cascade`, the env import table) rather than re-wrapping them. The JS split is the risky one — context-dependent loading — so it gets an explicit load-order audit before any code moves.

# Assumptions

- The existing suites are the behavioral spec; a refactor that needs new assertions to stay honest adds them at the new seams.
- No concurrent feature work in the touched files while a split is in flight.

# Constraints

- No wire, cache-key, route, or header changes. `PROTOCOL_VERSION` and `CACHE_EPOCH` untouched.
- Playwright is the arbiter for both Rust UI and JS changes; `test-integration` re-runs when anything compiler-adjacent moves.

# References to Code

See frontmatter references; hot files are the three named in the tasks.

# Non-Goals (MVP)

- Unifying the editor and viewer execution flows (a follow-up once the pipeline is a module).
- The bridge sim-bus mirror, unless T-002 creates its shared home for free.
- Any behavior change, however tempting mid-refactor.

# History

- **2026-08-05** — PRD created from the fanout review's three deliberately-deferred SOC findings, prioritized ahead of the feature backlog at Aaron's direction.

- **2026-08-05** — T-001 done: `pipeline.rs` (compile/execute flow as `wire_run_effect` over a `CellRunCtx` bundle, plus the PRD-0045 live-check dispatch and marker/warmth helpers). CellItem drops from 2,073 to ~1,500 lines and owns only the trigger, watchers, editor callbacks, and rendering. The extraction consumed two more shared seams for free: the run path's hand-rolled prerequisite cascade became `unexecuted_upstream` (the third copy unified), and the downstream-invalidation set became a pure, unit-tested `downstream_code_ids`. Session-event emission moved behind `CellRunCtx::emit_session_event` with the same live-session gate. Gate: cargo make ci (806 unit tests, two new), full Playwright 108 passed / 0 failed. uat-001 verified.
- **2026-08-05** — T-002 done: `executor-gpu.js` (WebGPU state/dispatch/readback/BMP, 230 lines) and `executor-glue.js` (env import table + ESM/rayon/preamble rewriting as pure text functions, 250 lines) split out; `executor-core.js` drops 1,480 → 1,070 and keeps the executor class + ABI + sim bus, consuming the two namespaces. Loaders updated with explicit ordering (worker importScripts chain; the bridge's fallback injection generalized to a sequential list). The env-import sync test retargeted to the glue file. Dead `_gpuBmpToBase64DataUrl` (zero callers) deleted. Verified by: node-level smoke of the moved behavior (namespace surface, a synthetic bindgen-glue rewrite through all three transformations, raw-path closure materialization), cargo make ci (806), full Playwright 108/0 — every e2e cell execution exercises the worker chain; the main-thread fallback injection sequence is the one path e2e cannot reach and carries the parse-check instead. uat-002 verified.
