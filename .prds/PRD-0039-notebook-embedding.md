---
id: PRD-0039
title: "Notebook embedding via iframe routes and a JS loader"
status: active
owner: "Aaron Roney"
created: 2026-07-10
updated: 2026-07-10

principles:
- "Reuse the existing ViewOnlyNotebook rendering path; the embed is a chrome variant, not a new renderer"
- "Cells stay runnable inside embeds; a static render defeats the point of ironpad"
- "Minimal changes: no routing restructure, no new server function; the embed rides the existing shared/public fetch paths"
- "Degrade honestly: threaded (rayon) cells cannot run in cross-origin embeds; say so in the UI rather than failing silently"

references:
- name: "Design draft (session scratchpad)"
  url: "file:///tmp/claude-1000/-home-twitchax-projects-ironpad/be75dcf4-4c50-4c40-b52a-fe3bfde20bf5/scratchpad/embed-design-draft.md"
- name: "crossOriginIsolated and SharedArrayBuffer requirements"
  url: https://developer.mozilla.org/en-US/docs/Web/API/Window/crossOriginIsolated

acceptance_tests:
- id: uat-001
  name: "Embed route renders a public notebook without app chrome and with runnable cells"
  command: npx playwright test tests/e2e/embed.spec.ts
  uat_status: unverified
- id: uat-002
  name: "embed.js loader injects an auto-resizing iframe for both snippet styles and handles multiple embeds per page"
  command: npx playwright test tests/e2e/embed.spec.ts
  uat_status: unverified
- id: uat-003
  name: "Embed snippet button copies iframe and script snippets from the view-only toolbar"
  command: npx playwright test tests/e2e/embed.spec.ts
  uat_status: unverified
- id: uat-004
  name: "Full gate stays green"
  command: cargo make uat
  uat_status: unverified

tasks:
- id: T-001
  title: "Embed routes + EmbedNotebookPage (shared + public sources), chrome-less AppLayout"
  priority: 1
  status: todo
  notes: "Routes /embed/shared/{hash} and /embed/public/{filename} in lib.rs; one page component parameterized by source; AppLayout skips header/status bar on /embed/ path prefix via use_location (SSR-deterministic); LayoutContext still provided."
- id: T-002
  title: "Embed mode in ViewOnlyNotebook: hide Fork, add 'Open in ironpad' badge"
  priority: 1
  status: todo
  notes: "Optional embed prop (default false). Fork writes IndexedDB, which is partitioned in cross-origin iframes; replace with target=_blank canonical link. Slim footer badge links to /shared/{hash} or /notebook/public/{filename}."
- id: T-003
  title: "In-frame height reporter posting ironpad:embed:height to parent"
  priority: 2
  status: todo
  notes: "Load + ResizeObserver; include per-embed id echoed from the ?embed_id= query param; only active when window.parent !== window."
- id: T-004
  title: "public/embed.js host-side loader"
  priority: 2
  status: todo
  notes: "Vanilla JS, no deps. Supports script[data-notebook] self-invocation and .ironpad-embed[data-notebook] divs; data-notebook is 'shared/{hash}' or 'public/{filename}'. Origin derived from document.currentScript.src; origin-validated message listener; multiple embeds per page via unique embed ids; iframe width 100%, loading=lazy."
- id: T-005
  title: "Embed snippet UI in the view-only toolbar"
  priority: 2
  status: todo
  notes: "Embed button opens a popover with iframe snippet + script snippet, each with CopyButton. Visible on /shared and /notebook/public pages; hidden in embed mode itself."
- id: T-006
  title: "CORP header for /embed/* and /embed.js"
  priority: 3
  status: todo
  notes: "Cross-Origin-Resource-Policy: cross-origin scoped to embed responses so COEP-isolated embedders can frame us. Server main.rs layer; config test."
- id: T-007
  title: "Threaded-cell degradation notice in embeds"
  priority: 3
  status: todo
  notes: "Cross-origin iframes are never crossOriginIsolated, so SharedArrayBuffer (rayon cells) is unavailable. Detect !crossOriginIsolated in embed mode and show a friendly note on cells that request rayon instead of a raw failure."
