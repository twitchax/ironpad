---
id: PRD-0006
title: "QoL Improvements & View Mode Polish"
status: active
owner: "Aaron Roney"
created: 2026-03-10
updated: 2026-03-10

principles:
- "View mode should present a polished, read-only experience"
- "Remove friction — eliminate redundant buttons and confusing UX"
- "Autosave should be invisible — no interruption, no manual button needed"
- "Monaco editors should grow with content, not constrain it"

references:
- name: "PRD-0005 (interactivity)"
  url: .prds/PRD-0005-interactivity.md

acceptance_tests:
- id: uat-001
  name: "Auto-run is on by default; Run Stale button removed from toolbar"
  command: cargo make uat
  uat_status: unverified
- id: uat-002
  name: "View/Edit toggle renders as a pencil/eyeball segmented toggle and does not overlap footer"
  command: cargo make uat
  uat_status: unverified
- id: uat-003
  name: "View mode shows rendered markdown, auto-runs all cells, and hides add/settings/menu buttons"
  command: cargo make uat
  uat_status: unverified
- id: uat-004
  name: "Public notebook page renders identically to editor view mode"
  command: cargo make uat
  uat_status: unverified
- id: uat-005
  name: "Manual save button removed; autosave does not steal editor focus"
  command: cargo make uat
  uat_status: unverified
- id: uat-006
  name: "Widget example notebook(s) exist and demonstrate interactive elements"
  command: cargo make uat
  uat_status: unverified
- id: uat-007
  name: "Monaco editor grows vertically with content instead of scrolling at fixed height"
  command: cargo make uat
  uat_status: unverified

tasks:
- id: T-001
  title: "Auto-run on by default and remove Run Stale button"
  priority: 1
  status: done
  notes: >
    (1) Change auto_run default from false to true in mod.rs:51 (RwSignal::new(true)).
    (2) Remove the Run Stale button from the toolbar (mod.rs ~lines 279-299).
    (3) Remove the auto-run toggle button since auto-run is always-on now. If we want
    the user to be able to disable auto-run, keep it as a toggle in the gear dropdown instead.
    Actually, keep the toggle but move it into the gear (⚙) dropdown menu as a checkbox item
    so it doesn't clutter the toolbar.
    Files: mod.rs (toolbar section), main.scss (remove .ironpad-auto-run-toggle if unused).

- id: T-002
  title: "Redesign View/Edit toggle and fix positioning"
  priority: 1
  status: done
  notes: >
    (1) Replace the single button with a segmented toggle showing two icons side-by-side:
    [✏️ | 👁] — pencil for edit mode, eyeball for view mode. The active side is highlighted.
    Click either side to switch modes. This makes it clear which mode you're IN vs. going TO.
    (2) Fix positioning so it doesn't overlap the status bar (28px tall at bottom).
    Current CSS: bottom: 24px. Change to bottom: 44px (28px status bar + 16px gap) or
    position it relative to the content area instead of the viewport.
    (3) Style: rounded pill shape, dark bg, subtle border, active side uses accent color.
    Files: mod.rs (toggle component ~lines 565-571), main.scss (.ironpad-mode-toggle).

- id: T-003
  title: "View mode: show markdown, auto-run all, hide add/settings buttons"
  priority: 1
  status: done
  notes: >
    Three fixes for view mode behavior:
    (1) Markdown cells should NOT be collapsed in view mode — only Code cells should collapse.
    Fix in cell_item.rs: the Effect that collapses cells on view mode entry should skip
    Markdown cells (check cell.cell_type == CellType::Markdown and leave those expanded).
    Actually better: in view mode, Markdown cells should show only their rendered output
    (not the editor), and Code cells should show only their output panel.
    (2) When entering view mode, auto-run ALL Code cells (not just stale ones). Collect all
    Code cell IDs and push to run_all_queue. This ensures the notebook presents a complete,
    up-to-date view.
    (3) Hide AddCellButton components in view mode. Either pass is_view_mode to AddCellButton
    and conditionally render nothing, or wrap the add-button rows in mod.rs with a Show guard.
    Also ensure the side action buttons (.ironpad-cell-side-actions) are hidden in view mode.
    Files: cell_item.rs (collapse Effect, view mode conditionals), mod.rs (AddCellButton
    visibility, auto-run trigger), main.scss (add-cell-row hide in view mode).

- id: T-004
  title: "Unify public notebook rendering with editor view mode"
  priority: 2
  status: done
  notes: >
    The public notebook page (view_only_notebook.rs) should render identically to the
    editor's view mode — showing rendered outputs (markdown, HTML, SVG, etc.) with code
    collapsed. Currently the public notebook shows source code in read-only Monaco editors,
    which is a different experience from the editor's view mode.
    Options:
    (a) Refactor view_only_notebook to reuse the same cell rendering components from
    notebook_editor but in a read-only/view-mode configuration.
    (b) Update view_only_notebook to match the visual style — collapse code by default,
    show outputs prominently, use the same CSS classes.
    Recommend (b) for now — simpler, less refactoring. Add a code collapse toggle per cell
    in the view-only notebook, defaulting to collapsed. Show rendered output panels by default.
    Files: view_only_notebook.rs, main.scss.

- id: T-005
  title: "Remove manual save button and fix autosave focus loss"
  priority: 1
  status: done
  notes: >
    Two save-related fixes:
    (1) Remove the manual save button from the header. Autosave handles persistence.
    In notebook_editor mod.rs, remove `layout.show_save_button.set(true)` and the
    Ctrl+S keyboard handler. Or keep Ctrl+S as an instant save (skip debounce) but
    remove the visual button.
    (2) Fix autosave stealing focus. The debounced save in cell_item.rs (~line 968-978)
    calls state.notebook.update() which modifies a RwSignal. If this triggers a re-render
    of the cell list (because notebook changed), Monaco editors could be destroyed and
    recreated, losing focus. Fix: use update_untracked() or batch the notebook update
    so it doesn't trigger downstream re-renders. Alternatively, the save closure could
    only write to IndexedDB without updating the reactive notebook signal (since the
    source signal already has the current value).
    Files: mod.rs (save button/keyboard), cell_item.rs (debounce save closure),
    app_layout.rs (show_save_button).

