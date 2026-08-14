---
id: PRD-0065
title: "Studio chrome for view-only notebooks, and a theme-aware Plot"
status: draft
owner: "Aaron Roney"
created: 2026-08-14
updated: 2026-08-14

depends_on:
- PRD-0047
- PRD-0056

principles:
- "The plot palette is a contrast bug before it is a design preference: 1.11:1 measured on prod against a 4.5:1 requirement."
- "Themeing belongs in CSS, not in the cell. A cell renders once and its SVG is persisted; only CSS can follow a theme toggle across a saved snapshot."
- "Design against the notebooks that exist. 43 of 46 have code lines wider than the mock's source pane, and 51% of all cells are markdown."
- "One component (components/view_only_notebook.rs) renders six surfaces. Every change lands on /public, /shared, /mutable and three embeds at once."
- "No new webfonts. v0.19.5 took 164KB off first paint; matching a type spec must not put it back."

references:
- name: "Design handoff: ironpad notebook chrome (Studio direction)"
  url: design_mock/design_handoff_notebook_chrome/README.md
- name: "WCAG 2.1 SC 1.4.3 Contrast (Minimum)"
  url: https://www.w3.org/WAI/WCAG21/Understanding/contrast-minimum.html
- name: "PRD-0056 persisted cell outputs (saved_output capture)"
  url: .prds/PRD-0056-persisted-cell-outputs.md

acceptance_tests:
- id: uat-001
  name: "Plot text meets WCAG AA contrast in BOTH themes on a real rendered chart"
  command: cargo make playwright -- plot-theme
  uat_status: unverified
- id: uat-002
  name: "A plot SVG copied out of ironpad still renders standalone (var() fallbacks present)"
  command: cargo make test
  uat_status: unverified
- id: uat-003
  name: "CSS custom properties survive SVG sanitization, including the fallback form"
  command: cargo make test
  uat_status: unverified
- id: uat-004
  name: "Saved outputs captured before this change re-theme on toggle without recapture"
  command: cargo make playwright -- plot-theme
  uat_status: unverified
- id: uat-005
  name: "Cell rail scroll-spy selects the cell nearest the top of the viewport, and click scrolls to it"
  command: cargo make playwright -- studio-chrome
  uat_status: unverified
- id: uat-006
  name: "Embeds render with no rail and no top bar (chrome-less contract preserved)"
  command: cargo make playwright -- embed
  uat_status: unverified
- id: uat-007
  name: "No webfont is requested on any notebook route"
  command: cargo make playwright -- first-paint
  uat_status: unverified
- id: uat-008
  name: "Full gate"
  command: cargo make uat
  uat_status: unverified

tasks:
- id: T-001
  title: "Emit CSS custom properties from the Plot SVG instead of baked hex"
  priority: 1
  status: todo
  notes: "plot.rs already post-processes its own output (the transparent-background replace in render_svg). Extend that seam: render with sentinel RGB constants, then map them to var(--ip-plot-*, <fallback>). Fallbacks are REQUIRED: the CopyButton hands this SVG to the clipboard and it must render standalone."
- id: T-002
  title: "Define the --ip-plot-* token block in both themes"
  priority: 1
  status: todo
  notes: "text, muted, grid, zero, axis, series-1..4. Reuse existing --ip-* values where they already match the handoff; light bg-app is already exactly #f5f6fa. Must pass tools/css-vars-check.py."
- id: T-003
  title: "Apply the handoff's chart style: no chart-area frame, horizontal dashed gridlines only, 5-6 ticks per axis"
  priority: 2
  status: todo
  notes: "Handoff 'Chart style guidance'. Titles move out of the SVG into the cell output caption (T-008), so the plot gets its full height. Bars get end labels and the rank opacity ramp."
- id: T-004
  title: "Bump CACHE_EPOCH and recapture every saved output"
  priority: 1
  status: todo
  notes: "ironpad-cell source is NOT part of the cache key, so a plot change is invisible to it by construction. CACHE_EPOCH 10 -> 11, then cargo make capture-outputs. capture-outputs-check is in ci and will fail until this runs."
- id: T-005
  title: "Studio visual language: cell frame, header row, output surface"
  priority: 2
  status: todo
  notes: "Bordered frame with header (index, title, state pill, meta, run glyph) over an output surface on bg-code. Stacked source-above-output, NOT the mock's two columns (see Constraints). Honor the existing per-cell collapsed flag."
- id: T-006
  title: "Left rail: cell outline with status dots, timings, runtime and deps groups"
  priority: 2
  status: todo
  notes: "New component. Suppressed in embed mode. Collapses to a top dropdown under ~1000px per the handoff's responsive rule."
- id: T-007
  title: "Rail scroll-spy and click-to-scroll"
  priority: 2
  status: todo
  notes: "Selected row follows the cell nearest the top of the viewport. Use IntersectionObserver, owned page-scoped and detached on cleanup (see the leaked-Closure rule in DEVELOPMENT.md)."
