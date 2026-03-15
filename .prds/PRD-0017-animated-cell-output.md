---
id: PRD-0017
title: "Animated Cell Output: Frame Sequences and Live Simulations"
status: active
owner: "Aaron Roney"
created: 2025-03-15
updated: 2025-07-15

principles:
- "Two modes: precomputed frame sequences (Animation) and live tick-loop simulations (Simulation)"
- "Animation is purely a display concern — full frame data is precomputed, no execution model changes"
- "Simulation introduces a persistent WASM execution mode with cell_tick export"
- "Both render to a <canvas> element with play/pause controls"
- "Downstream cells receive data from cell_main only — tick frames are purely visual"
- "Web Worker is the primary execution target for Simulation ticks; main-thread fallback still applies"

references:
- name: "requestAnimationFrame MDN"
  url: https://developer.mozilla.org/en-US/docs/Web/API/Window/requestAnimationFrame
- name: "wasm-bindgen exports"
  url: https://rustwasm.github.io/docs/wasm-bindgen/reference/attributes/on-rust-exports/

acceptance_tests:
- id: uat-001
  name: "Animation type renders a multi-frame looping animation in the browser"
  command: cargo make uat
  uat_status: unverified
- id: uat-002
  name: "Animation panel has play/pause and frame counter controls"
  command: cargo make uat
  uat_status: unverified
- id: uat-003
  name: "Simulation trait cells render with a live animation driven by cell_tick"
  command: cargo make uat
  uat_status: unverified
- id: uat-004
  name: "Simulation panel has play/pause/step controls"
  command: cargo make uat
  uat_status: unverified
- id: uat-005
  name: "Wave equation notebook animates correctly using Animation type"
  command: cargo make uat
  uat_status: unverified
- id: uat-006
  name: "Double pendulum notebook animates correctly using Simulation trait"
  command: cargo make uat
  uat_status: unverified
- id: uat-007
  name: "Simulation cells work in Web Worker (tick messages processed correctly)"
  command: cargo make uat
  uat_status: unverified
- id: uat-008
  name: "Unit tests pass for Animation and Simulation types in ironpad-cell"
  command: cargo make test
  uat_status: verified

tasks:
# ── Phase 1: Animation (precomputed frame sequences) ──
- id: T-001
  title: "Add Animation type to ironpad-cell"
  priority: 1
  status: done
  notes: >
    Create `Animation` struct holding `Vec<Canvas>` + `fps: u32`.
    Implement `From<Animation> for CellOutput` and `IntoPanels for Animation`.
    Add a new `DisplayPanel::Animation { width, height, fps, frame_count, data }` variant
    where `data` is base64-encoded concatenated RGB bytes for all frames.
    Add `TypeTag` impl. Add unit tests for serialization round-trip and panel generation.
    File: crates/ironpad-cell/src/canvas.rs (or new animation.rs), crates/ironpad-cell/src/lib.rs.

- id: T-002
  title: "Frontend rendering for DisplayPanel::Animation"
  priority: 1
  status: done
  notes: >
    Add `Animation` variant handling in cell_output.rs and view_only_notebook.rs.
    Decode base64 RGB data, create a `<canvas>` element, use requestAnimationFrame
    to cycle through frames at the specified fps. Add play/pause toggle button and
    a frame counter (e.g., "Frame 23/100"). The animation should auto-play and loop.
    CSS: style the canvas container and controls consistent with existing output panels.
    Files: crates/ironpad-app/src/pages/notebook_editor/cell_output.rs,
    crates/ironpad-app/src/components/view_only_notebook.rs, style/main.scss.

