---
id: PRD-0030
title: "Post-review delivery roadmap (2026-07-02 codebase review)"
status: active
owner: "Aaron Roney"
created: 2026-07-02
updated: 2026-07-02

principles:
- "Un-break cell execution first — nothing else matters if cells don't run"
- "Ship in reviewable batches; each epic PRD is independently landable"
- "Every behavioral fix ships a test (unit, integration, or Playwright)"
- "This is a fun project: pragmatic hardening over perfect isolation"

references:
- name: "Full review report (~90 findings, file:line + fixes)"
  url: reviews/2026-07-02-codebase-review.md

acceptance_tests:
- id: uat-001
  name: "All child epic PRDs (0031-0037) reach status: done"
  command: cargo make uat
  uat_status: unverified
- id: uat-002
  name: "A fresh cell (sim::read + host_message) compiles, links, and runs end-to-end"
  command: cargo make test-integration
  uat_status: unverified

tasks:
- id: T-001
  title: "PRD-0031 Toolchain & cell execution (P0 — un-break everything)"
  priority: 1
  status: done
  notes: "DONE + merged to main (commits ..1c2a8c5). Fixed host-import linking + wasm-bindgen drift; pinned nightly-2025-12-22 (T-006) which unblocked the whole build/test gate. Final review: ready to merge. See PRD-0031."
- id: T-002
  title: "PRD-0032 Editor UX bugs (E1-E11)"
  priority: 2
  status: done
  notes: "DONE + merged. All 12 tasks; uat-001/002/003/005 verified via committed e2e tests (uat-004 skipped/manual). See PRD-0032."
- id: T-003
  title: "PRD-0033 Data safety: storage & executor robustness"
  priority: 2
  status: in-progress
  notes: "6/8 done + merged (storage resilience + executor-bridge robustness). T-007 (GPU per-cell scoping) + rest of T-008 (rayon/panic/blob concurrency) deferred — deep executor-core, unverifiable here. See PRD-0033."
- id: T-004
  title: "PRD-0034 UI polish / stylesheet"
  priority: 3
  status: done
  notes: "Mostly independent of 0031; can run in parallel. Biggest visual payoff per line. See PRD-0034."
- id: T-005
  title: "PRD-0035 Collaboration / session layer"
  priority: 3
  status: done
  notes: "Independent track; do when agent collaboration is the focus. See PRD-0035."
- id: T-006
  title: "PRD-0036 Compiler/server correctness + pragmatic hardening"
  priority: 3
  status: done
  notes: "Some items (cache-key work) overlap 0031; sequence cache-key items after 0031. See PRD-0036."
- id: T-007
  title: "PRD-0037 Leak sweep"
  priority: 4
  status: todo
  notes: "Mechanical, cross-cutting; land after 0032/0034 touch the same components. See PRD-0037."
---

# Summary

This is the master roadmap for delivering the ~90 findings from the 2026-07-02 codebase review (six parallel code reviewers + a hands-on browser audit of the running app). The findings are grouped into seven epic PRDs (0031-0037). This PRD tracks their sequencing, dependencies, and overall completion.

# Problem

The review surfaced one product-breaking class of bug (cells did not compile/link on the current toolchain), a set of user-visible editor bugs, data-safety gaps in storage and the executor, systemic UI/stylesheet issues (broken drag handle, invisible light-theme hovers), collaboration desyncs, compiler/server correctness bugs, and a set of resource leaks. Delivering ~90 findings as one changeset would be unreviewable; delivering them ad hoc risks dropping the load-bearing ones. This roadmap batches them into landable epics with an explicit order.

# Goals

1. Restore cell execution on the current toolchain and keep it from regressing (0031).
2. Fix the editor bugs users hit within minutes (0032).
3. Close data-loss and stuck-state gaps in storage/executor (0033).
4. Make the UI look intentional in both themes and fix the broken drag handle (0034).
5. Fix collaboration desyncs and connection-lifecycle bugs (0035).
6. Correct compiler/server bugs and apply pragmatic (not perfect) hardening (0036).
7. Eliminate the timer/closure/listener leaks that degrade long sessions (0037).

# Technical Approach

## Sequencing

```
0031 (toolchain, P0) ─┬─> 0032 (editor UX)      ─┐
                      ├─> 0033 (data safety)      ├─> 0037 (leak sweep, last)
0034 (UI polish) ─────┤   (parallel-safe)         │
0035 (collab) ────────┤                           │
0036 (server) ────────┘   (cache-key item waits on 0031)
```

- **0031 goes first.** Every other epic's browser-level acceptance test needs cells that actually run. It is small and urgent.
- **0034 and 0035 are parallel-safe** — the stylesheet and the collab/server layers barely overlap the editor work, so they can proceed alongside 0032/0033 if capacity allows.
- **0037 (leak sweep) goes last** because it touches the same components as 0032/0034; landing it after avoids rebasing churn.
- Within **0036**, the cache-key task (folding toolchain versions into the blake3 key) shares surface with 0031's version-pinning task — do it right after 0031.

## Execution

Each epic PRD lists its findings as tasks with `file:line` and the fix in `notes`. Detailed, test-first task steps are expanded per batch just before executing that batch (via the writing-plans / subagent-driven-development flow), rather than all upfront — priorities may shift as batches land.

## Sandboxing decision (recorded)

Full build sandboxing (gVisor / rootless nested containers) is **explicitly out of scope**. Ironpad deploys to Fly.io, where each instance already runs in a Firecracker microVM, and the compilation proxy already filters network egress — so the blast radius of the `compile_cell` RCE is one ephemeral VM, not the host. The pragmatic hardening that *is* worth doing (cell_id validation, share-size cap, timeout process-group kill, output-HTML sanitization, non-root container user) lives in PRD-0036. Full multi-tenant isolation is a documented "only if this becomes a serious multi-tenant service" note, not a work item.

# Assumptions

- The dev loop (`cargo make dev`), CI (`cargo make ci`), integration (`cargo make test-integration`), and Playwright (`cargo make playwright`) all work as documented.
- Fixes land on feature branches and merge via the normal `cargo make ci` gate.

# Constraints

- Clippy cleanliness is enforced (`-D warnings`); every touched file must pass.
- No public-API signature changes unless a finding explicitly requires it.

# References to Code

- `.prds/reviews/2026-07-02-codebase-review.md` — the full findings report with file:line and fixes for every task in the child PRDs.
- Child PRDs: PRD-0031 through PRD-0037.

# Non-Goals (MVP)

- Full build sandboxing / multi-tenant isolation (documented residual risk; see PRD-0036).
- Reworking the protocol or model architecture beyond the specific desync fixes in PRD-0035.
- New features — this roadmap is strictly review remediation.

# History

(Entries appended during implementation go below this line.)

## 2026-07-02 -- Roadmap created
- Created from the 2026-07-02 review. Seven epic PRDs (0031-0037) scoped; sequencing and sandboxing decision recorded above.

## 2026-07-02 -- Toolchain-pin prerequisite discovered (during PRD-0031)
- While verifying PRD-0031, found the default `nightly-2026-06-01` cannot compile the `thaw` UI dependency (`error: queries overflow the depth limit!`), which breaks `cargo make {build,test,test-integration,ci,uat}` on any cold build (it had only been working via a stale cached `thaw` rlib). This blocks the verification gate for **every** epic, so PRD-0031 T-006 pins `rust-toolchain.toml` to `nightly-2025-12-22` as a cross-cutting prerequisite. All subsequent epics assume this pin is in place. Bump the pin forward once thaw/leptos/tachys ship a fix.
