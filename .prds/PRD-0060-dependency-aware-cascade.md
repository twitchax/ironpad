---
id: PRD-0060
title: "Dependency-aware run cascade and continue-past-failures Run All"
status: done
owner: "Aaron Roney"
created: 2026-08-06
updated: 2026-08-06

depends_on:
- PRD-0059

principles:
- "A cell's dependencies are knowable from its source: piped inputs are consumed ONLY through the injected cellN bindings and the last alias, so referenced-slot detection is exact up to string/comment false positives — which only over-cascade (today's behavior), never break."
- "The scaffold, the cache key, and the cascade must share ONE detection recipe (ironpad-common), or keys and codegen fork. Binding only referenced slots is what makes skipping unconsumed upstream cells SAFE: today every typed slot deserializes eagerly and panics on empty bytes."
- "Failure policy (user decision 2026-08-06): Run All and autorun continue past a failed cell for cells that do not transitively depend on it; true dependents are marked prerequisite-failed and skipped. Fixes the borrows/dynosaur wall where one deliberate compile-fail teaching cell strands every later cell."
- "The last alias is dynamic (last TYPED upstream at compile time), so a cell referencing last conservatively depends on ALL upstream runnable cells and keeps today's full binding set — fourier-series, rtz, and tutorial use it today."

references:
- name: "Shared detection + graph (new)"
  url: crates/ironpad-common/src/cell_deps.rs
- name: "Cache key recipe (normalization lands inside content_hash_with_fingerprint)"
  url: crates/ironpad-common/src/cache_key.rs
- name: "Scaffold binding filter"
  url: crates/ironpad-app/src/compiler/scaffold.rs
- name: "Cascade recipe (consumer)"
  url: crates/ironpad-app/src/components/executor.rs
- name: "Daemon terminal-outcome classification"
  url: crates/ironpad-cli/src/daemon.rs

acceptance_tests:
- id: uat-001
  name: "A cell referencing no upstream slots runs alone (no cascade), and its cache key is invariant to unreferenced upstream type tags"
  command: cargo make uat
  uat_status: verified
- id: uat-002
  name: "Run All on a notebook with an early failing cell still executes later independent cells; true dependents show prerequisite-failed instead of running"
  command: cargo make uat
  uat_status: verified
- id: uat-003
  name: "cells.run reports prerequisite_failed only when the target transitively depends on the failed cell; an unrelated failure does not terminate the wait"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "ironpad-common cell_deps: referenced_slots, normalize_previous_types, transitive deps"
  priority: 1
  status: done
  notes: "SlotRefs { slots: BTreeSet<usize>, last: bool }; cellN matched with word boundaries (not preceded/followed by [A-Za-z0-9_]); last matched with word boundaries and not preceded by '.' (excludes .last() calls; false positives only over-cascade). normalize: last => unchanged; else keep referenced slot types positionally, blank the rest, truncate trailing blanks (cross-notebook cache sharing for independent cells). Applied INSIDE content_hash_with_fingerprint so no key computation can forget it. CACHE_EPOCH 8 -> 9."
- id: T-002
  title: "Scaffold binds only referenced slots"
  priority: 1
  status: done
  notes: "generate_lib_rs: bind cellN for typed AND referenced slots; last alias (plus its target binding) only when last referenced; skip input reconstruction entirely when no bindings (an unused __ironpad_inputs__ would warn). Preamble math follows what is generated. Sim/LiveView wrappers unchanged (never bind)."
- id: T-003
  title: "Cascade + failure policy in the unified run flow"
  priority: 1
  status: done
  notes: "unexecuted_upstream gains a source_of closure and returns only the transitive dependency closure (unexecuted, runnable, notebook order). Editor sources: Monaco handle get_value() when mounted else model (model lags edits by the 1s debounce). Viewer sources: its IronpadCell vec. On failure: drop transitive dependents from the queue and mark them blocked (editor: cell_blocked_by, now in ALL modes not just reactive; viewer: new blocked map + badge with the failed cell named); independents keep running. abort_run_cascade retired for run-queue failures (kept for user-initiated terminate/AbortError, which still clears everything)."
- id: T-004
  title: "Daemon: dependency-aware prerequisite_failed"
  priority: 1
  status: done
  notes: "run_outcome takes a depends-on predicate computed from the daemon's cached notebook via the shared helper; another cell's failure is terminal only when the target transitively depends on it (else keep waiting — the queue continues now). Missing cache degrades to terminal (no hangs). Update the doc comment that codified clear-queue-on-any-failure."
- id: T-005
  title: "Live-check piped-input guard uses the shared detection"
  priority: 2
  status: done
  notes: "dispatch_live_check's (0..10) cellN substring scan and the references_pipes gate move to referenced_slots (handles N>=10 and last)."
- id: T-006
  title: "Tests: detection, normalization, scaffold, cascade, daemon, e2e"
  priority: 1
  status: done
  notes: "Unit: word-boundary matrix (cell1 vs cell10 vs mycell1 vs cell1x; .last() vs last), normalization invariance + truncation, scaffold binds referenced-only + preamble counts, transitive closure incl. last-conservative. e2e: run-all continues past the failing cell on a borrows-shaped notebook (independent cell runs, dependent shows blocked); single Run of an independent cell does not compile upstream."
---

# Summary

Running a cell cascades only the upstream cells it actually (transitively) consumes, and a failure in a run queue skips only the failed cell's true dependents. The scaffold binds only referenced piping slots (making the skip safe) and the cache key ignores unreferenced upstream types (making independent cells cache-portable). `CACHE_EPOCH` bumps to 9.

# Problem

Today every typed upstream output is eagerly bound and deserialized whether or not the cell uses it, so the cascade must run ALL unexecuted upstream cells, and any failure kills the whole queue (editor) — one deliberate compile-fail teaching cell leaves `borrows` and `dynosaur` readers (and the output-capture tool) with nothing after cell 1.

# Goals

1. Exact-up-to-conservatism dependency detection shared by scaffold, cache key, cascade, and daemon.
2. Run All / autorun continue past failures; dependents get an explicit prerequisite-failed state.
3. Cache keys invariant to unreferenced upstream types.

# Non-Goals (MVP)

- Dependency-aware stale marking / downstream output invalidation (stays position-based, conservative).
- Blanking unreferenced slots' bytes in `assemble_cell_inputs` (wasteful but correct; bindings ignore them).
- Parallel execution of independent cells (queue stays sequential).

# History

- **2026-08-06** — Created after the PRD-0059 seam landed; failure policy and scope confirmed by Aaron (continue past failures, grants Q separate).
- **2026-08-06** — Implemented and closed, with two discoveries beyond the plan. (1) The v0.17.0 `gen_blocks` gate was INERT: cells compiled on edition 2021, where `gen {` is a parse error before the gate matters — the unit tests covered detection, never a real compile. Cells now build on edition 2024 (two notebooks renamed a `gen` variable; `all_public_notebook_cells_compile` re-validated all 46). (2) Even on 2024, `#[wasm_bindgen]`'s syn parser rejects `gen { … }` ("expected identifier or integer" — it parses a struct literal), so `cell_main` became a one-line trampoline into a plain inner fn whose body syn never sees; base preamble 6 -> 7. A third catch came from e2e: `__ironpad_inputs__` raw-buffer access is a consuming surface the detection had to cover (conservative, like `last`) or reconstruction disappears under it. CACHE_EPOCH 8 -> 9. Gate: 822 unit, 13 integration (incl. a real gen-cell compile), full Playwright green; borrows 6/8 and dynosaur 5/6 now capture saved outputs — the wall this PRD existed to remove.
