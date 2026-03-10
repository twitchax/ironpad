---
id: PRD-0002
title: "Visual & UX Polish"
status: active
owner: Aaron Roney
created: 2026-03-09
updated: 2026-03-09

principles:
  - "Use CSS custom properties for all colors — no hardcoded hex values in Rust or SCSS"
  - "Follow existing patterns (home page delete, home page navigate) when adding editor-page equivalents"
  - "Leptos hooks must be called in the component body, never inside async blocks"
  - "Public seed notebooks must produce working cell pipelines end-to-end"

references:
  - name: "MegaPrd (architecture, design, all details)"
    url: "MegaPrd.md"
  - name: "PRD-0001 (MVP first pass)"
    url: ".prds/PRD-0001-first-pass.md"

acceptance_tests:
  - id: uat-001
    name: "Favicon is served at /favicon.ico and renders in the browser tab"
    command: "curl -s -o /dev/null -w '%{http_code}' http://localhost:3111/favicon.ico"
    uat_status: not_verified
  - id: uat-002
    name: "No hardcoded hex color values in Rust component files"
    command: "grep -rn '#[0-9a-fA-F]\\{6\\}' crates/ironpad-app/src/components/ && exit 1 || exit 0"
    uat_status: not_verified
  - id: uat-003
    name: "Delete button visible on notebook editor page; clicking it removes the notebook and navigates home"
    command: cargo make uat
    uat_status: not_verified
  - id: uat-004
    name: "Monaco editor retains focus after save is triggered"
    command: cargo make uat
    uat_status: not_verified
  - id: uat-005
    name: "Async & HTTP public notebook cell pipeline works: cell_1 returns raw JSON, cell_3 parses it"
    command: cargo make uat
    uat_status: not_verified
  - id: uat-006
    name: "cargo make ci passes (clippy + fmt-check + tests)"
    command: cargo make ci
    uat_status: not_verified

tasks:
  - id: T-001
    title: "Add favicon.ico"
    priority: 1
    status: done
    notes: >
      Add a favicon.ico to public/ so it is served by cargo-leptos.
      Use a simple Rust-themed or ironpad-branded icon.  A simple SVG
      favicon in the HTML head is also acceptable.

  - id: T-002
    title: "Fix hardcoded inline colors in Rust components"
    priority: 1
    status: done
    notes: >
      Replace hardcoded `style="color: #4a4a6a; ..."` in:
        - crates/ironpad-app/src/components/markdown_cell.rs (empty placeholder)
        - crates/ironpad-app/src/components/view_only_notebook.rs (empty markdown cell)
      Create a CSS class (e.g., `.ironpad-placeholder`) using `color: var(--ip-text-muted)`
      and `font-style: italic`, then apply it instead of inline styles.

  - id: T-003
    title: "Fix hardcoded SCSS colors in view-only error panel"
    priority: 1
    status: done
    notes: >
      In style/main.scss, the `.view-only-error` block uses raw
      `rgba(220, 53, 69, ...)` and `#dc3545`.  Replace with
      CSS-variable-based equivalents using `var(--ip-error)`.

  - id: T-004
    title: "Add delete-notebook button to the notebook editor page"
    priority: 2
    status: done
    notes: >
      Add a delete button (🗑 icon or similar) to the notebook editor
      toolbar in notebook_editor.rs.  Follow the home page pattern:
        1. Confirm via window.confirm_with_message().
        2. Call crate::storage::client::delete_notebook(&id).
        3. Navigate to "/" after deletion.
      Only show for private notebooks (not public/shared views).
      Call use_navigate() in the component body, not inside async.

  - id: T-005
    title: "Fix Monaco editor focus loss after save"
    priority: 2
    status: done
    notes: >
      In notebook_editor.rs, saving triggers layout signal updates
      and/or toast dispatch that cause the active Monaco editor to lose
      focus.  Investigate the save effect (save_generation effect) and
      either: (a) prevent the focus-stealing side effect, or
      (b) restore focus to the active editor after save completes.
      The MonacoEditor component exposes a JS handle; use
      window.IronpadMonaco.focus(editorId) or similar.

  - id: T-006
    title: "Fix Async & HTTP notebook cell[1] to return raw JSON"
    priority: 1
    status: done
    notes: >
      In public/notebooks/async-http.ironpad, cell_1 ("HTTP GET")
      currently returns a formatted summary string:
        format!("Response length: {} bytes\n\nFirst 500 chars:\n{}", ...)
      But cell_3 ("JSON Parse") expects raw JSON via `serde_json::from_str(&cell0)`.
      Fix cell_1 to return the raw `response` string directly so the
      pipeline works end-to-end.
---

# PRD-0002: Visual & UX Polish

## Summary

A collection of visual and UX fixes to polish the ironpad experience:
favicon, consistent theming via CSS variables, a delete button on the
editor page, fixing editor focus after save, and correcting the
Async & HTTP seed notebook's cell pipeline.