- id: T-008
  title: "Cell output caption row (chart title + right-side note)"
  priority: 2
  status: todo
  notes: "Owns the title that T-003 removes from the SVG. Falls back to no caption when a cell has no title."
- id: T-009
  title: "Top bar: breadcrumb, read-only pill, richer status bar"
  priority: 3
  status: todo
  notes: "Must NOT displace the global header's auth surface (PRD-0053 put sign-in there deliberately). Resolve per Constraints: extend the existing toolbar rather than introducing a second full-width bar."
- id: T-010
  title: "Regression tests: contrast in both themes, sanitizer var() survival, rail behavior, embed chrome-less, no webfont"
  priority: 1
  status: todo
  notes: "Contrast test must measure a REAL rendered chart at a computed ratio, not assert on a hex string. Watch each test fail before trusting it."
---

# Summary

Two changes to the view-only notebook surface, from a design handoff in
`design_mock/design_handoff_notebook_chrome/`.

**Tier 1** makes `Plot` theme-aware. Its palette is hardcoded dark, which renders
chart text at **1.11:1 contrast** in light theme against a 4.5:1 requirement.

**Tier 2** adopts the handoff's "Studio" chrome for the read-only notebook view:
a left cell rail with status and timings, a bordered cell frame, a cleaned-up
output surface, and a richer status bar.

The handoff's signature two-column cell body (source left, output right) is
deliberately **not** adopted. See Constraints.

# Problem

## The plot palette is a live accessibility bug

`crates/ironpad-cell/src/plot.rs:11` sets `COLOR_TEXT` to `#EAEAEA` and uses it
for every axis label, tick label, in-SVG title, and axis stroke. Measured on
production at `/public/charts-with-plot` in light theme:

| | |
| --- | --- |
| text fill | `#EAEAEA` |
| surface behind it | `rgb(245, 246, 250)` |
| contrast ratio | **1.11:1** |
| WCAG AA minimum | 4.5:1 |

Every chart label is invisible in light theme. It reaches 6 public notebooks and
15 cells, plus every user notebook that plots, across `/public`, `/shared`,
`/mutable` and all three `/embed` routes. `charts-with-plot` currently states
*"All charts render as SVG with a dark theme that matches ironpad's UI"* directly
above an unreadable chart, which documents the limitation as if it were a
feature.

## The read-only view is plainer than the product deserves

The current view is a single column of cells with no navigation. `facet` has 21
cells and `nuclear-reactor` has 15; there is no outline, no per-cell timing at a
glance, and no runtime or dependency summary.

# Goals

1. Chart text meets WCAG AA in both themes, on every surface that renders a plot.
2. A theme toggle re-themes charts, **including saved-output snapshots captured
   before this change**, with no recapture and no re-execution.
3. A plot SVG copied to the clipboard still renders standalone.
4. The read-only notebook gains an outline rail, a cell frame, and a status bar
   matching the handoff, on a layout that fits the notebooks that exist.
5. Zero new bytes on the first-paint critical path.

# Technical Approach

## Tier 1: CSS custom properties in the SVG

The cell runs in a worker and its SVG is serialized into `saved_output`
(PRD-0056). It therefore **cannot** know the page theme at render time, and a
snapshot captured in dark mode would stay dark forever. Passing a theme into the
cell is the wrong seam.

Instead the SVG carries CSS custom properties and the page themes it:

```
plotters renders with sentinel RGB  ->  post-process maps them to var(--ip-plot-*, fallback)
                                    ->  inner_html injects it INLINE into the document
                                    ->  --ip-plot-* resolve against the active theme
```

Three facts make this work, all verified rather than assumed:

1. **`render_svg` already post-processes its own output.** The transparent
   background is produced by `buf.replace("fill=\"#000000\"", ...)`. This extends
   an existing seam rather than inventing one.
2. **Panels are injected inline**, via `<div inner_html=safe>` in
   `output_render.rs`, not as an `<img>` or a `data:` URI. Custom properties only
   resolve for inline SVG, so this is load-bearing.
3. **The sanitizer passes `var()` through untouched.** Probed directly against
   `sanitize_svg`:

   ```
   in:  <text fill="var(--ip-plot-text)"> ... <rect fill="var(--ip-plot-grid, #ccc)"/>
   out: <text fill="var(--ip-plot-text)"> ... <rect fill="var(--ip-plot-grid, #ccc)"></rect>
   ```

   Both the bare and the fallback form survive. T-010 pins this as a permanent
   regression test, because ammonia treats some presentation attributes as
   URL-typed and a future allowlist edit could silently start stripping them.

**Fallbacks are required, not optional.** Every panel carries a `CopyButton`, and
`Download .ironpad` embeds the SVG. Outside ironpad's stylesheet an unresolved
`var()` with no fallback drops the attribute entirely, which is the same silent
failure mode `tools/css-vars-check.py` exists to catch in SCSS.

