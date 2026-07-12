---
id: PRD-0045
title: "Tier-3 intellisense: live check-on-type + prelude completions"
status: active
owner: "Aaron Roney"
created: 2026-07-12
updated: 2026-07-12

principles:
- "Typing never blocks: checks are skipped (not queued) when the cell's compile lock is busy, superseded results are discarded by generation, and a 10s server-side timeout turns a misclassified cold check into 'no markers this round', never a hang."
- "Warmth is classified statically, not guessed: default-deps cells are warm by construction (the image warmup seeds targets/default like the atomics sysroot); custom-deps cells go live only after their manifest has compiled once."
- "check_cell is the same pipeline as compile_cell minus codegen: shared scaffold, shared configure_cargo_cmd, shared diagnostics parsing and preamble mapping — a cell that checks clean builds clean."
- "Completions are a curated static index of the exact API surface cells see (ironpad-cell prelude via rustdoc JSON), not a type-inference engine. Deterministic, instant, offline."

acceptance_tests:
- id: uat-001
  name: "Typing invalid code paints inline markers without running the cell; fixing it clears them"
  command: cargo make playwright
  uat_status: unverified
- id: uat-002
  name: "Full gate green with live checks and completions in place"
  command: cargo make uat
  uat_status: unverified

tasks:
- id: T-001
  title: "check_cell server fn: un-gate check_micro_crate, kill_on_drop + short timeout, try-acquire lock (skip when busy), diagnostics mapped like compile"
  priority: 1
  status: done
  notes: "CheckResponse { status: Clean|Errors|Skipped|TimedOut, diagnostics } in ironpad-common; timeout parameterized (gate keeps 300s, live checks 10s)."
- id: T-002
  title: "Warmth policy: manifest_has_custom_deps in common + client-side compiled-ok manifest set"
  priority: 1
  status: done
  notes: "Eligible = no deps beyond ironpad-cell, OR (cell cargo_toml, shared cargo_toml) pair seen in a successful compile this page session."
- id: T-003
  title: "Client: check dispatched from the existing 1s save debounce, generation-discard, markers via the shared diagnostics->markers path"
  priority: 1
  status: done
  notes: "Skips markdown/shared cells and cells that are Compiling/Running; compile results and check results share the marker surface (latest writer wins)."
- id: T-004
  title: "Image warmup seeds targets/default (build + check) so default-deps cells are warm from server boot"
  priority: 2
  status: done
  notes: "Same /warmup-cache + entrypoint-seed pattern as the atomics sysroot; warmup runs cargo build AND cargo check (check artifacts are distinct)."
- id: T-005
  title: "Completions/hover: generated index of the ironpad-cell prelude + Monaco providers"
  priority: 2
  status: done
  notes: "Source-scan generator (tools/gen-completions.py) instead of rustdoc JSON: ironpad-cell is ours and consistently styled, signatures come out as written, and there is no coupling to rustdoc format churn. cargo make gen-completions regenerates the committed index; completion + hover providers in the Monaco bridge."
- id: T-006
  title: "Tests: lock try-acquire unit, manifest classification unit, check e2e (squiggle without run)"
  priority: 2
  status: done
  notes: ""
---

# Summary

Live diagnostics while typing (a debounced `cargo check` through the existing compile pipeline, minus codegen) plus curated completions and hover docs for the cell API surface. The IDE feel without rust-analyzer's memory bill.

# Problem

Cells only report errors after a full Run (compile + link + wasm-bindgen). The feedback loop for simple mistakes is tens of seconds, and the editor offers no guidance about the ironpad-cell API surface (`CellOutput`, `Table`, `blocking::*`, ...). Real rust-analyzer would fix both but wants 1.5-2.5GB resident per workspace on a 2GB prod box shared with cargo builds.

# Technical Approach

`check_micro_crate` (already used by the notebook gate) is promoted from test-only to a `check_cell` server fn. It shares scaffolding, toolchain selection, RUSTFLAGS, and diagnostics mapping with `compile_cell`, differing only in the cargo subcommand — so check results always agree with build results. Non-blocking is enforced three ways: the per-cell lock is try-acquired (busy → `Skipped`), the cargo process is `kill_on_drop` with a 10s timeout (→ `TimedOut`), and the client discards superseded responses by generation. The client dispatches a check from the tail of the existing 1s save debounce (the model is current at that point by construction) and paints markers through the same diagnostics→markers path compiles use.

Warmth: a static classifier (`manifest_has_custom_deps`) plus a per-page-session set of manifests that have compiled successfully. Default-deps cells are guaranteed warm because the image warmup now seeds `targets/default` with built AND checked artifacts for the ironpad-cell tree, seeded into the volume by the entrypoint exactly like the atomics sysroot.

Completions: `cargo rustdoc --output-format json` over ironpad-cell at authoring time produces a committed index (name, kind, signature, first doc paragraph) that Monaco completion/hover providers serve instantly.

# Constraints

- Shared cells are not live-checked in MVP (their diagnostics would need mapping through the shared.rs assembly); their errors surface via consuming cells as today.
- Completions are not type-aware; they cover the prelude surface plus std-common names, filtered by Monaco's word matching.
- A check fired inside the 1s save debounce window checks the previous model state — same contract as persistence (PRD-0044 note).

# Non-Goals (MVP)

- rust-analyzer (tracked as a possible future PRD; requires a bigger machine or an LSP sidecar)
- Go-to-definition, signature help, type-aware member completion
- Live checks in view-only/embed surfaces

# History

- 2026-07-12: Created after tier discussion (RA server-side deferred on memory grounds; RA-in-WASM rejected: single-file analysis can't see shared::, cellN, or crates.io deps).
