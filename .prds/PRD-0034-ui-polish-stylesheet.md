---
id: PRD-0034
title: "UI polish & stylesheet fixes (prettying up)"
status: done
owner: "Aaron Roney"
created: 2026-07-02
updated: 2026-07-02

principles:
- "The app must look intentional in BOTH themes — light mode is not an afterthought"
- "Prefer theme tokens over hardcoded colors so themes stay in sync"
- "Fix broken affordances (invisible controls) before decorative polish"

references:
- name: "Review report — P3 UI polish (stylesheet bugs + observed rough edges)"
  url: reviews/2026-07-02-codebase-review.md

acceptance_tests:
- id: uat-001
  name: "The cell drag-reorder handle is visible on cell hover and can drag-reorder"
  command: cargo make playwright
  uat_status: unverified
- id: uat-002
  name: "Hover states (toolbar toggles, dropdown items, session button) are visible in light mode"
  command: cargo make playwright
  uat_status: unverified
- id: uat-003
  name: "A cell with very large output is capped and scrolls internally; wide images don't overflow the card"
  command: cargo make playwright
  uat_status: unverified
- id: uat-004
  name: "An empty notebook shows a real empty-state (not two faint text buttons in a void)"
  command: cargo make playwright
  uat_status: unverified

tasks:
- id: T-001
  title: "Fix the permanently invisible drag handle"
  priority: 1
  status: done
  notes: "main.scss:2526 sets .ironpad-drag-handle opacity:0; the only reveal rule (:2577) is a descendant selector that can't match because the handle is in .ironpad-cell-side-actions, a SIBLING of .ironpad-cell-card. Fix: remove the self opacity:0 (the parent .ironpad-cell-side-actions already fades in via .ironpad-cell-row:hover at :996), delete the dead :2577 rule, and merge the two conflicting .ironpad-drag-handle blocks (:1004 and :2526, which also fight over width/height/font-size). DOM: cell_item.rs:1211,1388,1391."
- id: T-002
  title: "Theme-aware hover token — fix invisible light-theme hovers (systemic)"
  priority: 1
  status: done
  notes: "28 hardcoded rgba(255,255,255,0.06-0.12) hover backgrounds (e.g. :339,:790,:871,:1019,:1045,:2253,:2286,:2604) go invisible on light surfaces; three hovers set text to --ip-text-on-accent (#fff both themes) on transparent bgs (:2255 toolbar toggle, :2287 dropdown item, :2606 session button) -> white-on-white. Fix: add --ip-hover-overlay (rgba(255,255,255,0.08) in :root, rgba(0,0,0,0.06) in [data-theme=light]); replace the 28 overlays with the token; switch the 3 hover text colors to var(--ip-text-primary)."
- id: T-003
  title: "Cap output height; constrain wide media"
  priority: 1
  status: done
  notes: ".ironpad-output-body:1401 has overflow-y:auto but no max-height (contrast compile-result:1131 caps 300px) -> huge stdout/table grows the card unboundedly. Fix: max-height:480px. Also add .ironpad-output-visual img { max-width:100%; height:auto; } — cell_output.rs:288 emits fixed pixel img sizes (SVG :1424 and canvas :2749 already cap at 100%, img does not)."
- id: T-004
  title: "Style the three unstyled classes + fix progress bar tokens + focus glow"
  priority: 2
  status: done
  notes: ".ironpad-shared-source-panel (shared_editor_panel.rs:35) has no selector -> reuse .ironpad-shared-deps rules (:696). .ironpad-widget-button (cell_output.rs:937, view_only_notebook.rs:1129) unstyled -> add elevated-bg/border/radius/hover rule. .ironpad-loading (public_notebook.rs:36, shared_notebook.rs:35) unstyled -> center it (display:flex;justify-content:center;padding:48px 0;color:var(--ip-text-tertiary)). Progress bar uses undefined --ip-bg-tertiary + dead #4fc3f7 (:2556,:2563) -> var(--ip-bg-elevated) track, var(--ip-accent) fill. Focus glow :1541 hardcodes rgba(233,69,96,0.3) -> track the theme accent."
- id: T-005
  title: "Empty-notebook state + always-visible-ish cell action rail"
  priority: 2
  status: done
  notes: "Confirmed live: a new notebook is two faint '+ Code / + Markdown' text buttons floating in dark space -> add a real empty-state CTA card with keyboard hints. Cell action rail (.ironpad-cell-side-actions) is opacity 0 until hover -> undiscoverable and unusable on touch; make it always visible at reduced opacity, brightening on hover."
- id: T-006
  title: "Disambiguate overloaded red: neutral focus border; consistent cross-theme accent"
  priority: 2
  status: done
  notes: "Confirmed live: focused-cell border, error border, and brand accent are all the same red, so a focused cell looks broken. Give focus a neutral/accent-blue border distinct from error. Also 'New Notebook'/primary buttons are blue in dark theme but red in light theme — pick one accent per element across themes."
- id: T-007
  title: "Compile badge + byte-count readability"
  priority: 3
  status: done
  notes: "Confirmed live: cryptic '✓ 468+7MS' badge and '1 bytes'. Spell out ('468 ms compile · 7 ms run') or add a tooltip; pluralize/format byte counts."
