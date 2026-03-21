---
id: PRD-0019
title: "LiveView Trait — Tick-Driven Reactive Text/HTML/Markdown Output"
status: done
owner: "Aaron Roney"
created: 2026-03-16
updated: 2026-03-17

depends_on:
  - PRD-0018

principles:
  - "Mirror the Simulation trait pattern — same scaffold, same tick loop, same executor plumbing"
  - "Reuse existing DisplayPanel rendering (Html, Markdown, Text) — no new renderer"
  - "LiveView reads from the sim bus, making it reactive to other cells"
  - "Minimal FFI surface — one new result struct, one new JS method"

references:
  - name: "Simulation trait (pattern to follow)"
    url: "crates/ironpad-cell/src/lib.rs"
  - name: "Scaffold codegen"
    url: "crates/ironpad-app/src/compiler/scaffold.rs"
  - name: "JS executor (tick plumbing)"
    url: "public/executor.js"
  - name: "SimulationCanvas component (reference)"
    url: "crates/ironpad-app/src/components/animation_canvas.rs"

acceptance_tests:
  - id: uat-001
    name: "LiveView returning Html compiles and renders updated content each tick"
    command: cargo make uat
    uat_status: unverified
  - id: uat-002
    name: "LiveView returning Markdown renders with KaTeX math support"
    command: cargo make uat
    uat_status: unverified
  - id: uat-003
    name: "LiveView reads from sim bus (sim::read works inside tick)"
    command: cargo make uat
    uat_status: unverified
  - id: uat-004
    name: "LiveView works on public notebook page (view_only_notebook.rs)"
    command: cargo make uat
    uat_status: unverified
  - id: uat-005
    name: "LiveView works on notebook editor page (cell_output.rs)"
    command: cargo make uat
    uat_status: unverified
  - id: uat-006
    name: "Nuclear reactor dashboard LiveView cell displays live k_eff and flux data"
    command: cargo make uat
    uat_status: unverified
  - id: uat-007
    name: "cargo make ci passes (clippy + tests)"
    command: cargo make ci
    uat_status: verified

tasks:
  - id: T-001
    title: "Define LiveView trait, LiveContent enum, LiveViewMeta, LiveTickResult in ironpad-cell"
    priority: 1
    status: done
  - id: T-002
    title: "Add DisplayPanel::LiveView variant and From<LiveViewMeta> for CellOutput"
    priority: 1
    status: done
  - id: T-003
    title: "Add is_live_view() detection and generate_live_view_lib_rs() scaffold codegen"
    priority: 1
    status: done
  - id: T-004
    title: "Add LiveTickResult reading and tickLive() method in JS executors"
    priority: 1
    status: done
  - id: T-005
    title: "Create LiveViewPanel component"
    priority: 2
    status: done
  - id: T-006
    title: "Wire DisplayPanel::LiveView into cell_output.rs and view_only_notebook.rs"
    priority: 2
    status: done
  - id: T-007
    title: "Add nuclear reactor dashboard LiveView cell"
    priority: 3
    status: done
  - id: T-008
    title: "Unit tests for LiveView scaffold detection and codegen"
    priority: 2
    status: done
  - id: T-009
    title: "Integration test for LiveView compilation"
    priority: 3
    status: done
---

# Summary

Add a `LiveView` trait — a sibling to `Simulation` — whose `tick()` method returns
`LiveContent` (Text, Html, or Markdown) instead of a `Canvas`. This enables tick-driven
reactive text output that can read from the simulation bus, making it ideal for live
dashboards, instrumentation panels, and formatted data displays alongside simulations.

# Problem

Currently, the only way to get live-updating output is via the `Simulation` trait, which
returns pixel data (`Canvas`). There is no way to produce live-updating **text, HTML, or
Markdown** output. Users who want a reactive dashboard (e.g., displaying `k_eff` and flux
values from a nuclear reactor simulation) must either use a second `Simulation` that
renders text as pixels (wasteful, ugly) or accept static output.

# Goals

1. Define a `LiveView` trait with the same lifecycle as `Simulation` (init → tick loop)
   but returning `LiveContent` (Text | Html | Markdown) instead of `Canvas`.
2. Reuse the existing scaffold, executor, and animation infrastructure with minimal new code.
3. Enable LiveView cells to read from the simulation bus via `sim::read`.
4. Render LiveView output using existing DisplayPanel renderers (inner_html for Html,
   render_markdown for Markdown, pre for Text).
5. Support KaTeX math in Markdown LiveView output.

# Technical Approach

## Trait & Types (ironpad-cell)

```
LiveView trait                 LiveContent enum
┌──────────────────────┐       ┌──────────────────┐
│ init() -> Self       │       │ Text(String)     │
│ tick(&mut self)      │──────▶│ Html(String)     │
│   -> LiveContent     │       │ Markdown(String) │
│ fps() -> u32 [=10]  │       └──────────────────┘
└──────────────────────┘
```

`LiveViewMeta` carries `fps` + `initial_content: LiveContent` from the first tick.

`LiveTickResult` is a `#[repr(C)]` FFI struct: `{ kind: u32, content_ptr: *mut u8, content_len: usize }`.

## Scaffold Codegen

Mirrors `generate_simulation_lib_rs()`:

```rust
// cell_main: init, tick once, wrap in LiveViewMeta → CellOutput
// cell_tick: call tick, wrap in LiveTickResult, return pointer
static mut __IRONPAD_LIVE_VIEW__: Option<T> = None;
```

Detection: `is_live_view()` scans for `impl LiveView for <Name>`.

## Executor Plumbing (JS)

```
Browser Component
  ↓ tickLive(cellId)
BridgeExecutor
  ↓ postMessage { type: "tick_live", cellId }
Worker (executor-worker.js)
  ↓ executor.tickLive(cellId)
WorkerExecutor._readLiveTickResult()
  ↓ { kind: 0|1|2, content: "..." }
```

`tickLive()` calls the same `cell_tick` WASM export but reads `LiveTickResult`
(12 bytes) instead of `TickResult` (16 bytes). The bridge stores an `isLiveView`
flag per cell entry, set when `type_tag == "LiveView"` from initial execution.

## Frontend Component

`LiveViewPanel` component:
- Receives `fps`, `kind`, `content`, `cell_id`
- Renders initial content on mount
- Starts `requestAnimationFrame` loop at target fps
- Each frame: `tickLive(cell_id)` → update DOM
- For Markdown: re-render via `render_markdown()` + `IronpadKaTeX.renderMathIn()`
- Play/Pause/Step controls (same pattern as `SimulationCanvas`)

## DisplayPanel Integration

New variant: `DisplayPanel::LiveView { fps, kind, content }`.

Matched in both `cell_output.rs` and `view_only_notebook.rs` → renders `LiveViewPanel`.

# Assumptions

- The sim bus (PRD-0018) is complete and working.
- KaTeX rendering is available for Markdown output.
- The existing `cell_tick` export name can be reused (JS disambiguates via stored metadata).

# Constraints

- LiveView tick returns a string, not pixels — the FFI struct is different from TickResult.
- Markdown re-rendering + KaTeX on every tick at high fps could be expensive.
  Default fps is 10 to mitigate. Users can override.
- LiveView cells cannot receive piped input from previous cells (same as Simulation).

# References to Code

- **Simulation trait**: `crates/ironpad-cell/src/lib.rs:817-830`
- **TickResult FFI struct**: `crates/ironpad-cell/src/lib.rs:854-877`
- **SimulationMeta → CellOutput**: `crates/ironpad-cell/src/lib.rs:592-608`
- **Scaffold codegen**: `crates/ironpad-app/src/compiler/scaffold.rs:298-496`
- **JS tick plumbing**: `public/executor.js:467-576`, `public/executor-bridge.js:203-231`
- **SimulationCanvas component**: `crates/ironpad-app/src/components/animation_canvas.rs:327-573`
- **DisplayPanel match (editor)**: `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs:347-354`
- **DisplayPanel match (view-only)**: `crates/ironpad-app/src/components/view_only_notebook.rs:765-772`

# Non-Goals (MVP)

- Bidirectional interaction (LiveView writing to sim bus) — read-only for now
- CSS styling API for LiveView content — users write raw HTML/Markdown
- Streaming / partial updates — full content replacement each tick
- Server-side rendering of LiveView ticks — client-only

# History

(Entries appended during implementation go below this line.)

## 2026-03-17 — Batch Execution (T-001 through T-009)

- **Tasks completed**: T-001, T-002, T-003, T-004, T-005, T-006, T-007, T-008, T-009
- **Changes**:
  - T-001: Added `LiveView` trait, `LiveContent` enum, `LiveViewMeta`, `LiveTickResult` FFI struct, `From` impls, prelude exports, and 4 unit tests in `ironpad-cell/src/lib.rs`
  - T-002: Added `DisplayPanel::LiveView` variant, `From<LiveViewMeta> for CellOutput`, and match arms in cell_output.rs, view_only_notebook.rs, and export.rs
  - T-003: Added `is_live_view()` detection and `generate_live_view_lib_rs()` codegen in scaffold.rs; fixed `TickResult` substring collision
  - T-004: Added `LIVE_TICK_RESULT_SIZE`, `_readLiveTickResult`, `tickLive`/`_tickLiveBindgen`/`_tickLiveRaw` in executor.js, worker-executor.js, executor-bridge.js, executor-worker.js
  - T-005: Created `LiveViewPanel` component in `live_view_panel.rs` with rAF tick loop, play/pause/step, text/html/markdown rendering + KaTeX support
  - T-006: Wired `LiveViewPanel` into cell_output.rs and view_only_notebook.rs, added `tick_live` JS binding and `tick_live_cell()` to executor.rs
  - T-007: Added `cell_dashboard` LiveView cell to nuclear-reactor.ironpad — HTML control-room panel with k_eff gauge, status indicator, and flux bar
  - T-008: Added 8 unit tests for LiveView scaffold detection and codegen
  - T-009: Added `pipeline_live_view_compiles` integration test in compiler/mod.rs
- **Test results**: `cargo make ci` passes — 349 tests, 0 failures, clippy clean
- **UATs verified**: uat-007 (cargo make ci passes)
- **UATs deferred**: uat-001 through uat-006 require manual browser testing or Playwright e2e tests
- **Constitution compliance**: No violations