- id: T-003
  title: "Add DisplayPanel::Animation to export/deserialization"
  priority: 1
  status: done
  notes: >
    Update the DisplayPanel enum in the frontend (export.rs or wherever it's deserialized)
    to include the Animation variant. Ensure JSON deserialization handles it correctly.
    Files: crates/ironpad-app/src/pages/notebook_editor/export.rs,
    crates/ironpad-common/src/types.rs (if DisplayPanel is defined there).

- id: T-004
  title: "Create animated wave equation notebook"
  priority: 2
  status: done
  notes: >
    Convert the existing wave-equation.ironpad to use the Animation type.
    The simulation cell should precompute ~200 frames of the 1D wave equation
    evolution and return `Animation::new(frames, 30)`. Each frame renders the
    wave state as a Canvas (e.g., 600x300, blue wave on dark background).
    File: public/notebooks/wave-equation.ironpad.

# ── Phase 2: Simulation (persistent tick loop) ──
- id: T-005
  title: "Define Simulation trait in ironpad-cell"
  priority: 1
  status: done
  notes: >
    Add a `Simulation` trait:
    ```
    pub trait Simulation: Sized + 'static {
        fn init() -> Self;
        fn tick(&mut self) -> Canvas;
        fn fps() -> u32 { 30 }
    }
    ```
    Add `SimulationMeta` struct for the cell_main return (width, height, fps, first frame).
    Implement `From<SimulationMeta> for CellOutput` with a new `DisplayPanel::Simulation`
    variant carrying the metadata and first frame.
    Add a `simulation_result!()` macro or helper to bridge cell_main → SimulationMeta.
    File: crates/ironpad-cell/src/lib.rs (or new simulation.rs).

- id: T-006
  title: "Scaffold: detect Simulation trait and generate cell_tick export"
  priority: 1
  status: done
  notes: >
    In scaffold.rs `generate_lib_rs()`, detect when user source contains
    `impl Simulation for`. When detected, generate two WASM exports:
    (1) `cell_main`: calls `T::init()`, stores state in `static mut`, returns
    SimulationMeta (first frame + fps + dimensions).
    (2) `cell_tick`: retrieves state from static, calls `tick(&mut state)`,
    returns raw Canvas RGB bytes + dimensions as a CellResult-like struct.
    The struct name is parsed from `impl Simulation for <Name>`.
    Update preamble line count calculation for diagnostics.
    File: crates/ironpad-app/src/compiler/scaffold.rs.

- id: T-007
  title: "Executor: add tick message protocol"
  priority: 1
  status: done
  notes: >
    Add a `"tick"` message type to executor-bridge.js and executor-worker.js.
    Bridge gets a new method: `tick(cellId) -> Promise<{ width, height, rgbBytes }>`.
    Worker handler calls the WASM `cell_tick` export on the already-loaded module,
    reads the returned frame data, and transfers the ArrayBuffer back to the bridge.
    The module stays loaded between ticks (already the case — modules persist in
    the `this.modules` Map until `unload()` is called).
    Files: public/executor-bridge.js, public/executor-worker.js, public/worker-executor.js.

- id: T-008
  title: "Executor Rust bindings for tick"
  priority: 1
  status: done
  notes: >
    Add `tick_cell(cell_id: &str) -> Result<TickResult>` to the Rust executor
    bindings. TickResult contains width, height, and RGB bytes.
    File: crates/ironpad-app/src/components/executor.rs.

- id: T-009
  title: "Frontend rendering for DisplayPanel::Simulation"
  priority: 1
  status: done
  notes: >
    When a cell returns DisplayPanel::Simulation, render a `<canvas>` element
    with a requestAnimationFrame loop that calls `tick_cell()` each frame.
    Controls: play/pause button, step button, frame counter, fps display.
    On play: start rAF loop, call tick, draw returned RGB data to canvas.
    On pause: stop rAF loop. On step: single tick + draw.
    Auto-play on first render. Stop the loop when the cell is re-executed or
    the component unmounts. Handle both notebook editor and view-only pages.
    Files: crates/ironpad-app/src/pages/notebook_editor/cell_output.rs,
    crates/ironpad-app/src/components/view_only_notebook.rs, style/main.scss.

- id: T-010
  title: "Create live double pendulum notebook"
  priority: 2
  status: done
  notes: >
    Create or convert double-pendulum.ironpad to use the Simulation trait.
    The struct holds angles, angular velocities, and a trail buffer.
    `init()` sets initial conditions (θ1=π/2, θ2=π/2).
    `tick()` integrates equations of motion (RK4), appends to trail buffer,
    renders pendulum arms + bob positions + fading trail on a 450x450 Canvas.
    `fps() -> 60`. Should produce a mesmerizing chaotic animation.
    File: public/notebooks/double-pendulum.ironpad.

# ── Phase 3: Polish ──
- id: T-011
  title: "Simulation main-thread fallback support"
  priority: 2
  status: done
  notes: >
    Ensure the main-thread fallback path (executor-bridge.js) also supports
    tick messages. If the initial cell_main fell back to main thread, ticks
    should also run on the main-thread executor. Test with a plotters-dependent
    simulation (if applicable).
    File: public/executor-bridge.js.

- id: T-012
  title: "Cleanup: stop simulation on cell re-execution or unmount"
  priority: 2
  status: done
  notes: >
    Ensure the rAF loop is cancelled when the cell is re-executed, the notebook
    is closed, or the component unmounts. Prevent multiple simultaneous loops.
    Use Leptos on_cleanup or equivalent lifecycle hook.
    Files: crates/ironpad-app/src/pages/notebook_editor/cell_output.rs,
    crates/ironpad-app/src/components/view_only_notebook.rs.
---

# Summary

Add two new animated output modes to ironpad cells:

1. **Animation** (precomputed): A cell returns an `Animation` value containing a `Vec<Canvas>` of frames and an fps. The frontend renders them as a looping animation on a `<canvas>` element with play/pause controls. No changes to the execution model — the animation is fully precomputed during `cell_main`.

2. **Simulation** (live tick loop): A cell implements a `Simulation` trait with `init()` and `tick(&mut self) -> Canvas`. The scaffold generates two WASM exports: `cell_main` (initialization) and `cell_tick` (per-frame update). The WASM module stays loaded in the Web Worker, and the frontend drives a `requestAnimationFrame` loop that calls `cell_tick` each frame, drawing the returned pixels to a `<canvas>`.

---

# Problem

Currently, all cell outputs are static — text, HTML, SVG, or a single Canvas image. Notebooks that simulate dynamic systems (wave equations, physics, cellular automata) can only show snapshots or filmstrips, losing the temporal dimension that makes these simulations compelling. Users cannot see evolution over time, which limits ironpad's value as an interactive computational notebook.

---

# Goals

1. Enable cells to produce animated output (precomputed frame sequences) with a simple `Animation` API
2. Enable cells to run persistent simulations via a `Simulation` trait with per-frame tick execution
3. Provide consistent playback controls (play/pause, frame counter) for both modes
4. Ship at least one example notebook for each mode demonstrating the capability
5. Maintain Web Worker execution for simulation ticks to avoid blocking the UI thread

---

# Technical Approach

## Feature A: Animation (Precomputed Frames)

### ironpad-cell

New `Animation` struct in `canvas.rs`:

```rust
pub struct Animation {
    frames: Vec<Canvas>,
    fps: u32,
}

impl Animation {
    pub fn new(frames: Vec<Canvas>, fps: u32) -> Self { ... }
}
```

New `DisplayPanel::Animation` variant:

```rust
Animation {
    width: u32,
    height: u32,
    fps: u32,
    frame_count: u32,
    data: String,  // base64 of concatenated RGB bytes (frame0 ++ frame1 ++ ...)
}
```

The `IntoPanels` impl encodes all frames as a single base64 blob (width × height × 3 bytes per frame, concatenated). This avoids per-frame BMP overhead and allows efficient client-side rendering.

### Frontend

The `Animation` panel renders a `<canvas>` element. JavaScript decodes the base64 data, splits it into frame-sized chunks, and uses `requestAnimationFrame` to draw frames at the target fps using `putImageData()`. Controls overlay: ▶/⏸ toggle, frame counter.

```
┌─────────────────────────────┐
│         <canvas>            │
│    (animation frames)       │
├─────────────────────────────┤
│  ▶/⏸  │  Frame 23/100      │
└─────────────────────────────┘
```

## Feature B: Simulation (Persistent Tick Loop)

### ironpad-cell

New `Simulation` trait:

```rust
pub trait Simulation: Sized + 'static {
    fn init() -> Self;
    fn tick(&mut self) -> Canvas;
    fn fps() -> u32 { 30 }
}
```

### Scaffold

When `generate_lib_rs()` detects `impl Simulation for` in the source, it generates:

```rust
// --- Generated by scaffold (simulation mode) ---
use ironpad_cell::prelude::*;

// <user code: struct + impl Simulation>

static mut __IRONPAD_SIM__: Option<UserStruct> = None;

#[wasm_bindgen]
pub fn cell_main(_input_ptr: u32, _input_len: u32) -> u32 {
    console_error_panic_hook::set_once();
    let mut sim = UserStruct::init();
    let first_frame = sim.tick();
    let meta = SimulationMeta {
        width: first_frame.width(),
        height: first_frame.height(),
        fps: UserStruct::fps(),
        first_frame,
    };
    unsafe { __IRONPAD_SIM__ = Some(sim); }
    let output: CellOutput = meta.into();
    let result: CellResult = output.into();
    Box::into_raw(Box::new(result)) as u32
}

#[wasm_bindgen]
pub fn cell_tick() -> u32 {
    let sim = unsafe { __IRONPAD_SIM__.as_mut().unwrap() };
    let frame = sim.tick();
    // Return frame as raw RGB + dimensions
    let result = TickResult::from(frame);
    Box::into_raw(Box::new(result)) as u32
}
```

### Executor Protocol

New `"tick"` message type:

```
Main Thread                    Worker
     │                            │
     │──── { type: "tick",  ─────>│  calls cell_tick() on loaded module
     │       cellId }             │
     │                            │
     │<─── { type: "result", ─────│  returns { width, height, rgbBytes }
     │       value: frame }       │  (rgbBytes transferred, zero-copy)
     │                            │
```

### Frontend Rendering

The `Simulation` panel creates a `<canvas>`, starts a `requestAnimationFrame` loop, and calls `tick_cell()` each frame. The returned RGB bytes are written to an `ImageData` and drawn with `putImageData()`.

```
┌─────────────────────────────┐
│         <canvas>            │
│    (live simulation)        │
├─────────────────────────────┤
│  ▶/⏸  │ ⏭ Step │ Frame 142 │
└─────────────────────────────┘
```

---

# Assumptions

- Canvas RGB byte format (3 bytes/pixel, row-major) is stable and won't change.
- Web Workers support keeping WASM modules loaded indefinitely (they do — modules persist in the `CellExecutor.modules` Map).
- `requestAnimationFrame` provides sufficient timing precision for simulation fps targets.
- A single `unsafe static mut` for simulation state is acceptable since WASM is single-threaded.

---

# Constraints

- **WASM single-threaded**: Each cell's WASM instance runs single-threaded, so `tick()` must complete within one frame budget (16ms at 60fps). Heavy simulations may drop frames.
- **Memory**: Animation stores all frames in memory. For 200 frames at 600×450×3 bytes, that's ~162 MB. May need to document reasonable limits.
- **Serialization**: Simulation state lives in WASM memory only — not serialized for downstream cells. Downstream cells receive the `cell_main` output (metadata), not tick frames.
- **No shared state across cells**: Each simulation cell is independent. Two simulation cells cannot share state.

---

# References to Code

- `crates/ironpad-cell/src/canvas.rs` — Canvas struct, `to_html()`, BMP encoding
- `crates/ironpad-cell/src/lib.rs` — DisplayPanel enum (line ~214), IntoPanels trait, CellOutput, CellResult
- `crates/ironpad-app/src/compiler/scaffold.rs` — `generate_lib_rs()` (line ~310), `cell_main` wrapping
- `public/executor-bridge.js` — BridgeExecutor, message protocol, main-thread fallback
- `public/executor-worker.js` — Worker message handler, CellExecutor module Map
- `public/worker-executor.js` — `CellExecutor.loadBlob()`, `execute()`, module lifecycle
- `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs` — DisplayPanel rendering (line ~212)
- `crates/ironpad-app/src/components/view_only_notebook.rs` — View-only output rendering
- `crates/ironpad-app/src/components/executor.rs` — Rust↔JS executor bindings

---

# Non-Goals (MVP)

- Interactive simulations (user input affecting tick state, e.g., mouse clicks)
- Simulation speed controls (0.5×, 2×, etc.) — just play/pause/step for now
- Recording/exporting animations as GIF or video
- Simulation state serialization for downstream cells
- Multi-cell synchronized animations
- GPU-accelerated rendering (WebGL/WebGPU shader programs)

---

# History

(Entries appended during implementation go below this line.)

## 2025-07-15 — Batch Execution (T-001 through T-012)
- **Tasks completed**: T-001, T-002, T-003, T-004, T-005, T-006, T-007, T-008, T-009, T-010, T-011, T-012
- **Changes**:
  - T-001: Added `Animation` struct (`Vec<Canvas>` + fps) to `ironpad-cell/src/canvas.rs`, `DisplayPanel::Animation` variant, `From<Animation>`, `IntoPanels`, `TypeTag` impls, unit tests
  - T-002: `AnimationCanvas` component in new `animation_canvas.rs` — decodes base64 RGB, expands to RGBA, drives rAF loop at target fps, play/pause + frame counter
  - T-003: Added `Animation` and `Simulation` variants to duplicated `DisplayPanel` enums in `export.rs` and `view_only_notebook.rs`
  - T-004: Converted `wave-equation.ironpad` from plotters SVG to Canvas-based Animation (200 frames, 600×300, 30fps, dual Gaussian pulses)
  - T-005: Added `Simulation` trait, `SimulationMeta`, `TickResult` FFI struct, `DisplayPanel::Simulation` variant, `From<SimulationMeta>` impl
  - T-006: Scaffold `is_simulation()` detection + `generate_simulation_lib_rs()` producing `cell_main` + `cell_tick` WASM exports with `static mut` state storage; 8 unit tests
  - T-007: JS tick protocol — `CellExecutor.tick()` in worker-executor.js, `"tick"` message handler in executor-worker.js, `BridgeExecutor.tick()` in executor-bridge.js
  - T-008: Rust `tick_cell()` binding + `TickResult` struct in `executor.rs`
  - T-009: `SimulationCanvas` component — draws first frame, drives rAF loop calling `tick_cell()`, play/pause/step + frame counter + fps display
  - T-010: Created `double-pendulum.ironpad` — `Simulation` trait impl with RK4 integration, 450×450 canvas, fading trail, 60fps
  - T-011: Main-thread fallback for tick in executor-bridge.js (reuses existing fallback pattern)
  - T-012: `on_cleanup` cancellation of rAF loops, `tick_in_flight` guard for simulation
- **Test results**: 327 passed, 3 skipped — all green
- **UATs verified**: uat-008 (unit tests pass)
- **UATs deferred**: uat-001 through uat-007 require browser testing (manual or Playwright)
- **Constitution compliance**: No violations

---
