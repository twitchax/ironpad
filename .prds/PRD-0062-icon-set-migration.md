---
id: PRD-0062
title: "Replace Unicode UI glyphs with a shipped Lucide icon set behind an IconLabel component"
status: done
owner: "Aaron Roney"
created: 2026-08-07
updated: 2026-08-07

principles:
- "Consistency requires shipping the vectors. Measured inventory (test modules excluded): 47 symbol glyphs render in the UI across 118 uses — 5 are forced-color emoji (Emoji_Presentation=Yes), 9 are emoji-capable and color themselves on some Windows/Android configurations (including the run button, U+25B6, 14 uses), and 33 are non-emoji whose SHAPE comes from whatever system font supplies the codepoint (DejaVu / Segoe UI Symbol / Apple Symbols). No choice of codepoint fixes the last group; only shipped geometry does."
- "Per-set subcrate, never the umbrella crate. Measured: icondata_lu (Lucide only) compiles in 1.9s, costs ~250 bytes of release wasm per icon USED, and the ~1600 unused icons cost zero because they are plain statics the linker strips. The umbrella `icondata` crate is what the compile-time complaints describe (its crates.io metadata alone exceeds 10 MB) and is out of scope."
- "ONE SVG wrapper definition. The Leptos component and the Export-HTML string builder both need the same markup, and this codebase has a documented history of exactly that class of fork (the editor/viewer run paths, the two capture-manifest recipes). `icon_svg_markup` is the single definition; the component wraps it, export.rs calls it."
- "Call sites name a ROLE, not a vendor icon: `icons::HISTORY`, not `LuHistory`. One mapping module means a reskin, a weight change, or a whole-set swap is a single-file edit — and it keeps the icon-set dependency out of 50 files."
- "Icons size in `em` and paint in `currentColor`, so they inherit the surrounding font-size and the existing dark-theme custom properties for free. No size prop at the common call site."
- "Accessibility follows the visible text: an icon beside a label is decorative (`aria-hidden`), an icon-only control carries the name on the BUTTON (its existing `title`), not on the svg. This is also why most Playwright selectors survive — they already target `button[title=...]`, not glyph text."

references:
- name: "Glyph inventory + presentation classification (this PRD's measurements)"
  url: https://www.unicode.org/Public/UCD/latest/ucd/emoji/emoji-data.txt
- name: "Lucide icon set (ISC; Feather-derived subset MIT)"
  url: https://lucide.dev
- name: "Per-set Rust data crate"
  url: https://crates.io/crates/icondata_lu
- name: "Status-badge String context (the restructure site)"
  url: crates/ironpad-app/src/pages/notebook_editor/cell_item.rs
- name: "Export-HTML string builder (second rendering path)"
  url: crates/ironpad-app/src/pages/notebook_editor/export.rs

acceptance_tests:
- id: uat-001
  name: "No Unicode symbol glyph renders in UI markup: the guard script finds zero in non-comment Rust/JS, and ci fails when one is reintroduced"
  command: cargo make ci
  uat_status: verified
- id: uat-002
  name: "Icons render identically under SSR and after hydration, inherit currentColor in both themes, and Export HTML remains a self-contained file with inline SVG (no font or network dependency)"
  command: cargo make uat
  uat_status: unverified
- id: uat-003
  name: "Playwright is green with no selector matching on glyph text; icon-only controls expose their name via the button, and icon+label controls expose the label once (not doubled by the svg)"
  command: cargo make playwright
  uat_status: unverified

tasks:
- id: T-001
  title: "Add icondata_lu + license notice"
  priority: 1
  status: done
  notes: "Workspace dep on icondata_lu (pin a version, not `*`). Lucide is ISC with a Feather-derived MIT subset; both are notice-only, so reproduce both notices in a LICENSES/ file (or THIRD-PARTY.md) and reference it from README. No npm dependency — the icons ride in the Rust bundle, so the Export-HTML and SSR paths need no asset fetch."
- id: T-002
  title: "components/icon.rs: Icon, IconLabel, and the one icon_svg_markup definition"
  priority: 1
  status: done
  notes: |
    `icon_svg_markup(icon, extra_class) -> String` is THE wrapper: viewBox, stroke/fill,
    stroke-width/linecap/linejoin from the IconData fields, `width/height: 1em`,
    `aria-hidden=true`, class `ironpad-icon`. Path data is injected via inner_html —
    safe because it is a compile-time constant from the icon crate, never user input;
    say so in a comment so a future reader does not have to re-derive it.

    Components (the shape Aaron asked for):
      #[component] pub fn IconLabel(icon: Icon, #[prop(into)] label: String) -> impl IntoView
      #[component] pub fn Icon(icon: Icon) -> impl IntoView   // icon-only; the BUTTON names it

    `label: String` (not a Signal) is deliberate: every reactive call site already sits
    inside a `move ||` closure in a `view!`, so the closure rebuilds the IconLabel and a
    signal prop would buy nothing but complexity.

    Unit tests: markup contains the viewBox and path data; em sizing present; aria-hidden
    present; two different icons produce different path data (guards a mapping typo).
