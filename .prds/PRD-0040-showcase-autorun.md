---
id: PRD-0040
title: "Route-scoped auto-run: showcase notebooks run on load, shared never does"
status: done
owner: "Aaron Roney"
created: 2026-07-10
updated: 2026-07-10

depends_on:
- PRD-0025
- PRD-0039

principles:
- "Auto-run follows the trust boundary, which is already encoded in the routes: first-party public content runs, arbitrary shared content never does"
- "In embeds the decision belongs to the embedder (data-autorun), never to the notebook author or the reader by surprise"

references:
- name: "PRD-0025 opt-in reactivity principle"
  url: ".prds/PRD-0025-reactive-dataflow.md"

acceptance_tests:
- id: uat-001
  name: "Public showcase notebooks auto-run on load (smoke tests pass with no Run All click)"
  command: npx playwright test tests/e2e/notebook-smoke.spec.ts
  uat_status: verified
- id: uat-002
  name: "Embeds are click-to-run by default and run on load with ?autorun=1"
  command: npx playwright test tests/e2e/embed.spec.ts
  uat_status: verified
- id: uat-003
  name: "Full gate stays green"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "autorun prop on ViewOnlyNotebook, unified with the reactive_mode auto-run path"
  priority: 1
  status: done
  notes: "PublicNotebookPage passes autorun=true; SharedNotebookPage does not (and never will); EmbedPublicPage honors ?autorun=1; EmbedSharedPage ignores it."
- id: T-002
  title: "embed.js forwards data-autorun as ?autorun=1"
  priority: 2
  status: done
  notes: "Both snippet styles (script attribute + placeholder-div attribute). README documents it."
- id: T-003
  title: "Tests: smoke tests drop Run All clicks; embed spec covers both default-idle and opt-in"
  priority: 2
  status: done
  notes: "Removing the Run All clicks from the smoke tests IS the behavioral regression lock for showcase auto-run."
---

# Summary

Public showcase notebooks auto-run on page load again; shared notebooks never auto-run anywhere; embeds are click-to-run unless the embedder opts in with `data-autorun`.

# Problem

PRD-0025 made execution opt-in globally ("never surprise users with auto-execution"), which silently turned the showcase gallery into blank pages behind a Run All button. The principle is right where content is untrusted or the surprise is real, but it protects nobody on the first-party gallery: a Mandelbrot demo that renders nothing is just a worse demo.

# Technical Approach

The trust boundary is already encoded in the routes, so the policy lives there and nowhere else:

| Surface | Auto-run | Why |
|---|---|---|
| `/notebook/public/{file}` | **always** | First-party curated content; compiles are cache-warm; execution is sandboxed client-side wasm |
| `/shared/{hash}` | **never** | Arbitrary user content: auto-run would mean opening a link executes someone else's code and forces compiles server-side |
| `/embed/public/...` | `?autorun=1` only | The embedder knows their audience; readers didn't choose ironpad |
| `/embed/shared/...` | **never** | Same as `/shared`, and the route ignores the param entirely |

`ViewOnlyNotebook` gains an `autorun` prop unified with the existing `reactive_mode` auto-run effect; no schema change, no `reactive_mode: true` sprinkled on data files (which would also change editing semantics on forks).

# Non-Goals

- Auto-running shared notebooks under any flag, ever.
- Per-notebook autorun metadata (the route already carries the trust signal).

# History

- 2026-07-10: Created and implemented in one pass after Aaron approved the route-scoped design; supersedes the smoke tests' temporary Run All clicks added earlier the same day.