- id: T-008
  title: "Playwright embed.spec.ts + docs"
  priority: 3
  status: todo
  notes: "Host-page fixture exercising both snippet styles: renders, chrome absent, resize message fires, snippet copy works. Update CLAUDE.md routes list + README embed section."
---

# Summary

Any shared or public notebook becomes embeddable on third-party pages two ways: a plain iframe pointing at a chrome-less `/embed/...` route, or a one-line `embed.js` script that injects and auto-resizes that iframe. Cells remain fully runnable inside the embed.

# Problem

Notebooks currently live only on the ironpad origin. Anyone writing a blog post or docs page about Rust cannot show a live, runnable notebook inline; the best they can do is screenshot + link. Embedding is the standard distribution mechanism for interactive content (CodePen, observable, YouTube all won on it), and ironpad's whole differentiator (cells that actually compile and run) is exactly the thing worth embedding.

# Goals

1. A chrome-less embed view for both read-only notebook sources (shared hash + public file), with cells runnable.
2. A no-dependency `embed.js` loader that handles iframe injection, height auto-resize, and multiple embeds per page.
3. A copy-snippet affordance in the view-only toolbar so users never hand-write the markup.
4. Honest degradation for threaded cells in cross-origin contexts.

# Technical Approach

The embed is a chrome variant of the existing view-only path, not a new renderer:

```
third-party page
  └─ <script src=embed.js data-notebook="shared/abc123">      (loader)
       └─ <iframe src="https://ironpad/embed/shared/abc123?embed_id=ip-1">
            └─ EmbedNotebookPage                               (new page)
                 └─ ViewOnlyNotebook embed=true                (existing component)
                      └─ Monaco / executor / KaTeX             (already global in shell)
            └─ height reporter → postMessage → loader resizes iframe
```

- **Routing**: two new routes inside the existing `Router`/`AppLayout` tree. `AppLayout` checks `use_location().pathname` for the `/embed/` prefix and renders children without header/status bar; this is SSR-deterministic and avoids restructuring routes or LayoutContext provision.
- **Headers**: the server's global `COOP: same-origin` + `COEP: require-corp` do not block being framed (no `X-Frame-Options`, no `frame-ancestors`). One addition: `CORP: cross-origin` scoped to embed responses so embedders that are themselves COEP-isolated can load us.
- **Resize protocol**: `{ type: "ironpad:embed:height", id, height }` messages, loader validates event origin against the iframe origin and matches `id` so multiple embeds coexist.

# Assumptions

- Shared and public notebooks remain the only server-resolvable read-only sources (private notebooks are client-side IndexedDB and cannot be embedded).
- The global shell head (Monaco, executor, KaTeX) is acceptable payload for embeds in v1; a slimmer embed shell is a possible follow-up.

# Constraints

- Cross-origin iframes can never be `crossOriginIsolated`, so SharedArrayBuffer is unavailable: rayon/threaded cells will not run in third-party embeds (plain and async cells work). T-007 surfaces this in-UI.
- Fork-to-IndexedDB is unavailable/partitioned inside cross-origin iframes; embed mode links out instead.

# References to Code

- `crates/ironpad-app/src/lib.rs` — route table + shell (global scripts)
- `crates/ironpad-app/src/components/view_only_notebook.rs` — the reused renderer (fork button at :159-186, toolbar at :195)
- `crates/ironpad-app/src/components/app_layout.rs` — chrome + LayoutContext
- `crates/ironpad-app/src/pages/{shared_notebook.rs,public_notebook.rs}` — fetch patterns the embed page mirrors
- `crates/ironpad-server/src/main.rs:146-153` — existing COOP/COEP header layers (add CORP here)
- `crates/ironpad-app/src/components/copy_button.rs` — snippet copy UI
- `tests/e2e/` — Playwright conventions

# Non-Goals (MVP)

- Embedding private (IndexedDB) notebooks.
- A slim embed-specific JS bundle (embeds load the standard shell).
- Web-component (`<ironpad-notebook>`) embedding without an iframe boundary.
- Server-side render-to-static-HTML export (that's the existing Export HTML feature).

# History

- 2026-07-10: PRD created from the approved brainstorm design (session: code-quality + notebooks + embed).
