---
id: PRD-0037
title: "Leak sweep — timers, closures, and listeners (long-session degradation)"
status: draft
owner: "Aaron Roney"
created: 2026-07-02
updated: 2026-07-02

depends_on:
- PRD-0032
- PRD-0034

principles:
- "Every resource created in a component is released in on_cleanup"
- "No .forget() without a matching cleanup that drops the closure"
- "A heavily-navigated session must stay leak-free"

references:
- name: "Review report — P3 Leaks"
  url: reviews/2026-07-02-codebase-review.md

acceptance_tests:
- id: uat-001
  name: "Navigating home <-> notebook 20x does not accumulate intervals/listeners (no growth)"
  command: cargo make playwright
  uat_status: unverified
- id: uat-002
  name: "cargo make ci stays green after cleanup changes (no behavioral regressions)"
  command: cargo make ci
  uat_status: unverified

tasks:
- id: T-001
  title: "StatusBar 30s interval: clear on unmount"
  priority: 1
  status: todo
  notes: "app_layout.rs:340-354: the tick interval is registered with set_interval + closure.forget(), no id stored, no clear_interval, no on_cleanup. StatusBar is conditionally rendered (show_footer), so it remounts on navigation -> each notebook visit orphans another interval ticking a disposed signal. Fix: capture the interval id, clear_interval_with_handle(id) + drop the closure in on_cleanup (store in StoredValue/Rc, not forget())."
- id: T-002
  title: "Monaco on_change closure: drop on dispose (and fix the misleading comment)"
  priority: 1
  status: todo
  notes: "monaco_editor.rs:243-253 (esp. 252): closure.forget() leaks the on_change closure for page lifetime; the comment claims dispose frees it but js::dispose(id) (274-278) frees only the JS editor. Fix: store the Closure in a StoredValue/Rc captured by on_cleanup and drop it there alongside js::dispose(id); correct the comment."
- id: T-003
  title: "Global keydown + outside-click listeners: remove on cleanup"
  priority: 1
  status: todo
  notes: "mod.rs:113-182 adds a document keydown listener with .forget() and no cleanup -> each notebook mount adds another; after Close->reopen, stale listeners fire Ctrl+S/Ctrl+Shift+N against a disposed scope. The outside-click listener (mod.rs:336) has the same issue (its churn is dominated by the E1 rebuild — land after PRD-0032 T-001). Fix: keep the handles and remove_event_listener in on_cleanup."
- id: T-004
  title: "Markdown Escape-keybinding closure: drop on cleanup"
  priority: 2
  status: todo
  notes: "markdown_cell.rs:87-98: the Effect tracking editing/editor_handle creates a fresh Closure and cb.forget()s it on each entry into edit mode -> one leaked per edit. Fix: store/drop via on_cleanup (or reuse a single stored closure)."
- id: T-005
  title: "requestAnimationFrame closures: break the Rc cycle and stop the loop when paused"
  priority: 2
  status: todo
  notes: "animation_canvas.rs:231-271,389-447 & live_view_panel.rs:134-195: cb: Rc<RefCell<Option<Closure>>> holds a closure that captures the same Rc -> reference cycle; on_cleanup cancels the pending frame but never sets *cb.borrow_mut()=None, so the closure leaks per instance. Also the loop keeps calling requestAnimationFrame at ~60fps while paused (skipping the draw). Fix: clear the stored closure in on_cleanup; stop rescheduling on pause, restart on resume."
- id: T-006
  title: "CopyButton timeout closure: clear on unmount and on re-click"
  priority: 2
  status: todo
  notes: "copy_button.rs:29-37: reset.forget() leaks the closure and the timeout handle is discarded (no clear_timeout); unmount within 1.5s calls copied.set(false) on a disposed signal, and rapid double-clicks stack timers (first clears 'Copied!' early). Fix: store the handle, clear it in on_cleanup and reset the prior timer on re-click."
- id: T-007
  title: "Cell debounce timers + editor_handles: cancel/prune on cell disposal"
  priority: 2
  status: todo
  notes: "cell_item.rs:993-1084: source/cargo debounce closures are .forget()ed and their setTimeout handles cleared only on the next keystroke, never in on_cleanup -> on delete a pending timer fires save_fn against disposed signals / a gone cell. cell_item.rs:765-767: editor_handles inserted but never removed on delete -> handles for deleted cells accumulate. Fix: clear both timer handles and remove the cell's editor_handle in on_cleanup / the delete cleanup."
---

# Summary

The app is a long-lived SPA, and several components create timers, closures, and document listeners with `.forget()` and no `on_cleanup`, so a heavily-navigated session accumulates orphaned resources that tick/fire against disposed signals. This epic is a mechanical sweep to release every such resource on unmount. It lands after PRD-0032 and PRD-0034 because it touches the same components (avoiding rebase churn) — and because fixing the E1 rebuild first removes the pathological per-mutation listener re-registration that dominates some of these leaks.

# Problem

`.forget()` intentionally leaks a `Closure` for the page lifetime; that's only correct for truly page-lifetime handlers. Here it's used for per-component timers and listeners (StatusBar interval, Monaco on_change, rAF loops, CopyButton timeout, cell debounces, markdown keybinding, global keydown/outside-click) that should die with the component. The result is unbounded growth of intervals/listeners and callbacks firing on disposed scopes across a navigation-heavy session.

# Goals

1. Every timer, listener, and closure created in a component is released in `on_cleanup`.
2. Navigating repeatedly between home and notebooks does not accumulate resources.
3. Animation/live loops stop scheduling frames while paused.

# Technical Approach

Mechanical, per the task notes: replace `.forget()` + discarded handles with `StoredValue`/`Rc`-held closures and matching `clear_interval_with_handle` / `clear_timeout` / `remove_event_listener` in `on_cleanup`. T-001-T-003 are the highest-impact (StatusBar interval, Monaco closure, global listeners). Verify with a navigation-loop Playwright check that asserts interval/listener counts don't grow.

# Assumptions

- PRD-0032 T-001 (memo load gate) has landed, so the outside-click/Sortable re-registration is no longer per-mutation.
- `on_cleanup` runs on component disposal in this Leptos version (it does).

# Constraints

- Cleanup must not change behavior while mounted — only release on unmount.
- No public API changes; internal storage of handles only.

# References to Code

- `crates/ironpad-app/src/components/{app_layout.rs,monaco_editor.rs,markdown_cell.rs,animation_canvas.rs,live_view_panel.rs,copy_button.rs}`
- `crates/ironpad-app/src/pages/notebook_editor/{mod.rs,cell_item.rs}`

# Non-Goals (MVP)

- A general leak-detection harness or automated resource-accounting (a targeted navigation-loop test is enough).

# History

(Entries appended during implementation go below this line.)