- id: T-003
  title: "icons.rs mapping table: role names, one file"
  priority: 1
  status: done
  notes: "`pub const HISTORY: Icon = icondata_lu::LuHistory;` etc. — one named const per UI role, with the glyph it replaces in a trailing comment for reviewability. ~35 entries after collapsing pairs (▸/▾ is one chevron rotated by CSS; ◆/◇ and ☾/☼ are toggle states; ▶/⏸/⏹/⏭ are the transport set). No call site outside this file may name an `icondata_lu::*` symbol — that is what makes a set swap one edit."
- id: T-004
  title: "Migrate markup call sites"
  priority: 1
  status: done
  notes: "The bulk: ~40 sites across cell_item.rs, notebook_editor/mod.rs, view_only_notebook.rs, home_page.rs, app_layout.rs, error_panel.rs, animation_canvas.rs, copy_button.rs, mutable_notebook.rs, embed_notebook.rs, session_panel.rs. Mechanical: `\"🕘 History\"` becomes `<IconLabel icon=icons::HISTORY label=\"History\"/>`; a bare glyph in a titled button becomes `<Icon icon=icons::RUN/>` with the existing `title` untouched."
- id: T-005
  title: "Restructure the String-context sites"
  priority: 1
  status: done
  notes: "The real cost driver, and it is small: the CellStatus badge (cell_item.rs ~1190-1206) matches to `String` today and must yield a view instead — it already lives inside a `{move || …}` closure, so the arms return `IconLabel` and the compile-time-ms arm keeps its `format!` for the text half only. export.rs builds an HTML String and calls `icon_svg_markup` directly (☑/☐ checkbox arms), which is precisely why that helper is a string function rather than component-only."
- id: T-006
  title: "SCSS: .ironpad-icon sizing, alignment, and dead-style removal"
  priority: 2
  status: done
  notes: "`.ironpad-icon { width: 1em; height: 1em; flex-shrink: 0; vertical-align: -0.125em; }` and an inline-flex + gap rule for the label wrapper. Sweep style/main.scss for rules that existed to nudge glyph metrics (line-height/letter-spacing hacks around the old characters) and delete them rather than leaving them to fight the new box."
- id: T-007
  title: "Tests: e2e selector migration and a11y assertions"
  priority: 1
  status: done
  notes: "Two specs select on glyph text and WILL break: shared-live-check.spec.ts:63 (`getByText(\"Code ●\")`) and editor-ux.spec.ts:326 (`hasText: \"☰\"`). Move both to class/title selectors. Every other spec already targets `button[title=...]` and must keep passing untouched — that is the regression signal for T-002's a11y decision. Add an assertion that an icon+label control exposes its accessible name exactly once."
- id: T-008
  title: "ci guard: no symbol glyph in rendered source"
  priority: 2
  status: done
  notes: "A `tools/glyph-check.py` in the `gen-completions-check` / `capture-outputs-check` mold: scan non-comment lines of crates/**/src and public/*.js for General_Category=S codepoints >= U+2000, allowlist the deliberate exceptions (box-drawing section rules in comments, the ✓ that is punctuation in prose, cell-runtime test fixtures), fail with the file:line and the suggested `icons::` const. Wire into the `ci` task. This is what stops the next feature from reintroducing a bare emoji — constitution rule 7, enforce the invariant rather than documenting it."
---

# Summary

Replace the UI's 47 Unicode symbol glyphs with a shipped Lucide icon set, rendered through one `IconLabel` (icon + text) / `Icon` (icon only) component pair, so every affordance looks identical on every machine.

# Problem

ironpad draws its UI affordances with Unicode characters, which means the browser picks the shape from whatever font on the user's machine happens to cover the codepoint. That produces three distinct failure modes, all present today:

1. **Five forced-color emoji** (🕘 History, ⚡ Reactive Mode and the embed badge, 🔒 the private-share denial, ⏳ the stale indicator, ⛔ the blocked status) render full-color next to monochrome text. This is the visible inconsistency that prompted the work.
2. **Nine emoji-capable glyphs** default to a text presentation but are free to render as color emoji, and some Windows and Android configurations do exactly that. This includes **▶ (U+25B6), the run button, at 14 uses** — the most prominent control in the app.
3. **Thirty-three non-emoji glyphs** are monochrome everywhere but come from DejaVu Sans, Segoe UI Symbol, or Apple Symbols depending on platform, so the check mark, chevrons, and status dots are simply different shapes per machine.

