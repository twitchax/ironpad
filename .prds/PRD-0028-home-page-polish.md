---
id: PRD-0028
title: "Home page polish: footer, light mode cards, hero tagline"
status: done
owner: "Aaron Roney"
created: 2026-03-22
updated: 2026-03-23

principles:
- "Small, focused UI improvements — no architectural changes"
- "Dark mode is primary; light mode must still look intentional"
- "Footer should provide contextual information, not confuse"

references:
- name: "Screenshot: home page dark mode"
  url: home-page.png
- name: "Screenshot: home page light mode"
  url: home-light-mode.png

acceptance_tests:
- id: uat-001
  name: "Footer is hidden on the home page (/ route)"
  command: cargo make playwright
  uat_status: unverified
- id: uat-002
  name: "Footer is visible on notebook editor, public notebook, and shared notebook pages"
  command: cargo make playwright
  uat_status: unverified
- id: uat-003
  name: "Notebook cards have visible borders/shadows in light mode"
  command: cargo make playwright
  uat_status: unverified
- id: uat-004
  name: "Home page displays a tagline below the Notebooks heading"
  command: cargo make ci
  uat_status: verified

tasks:
- id: T-001
  title: "Hide footer status bar on the home page"
  priority: 1
  status: done
  notes: "The AppLayout wraps all pages with a <footer> containing StatusBar. Either conditionally render based on route, or pass a flag from the page. The footer should still appear on /notebook/*, /notebook/public/*, and /shared/* routes."
- id: T-002
  title: "Add subtle border/shadow to notebook cards in light mode"
  priority: 1
  status: done
  notes: "In style/main.scss, the .ironpad-notebook-card uses var(--ip-border) which is likely too faint in light mode. Add a box-shadow or increase border opacity for the light theme. Check that dark mode is unaffected."
- id: T-003
  title: "Add hero tagline to the home page"
  priority: 1
  status: done
  notes: "Add a subtitle line below the h1 'Notebooks' heading in home_page.rs. Copy: 'Interactive Rust notebooks -- compile to WebAssembly, run in the browser.' Style it as muted secondary text."
---

# Summary

Three small UI polish items for the home page: hide the irrelevant footer status bar, improve notebook card visibility in light mode, and add a welcoming tagline for new visitors.

# Problem

1. **Footer confusion**: The status bar shows "Status: Ready | Compiler: stable | Cells: 0" on the home page, which is meaningless — there's no active notebook. It should only appear on pages where a notebook is loaded.

2. **Light mode card contrast**: Notebook cards in light mode have nearly invisible borders, making them blend into the background. They need a subtle shadow or stronger border.

3. **No introduction for new visitors**: The home page jumps straight into a notebook grid with no explanation of what ironpad is. A brief tagline would orient first-time visitors.

# Goals

1. Hide the footer on the home/list page; keep it on editor, public, and shared notebook pages.
2. Make notebook cards visually distinct in light mode with a subtle shadow or border.
3. Add a one-line tagline under the "Notebooks" heading.

# Technical Approach

## T-001: Footer visibility

The `AppLayout` component in `app_layout.rs` unconditionally renders `<footer><StatusBar /></footer>`. Options:

- **Preferred**: Add a `show_footer: bool` field to `LayoutContext` (or a new signal). The home page sets it to `false`; notebook pages leave it `true` (default). The layout conditionally renders the footer based on this flag.
- **Alternative**: Use `leptos_router::use_location()` inside AppLayout to check the current path and hide the footer on `/`. Less clean but avoids threading a prop.

## T-002: Light mode card contrast

In `style/main.scss`, add a light-theme-specific rule for `.ironpad-notebook-card`:

```scss
[data-theme="light"] .ironpad-notebook-card {
    box-shadow: 0 1px 3px rgba(0, 0, 0, 0.08), 0 1px 2px rgba(0, 0, 0, 0.06);
    border-color: rgba(0, 0, 0, 0.12);
}
```

Check how the theme attribute is applied (could be `data-theme`, a class, or CSS custom properties).

## T-003: Hero tagline

In `home_page.rs`, add a `<p>` element after the `<h1>"Notebooks"</h1>` with the tagline text. Style with a new `.ironpad-home-tagline` class using `--ip-text-secondary` color and smaller font size.

# Assumptions

- The theme toggle mechanism (dark/light) uses CSS custom properties or a data attribute that can be targeted in SCSS.
- `LayoutContext` can be extended without breaking existing pages.

# Constraints

- Changes must not affect dark mode appearance (T-002).
- Footer must still work correctly on all notebook page types (T-001).

# References to Code

- `crates/ironpad-app/src/components/app_layout.rs` — AppLayout, StatusBar, LayoutContext
- `crates/ironpad-app/src/pages/home_page.rs` — home page component
- `style/main.scss` — .ironpad-notebook-card styles, theme variables

# Non-Goals (MVP)

- Notebook card thumbnail previews
- Mobile responsiveness audit
- Reordering cards (featured/pinned notebooks)
- Empty state improvements for private notebook filter

# History

(Entries appended during implementation go below this line.)

## 2026-03-22 -- Batch Execution (T-001, T-002, T-003)
- **Tasks completed**: T-001, T-002, T-003
- **Changes**:
  - T-001: Used `leptos_router::hooks::use_location()` in AppLayout to conditionally render footer only when pathname != "/". Footer still renders on all notebook routes.
  - T-002: Added `[data-theme="light"] .ironpad-notebook-card` rule with box-shadow and border-color for light mode card visibility.
  - T-003: Added `<p class="ironpad-home-tagline">` with tagline text in home_page.rs, styled with `.ironpad-home-tagline` in main.scss.
- **Test results**: cargo make ci — 479 passed, 0 failed, 6 skipped
- **UATs verified**: uat-004 (tagline present in compiled output, verified by CI)
- **UATs deferred**: uat-001, uat-002, uat-003 (require visual/browser verification via playwright or manual check)
- **Constitution compliance**: No violations