- id: T-006
  title: "Widget example notebook(s)"
  priority: 2
  status: done
  notes: >
    Create 1-2 public notebooks demonstrating interactive widgets:
    (1) "interactive-widgets.ironpad" — A showcase notebook using most/all 6 widget types:
    slider, dropdown, checkbox, text input, number input, switch. Show how widget values
    flow to downstream cells and how markdown can display computed results.
    Cool example idea: a "unit converter" or "tip calculator" or "color mixer" that uses
    sliders and dropdowns with reactive downstream output.
    (2) Update public/notebooks/index.json to include the new notebook(s).
    (3) Include build speed optimizations in shared deps (opt-level=1, codegen-units=16).
    Files: public/notebooks/interactive-widgets.ironpad (new), public/notebooks/index.json.

- id: T-007
  title: "Monaco editor auto-height (grow with content)"
  priority: 2
  status: done
  notes: >
    Make the Monaco editor container grow vertically to fit content instead of using a
    fixed min-height with internal scrolling.
    Current: .ironpad-monaco-container { min-height: 200px; } — editor scrolls internally.
    Target: editor container grows with content lines, up to a max-height (e.g., 600px),
    then scrolls. This gives a better editing experience for short cells.
    Implementation: Use Monaco's onDidContentSizeChange event to update the container height.
    In bridge.js, after creating the editor:
    editor.onDidContentSizeChange(() => {
      const contentHeight = editor.getContentHeight();
      container.style.height = Math.min(contentHeight, 600) + 'px';
      editor.layout();
    });
    Set initial height based on content. Remove fixed min-height from CSS (or set a
    small min-height like 60px for empty cells).
    Files: public/monaco/bridge.js (onDidContentSizeChange), main.scss (.ironpad-monaco-container).
---

# Summary

A collection of QoL fixes addressing view mode polish, toolbar cleanup, autosave issues,
editor ergonomics, and example notebooks for the new interactive widget system.

# Problem

After PRD-0005 added view mode, interactive widgets, and auto-run, several rough edges remain:
- View mode hides markdown content (critical UX bug)
- The View/Edit toggle is confusing (shows mode name, not action)
- Auto-run is off by default but should be on
- Run Stale button is redundant with auto-run
- Manual save button is redundant with autosave
- Autosave causes editor focus loss (typing interrupted)
- No example notebooks demonstrating interactive widgets
- Monaco editors use fixed height instead of growing with content

# Goals

1. Make view mode presentation-ready: rendered markdown, auto-run, clean UI
2. Remove redundant UI elements (Run Stale, save button)
3. Fix the autosave focus loss bug
4. Add widget showcase notebooks
5. Make code editors grow with content

# Technical Approach

## Auto-run & Toolbar Cleanup (T-001)

- Flip `auto_run` default to `true`
- Remove Run Stale button (auto-run handles it)
- Move auto-run toggle into gear dropdown as a settings item

## View/Edit Toggle (T-002)

Replace single button with segmented toggle:
```
┌──────────┐
│ ✏️ │ 👁 │
└──────────┘
```
Active side highlighted with accent color. Fixed at bottom-left, above status bar.

## View Mode Fixes (T-003)

- Markdown cells: skip collapse in view mode, show rendered output
- Code cells: collapse code, show output panel only
- Auto-run all cells when entering view mode
- Hide: AddCellButton, side action buttons, cell settings

## Autosave Fix (T-005)

The debounced save closure calls `state.notebook.update()`, which triggers reactive updates
that may re-render the cell list and destroy Monaco editors. Fix by either:
- Using `update_untracked()` to prevent reactive propagation
- Only writing to IndexedDB without updating the signal

## Monaco Auto-Height (T-007)

Use Monaco's `onDidContentSizeChange` to dynamically resize the container:
```javascript
editor.onDidContentSizeChange(() => {
    const h = Math.min(Math.max(editor.getContentHeight(), 60), 600);
    container.style.height = h + 'px';
    editor.layout();
});
```

# Assumptions

- Autosave focus loss is caused by reactive signal updates during save
- Monaco's onDidContentSizeChange fires reliably for content changes
- View mode auto-run-all won't cause issues with uncompiled cells

# Constraints

- Must not break existing edit mode workflow
- Public notebook page refactoring kept minimal (visual parity, not code reuse)
- Monaco max-height prevents viewport overflow for very long cells

# References to Code

- Auto-run default: `mod.rs:51` — `RwSignal::new(false)`
- Run Stale button: `mod.rs:279-299`
- View/Edit toggle: `mod.rs:565-571`, `main.scss:1853-1872`
- View mode collapse: `cell_item.rs:59-67` (Effect)
- AddCellButton: `mod.rs:552,560`
- Save button: `app_layout.rs:27,84`, `mod.rs:94-96` (Ctrl+S)
- Autosave debounce: `cell_item.rs:960-1013`
- Monaco config: `public/monaco/bridge.js:212-217`
- Monaco container CSS: `main.scss:914-919`
- Public notebook: `view_only_notebook.rs`
- Status bar: `main.scss:265-288` (28px height)

# Non-Goals (MVP)

- Full component reuse between editor view mode and public notebook (future refactor)
- Persistent auto-run preference in localStorage
- Collaborative editing or multi-user save conflicts

# History
(Entries appended during implementation go below this line.)