Choosing "plainer" codepoints cannot fix the third group, and the third group is the majority. Consistency requires shipping the geometry.

# Goals

1. Every UI affordance renders identically across platforms and browsers.
2. Icons inherit color and size from their context, so the dark theme and every font-size keep working with no per-call-site plumbing.
3. One definition of the SVG wrapper serving both the Leptos view path and the Export-HTML string path.
4. A guard that fails ci when a bare glyph is reintroduced.

# Technical Approach

`icondata_lu` supplies Lucide's icons as `&'static IconData` statics — plain data, no components, no runtime. A local `components/icon.rs` owns the rendering:

```
icondata_lu ──▶ icons.rs (role names) ──┬──▶ IconLabel / Icon   (view! call sites)
                                        └──▶ icon_svg_markup()  (export.rs String path)
```

Deliberately NOT taking `leptos_icons`: it is a thin wrapper whose Leptos-compat cadence would sit between ironpad and a future Leptos upgrade, and the component it replaces is about fifteen lines.

Sizing is `1em` square with `currentColor`, so an icon in a menu item, a status badge, and a toolbar button all size themselves from the surrounding text without a size prop. Accessibility follows the visible text: icons beside labels are `aria-hidden`, and icon-only controls are named by the button's existing `title` attribute — which is also why the Playwright suite mostly survives, since it already selects on those titles.

# Assumptions

- Lucide's stroke-based visual language is the intended direction (confirmed: reading more like modern SaaS and less like a terminal is acceptable).
- Release wasm keeps dropping unused statics, so the icon crate's size stays proportional to icons actually used (measured: ~250 bytes each, zero for the rest).

# Constraints

- Export HTML must remain a single self-contained file, so icons must inline as markup rather than reference a font or sprite sheet.
- SSR and hydration must produce identical markup; the icon data is static, so this holds as long as nothing derives icon choice from client-only state.
- `inner_html` carries the path data. It is a compile-time constant from the icon crate; no call site may pass user input through it.

# References to Code

- `crates/ironpad-app/src/components/` — new `icon.rs`, joins the existing component conventions (`copy_button.rs` is the closest shape).
- `crates/ironpad-app/src/pages/notebook_editor/cell_item.rs` — the status badge, the transport controls, the collapse toggles; the densest call site.
- `crates/ironpad-app/src/pages/notebook_editor/export.rs` — the second rendering path, string-based.
- `style/main.scss` — icon sizing plus removal of glyph-metric workarounds.
- `tests/e2e/shared-live-check.spec.ts`, `tests/e2e/editor-ux.spec.ts` — the two glyph-text selectors.

# Non-Goals (MVP)

- The `→` used in prose, comments, and one tracing log line: not UI.
- Restyling or re-laying-out anything. Icons land where glyphs were; visual polish beyond the swap is separate.
- Public-notebook prose that references `▶` (3 notebooks). The Lucide play icon still reads as "play"; revisit only if it looks wrong in context.
- An icon for the `⠿` drag handle, which is a deliberate braille-pattern texture rather than a pictogram — evaluate during T-004 and keep it if the Lucide equivalent reads worse.

# History

- **2026-08-07** — Implemented and closed. 95 call sites across 19 files migrated;
  `tools/glyph-check.py` (wired into `ci`) reports zero bare glyphs in rendered
  source and was negative-tested by reintroducing one. Three findings during
  implementation revised the plan: (1) `⋯` was listed as punctuation in the
  Non-Goals but is the **cell menu** button, so it became `icons::MORE`;
  (2) `▶▶` carried a distinct meaning from `▶` and earned its own `RUN_ALL`
  role rather than collapsing into `RUN`; (3) the `▸`/`▾` disclosure pair
  appeared at 8 sites as a hand-rolled conditional and became a `Chevron`
  component, since 8 copies of one policy is how disclosure behaviour drifts.
  The `✓` that survived in two menu labels ("Force Recompile ✓") became a
  trailing `icons::SUCCESS`, so no rendered check mark remains. The unsaved-
  source dot gained a `.ironpad-tab-dirty` hook, which is both the CSS seam
  and what the e2e selector now targets instead of glyph text.

- **2026-08-07** — Created. Scope set after measuring the glyph inventory (47 rendered across 118 uses, classified against the Unicode `emoji-data.txt` presentation property) and benchmarking the dependency (`icondata_lu`: 1.9s build, ~250 bytes per used icon, unused icons stripped entirely from release wasm). Aaron chose the full icon-set migration over a stopgap emoji swap, and accepted Lucide's SaaS-leaning stroke aesthetic.