- id: T-008
  title: "View-only page polish: duplicate title, filename-as-title, cached/fresh toggle look, nested scroll"
  priority: 3
  status: done
  notes: "public_notebook.rs:19-20 sets the header center title AND view_only_notebook.rs:183 renders an <h1> with the same name -> shown twice; header also shows the filename (lagrange-points) not the notebook title. 'Cached/Fresh' toggle (view_only_notebook.rs:184-197) reuses .ironpad-theme-toggle classes so it looks like the dark/light switch -> give it its own class. Nested scroll containers (main.scss:358 .ironpad-content vs :1951 .view-only-cells) risk a double scrollbar -> let .ironpad-content own scrolling."
- id: T-009
  title: "Apply saved light-theme to Monaco on load; label icon-only toolbar buttons"
  priority: 3
  status: done
  notes: "app_layout.rs:135-149 reads the stored theme but setTheme(...) is only called in the toggle click handlers (:226-241,:268-283) -> on reload with theme=light the toggle shows light while Monaco renders dark until re-toggled; on mount, if light, apply data-theme + call IronpadMonaco.setTheme once. Also add title/aria-label to the icon-only ☰ (mod.rs:502-510) and ⚙ (mod.rs:707-716) toolbar buttons."
- id: T-010
  title: "Home card hover lift; dead modifier classes; error-panel For key collision"
  priority: 3
  status: done
  notes: "Home cards only change border-color on hover (:526) -> add transform:translateY(-2px)+box-shadow with a matching transition. Drop or style dead modifier classes (ironpad-notebook-badge private/public at home_page.rs:297,329; base ironpad-cell-type-badge at cell_item.rs:1266). error_panel.rs:64,156 <For> keys collide for duplicate diagnostics/spans -> include the item index in the key."
- id: T-011
  title: "Mobile: keep the notebook title in the header at narrow widths; fork double-click guard"
  priority: 3
  status: done
  notes: "Confirmed live: notebook title disappears from the header on mobile (no responsive slot). Give it a responsive slot or a truncated variant. Also (from components review) view_only_notebook.rs:158-178,205-207: Fork button not disabled during async fork -> double-click creates two notebooks and a save failure still navigates; gate on an in_flight signal and navigate only after a successful save."
---

# Summary

The stylesheet has genuine bugs, not just taste: the drag-reorder handle is permanently invisible, light-theme hover states disappear (white-on-white across 28 hardcoded overlays), cell output has no height cap, and several classes referenced in markup have no CSS at all. This epic fixes those and applies the visual polish that makes the app read as intentional in both themes — the batch with the biggest visual payoff per line changed.

# Problem

Two systemic issues dominate: (1) the drag handle sets its own `opacity:0` with a reveal rule that targets the wrong element, so drag-to-reorder is undiscoverable; (2) hover feedback is built from hardcoded white-alpha overlays and `--ip-text-on-accent` (white in both themes), so light mode loses all hover states. The rest are smaller layout/token/affordance fixes observed in code review and the live browser audit.

# Goals

1. Drag handle visible and functional; cell actions discoverable (incl. touch).
2. Hover/focus states correct in both themes via shared tokens.
3. Output and media constrained to the card; real empty state.
4. Consistent accent usage and readable badges across themes and viewports.

# Technical Approach

Almost entirely `style/main.scss` token/selector work, with small markup touches (`home_page.rs`, `view_only_notebook.rs`, `mod.rs`, `error_panel.rs`, `app_layout.rs`). T-001-T-003 are the "broken feature / broken theme / layout blowout" trio and should land first. Verify each in the browser at desktop and ~390px widths.

# Assumptions

- The theme mechanism is `[data-theme="light"]` on a root element (per existing PRD-0028 usage).
- Existing `--ip-*` custom properties are the right vocabulary to extend.

# Constraints

- Dark mode must not regress (it is primary).
- Prefer tokens over new hardcoded colors; every new color needs a light + dark value.

# References to Code

- `style/main.scss` (primary surface — see task notes for exact line numbers)
- `crates/ironpad-app/src/pages/notebook_editor/{cell_item.rs,cell_output.rs,mod.rs}`
- `crates/ironpad-app/src/pages/{home_page.rs,public_notebook.rs,shared_notebook.rs}`
- `crates/ironpad-app/src/components/{app_layout.rs,view_only_notebook.rs,error_panel.rs,shared_editor_panel.rs}`

# Non-Goals (MVP)

- A full responsive/mobile redesign (only the title-in-header regression is fixed here).
- Notebook card thumbnails or a visual redesign of the card grid.

# History

(Entries appended during implementation go below this line.)

## 2026-07-03 — Complete (11/11)
All UI-polish tasks landed on branch `fix/prd-0034-ui-polish`, each gate-clean (cargo make clippy + 491 tests). Browser-verified: drag handle, light-theme hover token, output cap, empty-notebook state. Commits: 878f364 (T-001/002/003), 35cf54c (T-004/006/010 + rail), 15598ca (T-007/008/009), 3ddca98 (T-005/T-011).
