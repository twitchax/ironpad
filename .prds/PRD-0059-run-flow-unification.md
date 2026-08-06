---
id: PRD-0059
title: "Run-flow unification: one blob-acquisition and execute engine for editor and viewer"
status: done
owner: "Aaron Roney"
created: 2026-08-06
updated: 2026-08-06

principles:
- "The editor and the viewer already share the piping recipes (assemble_cell_inputs, unexecuted_upstream, probe policy); this closes the remaining fork — the async compile/load/execute engine itself — before PRD-0060 changes its semantics."
- "The fork has already produced real divergence: the viewer advances its run queue past failures and retries compile transport errors; the editor aborts the cascade and never retries. Unification adopts the stricter robustness (retries) everywhere and PARAMETERIZES the failure policy so this refactor stays behavior-shaped; PRD-0060 then unifies the policy itself."
- "Surfaces keep their own signals, status enums, session events, and bookkeeping. The shared engine owns exactly the async pipeline: snapshot -> local probe -> compile-with-retry -> store-back -> load -> execute."

references:
- name: "Editor pipeline (consumer)"
  url: crates/ironpad-app/src/pages/notebook_editor/pipeline.rs
- name: "Viewer run flow (consumer)"
  url: crates/ironpad-app/src/components/view_only_notebook.rs
- name: "Shared piping seams (PRD-0055/W2)"
  url: crates/ironpad-app/src/components/executor.rs

acceptance_tests:
- id: uat-001
  name: "Behavior-preserving refactor: full suite green (ci + integration + playwright), editor and viewer both compile through the shared engine"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "components/run_flow.rs: acquire_blob + load_and_execute + advance_queue"
  priority: 1
  status: done
  notes: "acquire_blob(request, share_blob, force): share snapshot (viewer-only input, None for editor) -> blob_cache::probe_local -> compile_cell with 2 transport retries + backoff (moved transport_backoff here); returns BlobAcquisition { result, served_without_server, request_hash }. load_and_execute(cell_id, response, inputs): store_unless_served is the CALLER's business (editor stores before load; keep) — actually fold store-back into acquire? No: store_unless_served stays caller-visible so the editor's ordering (store before load) is preserved exactly. advance_queue(queue, cell_id): the pop-front-if-me all four sites hand-roll."
- id: T-002
  title: "Rewrite editor pipeline.rs and viewer over the engine"
  priority: 1
  status: done
  notes: "Editor gains compile-transport retries (disclosed; idempotent requests, viewer already had them). Failure policy untouched: editor abort_run_cascade, viewer advance-past. Session events unchanged (emitted around the engine's stages)."
- id: T-003
  title: "Gate + docs"
  priority: 2
  status: done
  notes: "cargo make uat green. CLAUDE.md hot-edit files note run_flow.rs."
---

# Summary

Extract the duplicated async cell-run engine (blob acquisition through execution) from the editor's `pipeline.rs` and the viewer's `ViewOnlyCodeCell` into one shared module, so PRD-0060's execution-semantics change (dependency-aware cascade, continue-past-failures) lands once instead of twice.

# Problem

The compile/load/execute pipeline exists twice, and the copies have measurably diverged: the viewer retries compile transport errors (idempotent requests) and advances its queue past failures; the editor does neither. Every past bug of this class (Unpublish's missed flush, the three copies of the prerequisite cascade) came from workflow logic duplicated across surfaces.

# Goals

1. One `acquire_blob` policy: share snapshot -> local blob probe -> server compile with transport retries.
2. One `load_and_execute` wrapper and one queue-advance helper.
3. Editor and viewer consume the engine; their signals, status models, and failure policies stay put (policy unifies in PRD-0060).

# Technical Approach

New `crates/ironpad-app/src/components/run_flow.rs` (hydrate-gated), consumed by `pipeline.rs::wire_run_effect` and `view_only_notebook.rs::ViewOnlyCodeCell`. `transport_backoff` moves in. `store_unless_served` remains a caller call so the editor's store-before-load ordering is byte-identical.

# Non-Goals (MVP)

- Changing cascade or failure semantics (PRD-0060).
- Unifying the queue-watcher effects or status enums (different UI models, low duplication).

# History

- **2026-08-06** — Created; scoped as the seam-preparation refactor ahead of PRD-0060.
- **2026-08-06** — Implemented and closed. `components/run_flow.rs` owns acquire_blob (snapshot -> local probe -> compile with 2 transport retries), load_and_execute, and advance_queue; editor pipeline, viewer, and the cell_item markdown/shared skip all consume it. The editor gained the viewer's compile-transport retries (disclosed behavior improvement; requests are idempotent). Failure policies untouched (editor aborts, viewer advances) pending PRD-0060. Gate: cargo make ci (810), Playwright 112 passed / 0 failed.