**Cache invalidation.** The cell cache key hashes cell source, cargo_toml, types
and a toolchain fingerprint. **`ironpad-cell`'s own source is not in it**, so
this change is invisible to the cache by construction. `CACHE_EPOCH` 10 -> 11
(T-004), then `cargo make capture-outputs`; `capture-outputs-check` runs in `ci`
and will fail until that lands.

## Tier 2: Studio chrome

`components/view_only_notebook.rs` (1,092 lines) renders `/public`, `/shared`,
the `/mutable` reader, and three `/embed` routes. Changes land on all six, so
embed mode gates the rail and the top bar off.

The token table in the handoff maps almost 1:1 onto the existing 50 `--ip-*`
custom properties, and light `bg-app` already matches `#f5f6fa` exactly. The new
tokens are the `--ip-plot-*` block (T-002) plus whatever the frame needs.

# Assumptions

- The handoff's colors, spacing and radii are final and matched as specified.
- The chart SVGs in the mock are a style target, not markup to emit; plotters
  generates the real ones.
- Per-cell `collapsed` flags in the notebook file continue to be honored. The
  mock always shows source, but story-style notebooks set it per cell and
  `public-notebooks.spec.ts` asserts the counts.

# Constraints

## The two-column cell body is out of scope, on measurement

The handoff specifies `grid-template-columns: 340px 1fr` with the source pane at
Fira Code 11.5px and 14px padding, which fits **45 characters**. Measured across
`public/notebooks/`:

| metric | value |
| --- | --- |
| notebooks whose p95 code line exceeds 45 chars | **43 of 46** |
| median p95 code line | 76 chars |
| median longest code line | 92 chars |
| longest code line (`lagrange-points`) | 316 chars |

The mock's own sample cells run 30 to 45 characters. Nearly every real cell would
wrap or scroll horizontally.

Separately, **215 of 420 cells (51%) are markdown**, and the mock contains none;
a source-left/output-right grid has no meaning for a prose cell. The mock's own
target notebook has 11 cells in reality and the mock shows 6, all code.

Cells stay stacked (source above output). Most of the Studio density comes from
the frame, header row and output surface, which are adopted in full.

## The "stale" state is not reachable on `/public`

Public source is read-only, and `capture-outputs-check` runs in `ci` precisely so
a shipped notebook's saved output can never be stale. The handoff's stale
treatment (amber dot, pill, dimmed output, rerun affordance) is real in the
**editor** and arguably in the `/mutable` reader (draft vs published), both of
which the handoff declares out of scope. Not built here.

## The top bar must not displace the auth surface

The handoff replaces the global header with a notebook top bar carrying the theme
toggle. The real header carries GitHub, theme, and the auth avatar, which
PRD-0053 placed there deliberately after it was hidden in the status bar.
Implementing the top bar literally would either drop sign-in on notebook pages or
stack two full-width bars. T-009 extends the existing toolbar instead.

## No webfonts

`--ip-font-mono` and `--ip-font-sans` already *name* Fira Code and Inter but load
neither; they are system-only fallback stacks. Matching the type spec faithfully
would mean self-hosting roughly 100 to 200KB of woff2 onto the critical path that
v0.19.5 just cleared of 164KB. The existing stacks stay. Sizes, weights and
spacing from the handoff are adopted; the families are not.

# References to Code

- `crates/ironpad-cell/src/plot.rs` — palette constants (11-13), the
  `render_svg` post-process seam (137-168), and every `into_font().color()` /
  `axis_style` / `configure_mesh` site.
- `crates/ironpad-app/src/components/output_render.rs:321` — `DisplayPanel::Svg`
  inline injection.
- `crates/ironpad-app/src/sanitize.rs` — `sanitize_svg`, `SVG_TAGS`,
  `ID_SCOPED_SVG_TAGS`.
- `crates/ironpad-app/src/components/view_only_notebook.rs` — the six-surface
  component; `ViewOnlyCell`, `ViewOnlyCodeCell`, `ViewOnlyOutput`.
- `crates/ironpad-common/src/cache_key.rs:81` — `CACHE_EPOCH`.
- `style/main.scss` — `--ip-*` block at 51, `.view-only-*` rules from 2629.
- `tools/capture-outputs.mjs`, `tools/css-vars-check.py`.

# Non-Goals (MVP)

- The two-column cell body (measured out; see Constraints).
- Stale detection and the rerun pill on `/public` (unreachable state).
- Editor chrome. The editor's preview mode renders `ViewOnlyNotebook`, so it
  inherits Tier 2 for free; its own toolbar and Monaco panes are untouched.
- Webfonts.
- Chart tooltips beyond what `Plot::tooltips` already does.
- A user-facing `Plot::color()` / `Theme` API as shown in the mock's cell 6.

# History

- 2026-08-14: Created. Tier 1 + Tier 2 approved; two-column layout rejected on
  measurement. Contrast bug (1.11:1) measured on production; `var()` survival
  through the sanitizer probed directly before committing to the approach.
