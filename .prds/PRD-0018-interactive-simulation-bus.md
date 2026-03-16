---
id: PRD-0018
title: "Interactive Simulation Bus: emit/read IPC + Slider Integration"
status: draft
owner: "Aaron Roney"
created: 2026-03-16
updated: 2026-03-16

depends_on:
- PRD-0017

principles:
- "One unified key-value bus: simulations, widgets, and regular cells all use the same emit/read API"
- "Sliders are just emitters — no special 'SimSlider' type; executor auto-emits slider values to the bus"
- "Inter-simulation coupling falls out for free: sim A emits, sim B reads"
- "JSON serialization via serde_json for bus values — human-debuggable, JS-inspectable"
- "Ring buffer per key (1000 entries) enables downstream time-series plotting"
- "read returns Option — callers use .unwrap_or(default) for graceful fallback when no value emitted yet"
- "Zero breaking changes to existing Simulation trait — bus is opt-in via sim:: module functions"

references:
- name: "ironpad-cell host_message FFI"
  url: "crates/ironpad-cell/src/lib.rs:14-48"
- name: "Executor host message dispatch"
  url: "public/executor.js:50-86"
- name: "Web Worker host message forwarding"
  url: "public/executor-worker.js:30-55"
- name: "Executor bridge host message dispatch"
  url: "public/executor-bridge.js:254-277"
- name: "Existing slider widget"
  url: "crates/ironpad-cell/src/ui.rs:32-119"
- name: "Scaffold simulation codegen"
  url: "crates/ironpad-app/src/compiler/scaffold.rs:443-495"
- name: "SimulationCanvas tick loop"
  url: "crates/ironpad-app/src/components/animation_canvas.rs:288-497"

acceptance_tests:
- id: uat-001
  name: "sim::emit sends a JSON value to the JS bus and sim::read retrieves it within the same simulation"
  command: cargo make uat
  uat_status: unverified
- id: uat-002
  name: "sim::read returns None when no value has been emitted for a key"
  command: cargo make uat
  uat_status: unverified
- id: uat-003
  name: "sim::read_all returns the ring buffer history (oldest-first) for a key"
  command: cargo make uat
  uat_status: unverified
- id: uat-004
  name: "Bus works identically in Web Worker and main-thread fallback execution paths"
  command: cargo make uat
  uat_status: unverified
- id: uat-005
  name: "Unit tests pass for sim module in ironpad-cell (emit/read/read_all with non-wasm stubs)"
  command: cargo make test
  uat_status: unverified
- id: uat-006
  name: "SimSlider widget renders in simulation output and auto-emits to bus on change"
  command: cargo make uat
  uat_status: unverified
- id: uat-007
  name: "Simulation tick() can read slider values via sim::read with correct defaults"
  command: cargo make uat
  uat_status: unverified
- id: uat-008
  name: "Nuclear reactor notebook uses SimSliders for rod depth, rod absorption, fission, and fuel absorption"
  command: cargo make uat
  uat_status: unverified
- id: uat-009
  name: "Downstream non-simulation cell can use sim::read_all to retrieve time-series data"
  command: cargo make uat
  uat_status: unverified

tasks:
# ── Phase 1: Core Bus (emit + read + read_all) ──
- id: T-001
  title: "Add sim module to ironpad-cell with emit/read/read_all API"
  priority: 1
  status: todo
  notes: "New module crates/ironpad-cell/src/sim.rs. emit<T: Serialize>(key, &value) calls host_message_json with type 'sim_emit'. read<T: DeserializeOwned>(key) calls new FFI import ironpad_sim_read(key_ptr, key_len) -> ptr (returns JSON bytes or null). read_all<T>(key) calls ironpad_sim_read_all. Non-wasm32 stubs return None/empty. Re-export sim in prelude."

- id: T-002
  title: "Add sim bus store + ironpad_sim_read to executor.js"
  priority: 1
  status: todo
  notes: "Add _simBus Map<string, {latest: string, ring: string[]}> to CellExecutor. Register 'sim_emit' host message handler that pushes to ring buffer (cap 1000) and updates latest. Provide ironpad_sim_read(ptr, len) and ironpad_sim_read_all(ptr, len) functions in env imports during loadBlob. These read the key from WASM memory, look up the bus, and write JSON result back via ironpad_alloc. Return pointer to allocated JSON bytes (0 if no value)."

- id: T-003
  title: "Wire ironpad_sim_read imports in executor-worker.js and executor-bridge.js"
  priority: 1
  status: todo
  notes: "Depends on T-002. The worker already forwards host messages to the bridge. For sim_read, the bus state lives in the Worker (since WASM runs there). Wire ironpad_sim_read and ironpad_sim_read_all into the worker's env imports the same way ironpad_host_message is wired. The bridge needs to forward sim_emit messages from Worker to main thread so the bridge-side bus stays in sync (for main-thread fallback cells that might read). Also wire for main-thread fallback executor."

- id: T-004
  title: "Update scaffold codegen to declare ironpad_sim_read and ironpad_sim_read_all imports"
  priority: 1
  status: todo
  notes: "In generate_simulation_lib_rs and generate_lib_rs, the generated code already gets ironpad_host_message via extern C. Add ironpad_sim_read(key_ptr: *const u8, key_len: u32) -> u32 and ironpad_sim_read_all(key_ptr: *const u8, key_len: u32) -> u32 to the extern C block in ironpad-cell/src/sim.rs (same pattern as ironpad_host_message in lib.rs). The scaffold does NOT need changes — the imports come from the ironpad-cell crate itself."

- id: T-005
  title: "Add unit tests for sim module (non-wasm stubs)"
  priority: 2
  status: todo
  notes: "Test that emit/read/read_all compile and work on native (non-wasm) target. emit should be a no-op. read should return None. read_all should return empty Vec. Test serialization edge cases. Tests go in crates/ironpad-cell/src/sim.rs."

# ── Phase 2: SimSlider Widget + Auto-Emission ──
- id: T-006
  title: "Add SimSlider widget to ironpad-cell ui module"
  priority: 2
  status: todo
  notes: "New widget type in ui.rs: SimSlider. Builder API: sim_slider(key, min, max).step(s).label(l).default_value(v). Produces DisplayPanel::Interactive { kind: 'sim_slider', config: json }. The config includes the bus key. SimSlider is NOT a CellOutput — it's returned from Simulation::init() metadata (see T-007). Serializes default value via serde."

- id: T-007
  title: "Extend SimulationMeta to carry slider declarations"
  priority: 2
  status: todo
  notes: "Add sliders: Vec<SimSliderMeta> to SimulationMeta. SimSliderMeta has key, min, max, step, label, default. The scaffold's cell_main already serializes SimulationMeta — just add the field. Simulation trait gets optional fn sliders() -> Vec<SimSliderMeta> { vec![] }. Generated cell_main calls sliders() and includes in meta."

- id: T-008
  title: "Render SimSliders in SimulationCanvas component"
  priority: 2
  status: todo
  notes: "When SimulationMeta contains sliders, render HTML range inputs alongside the canvas. On input change, call executor JS to push value into the sim bus (sim_emit host message from JS side, or a dedicated bridge call). Each slider shows label + current value. Sliders are rendered below the canvas controls."

- id: T-009
  title: "Auto-emit slider values into bus from executor JS"
  priority: 2
  status: todo
  notes: "When bridge/executor receives a sim_slider render request, it emits the slider's current value to the bus on every change event AND on initialization (with default). The bus key matches the SimSlider's declared key. This means sim::read(key) in tick() gets the latest slider value without any special wiring."

- id: T-010
  title: "Update nuclear reactor notebook to use SimSliders"
  priority: 3
  status: todo
  notes: "Add 4 sliders via Simulation::sliders(): rod_depth (0.0-1.0, default 0.50), rod_absorption (0.1-10.0, default 3.0), fission_xs (0.01-0.30, default 0.095), fuel_absorption (0.01-0.20, default 0.082). In tick(), read each via sim::read::<f64>(key).unwrap_or(default). Remove the hardcoded auto-oscillation phase logic (slider replaces it). Keep current physics engine unchanged."

# ── Phase 3: Downstream Consumption ──
- id: T-011
  title: "Enable sim::read and sim::read_all in non-simulation cells"
  priority: 3
  status: todo
  notes: "Regular (non-simulation) cells should also be able to call sim::read_all to retrieve time-series data emitted by upstream simulations. The ironpad_sim_read imports need to be wired in generate_lib_rs (not just generate_simulation_lib_rs). Verify the bus persists across cell executions within the same notebook session."

- id: T-012
  title: "Add integration test: slider-driven simulation + downstream read_all"
  priority: 3
  status: todo
  notes: "E2E or integration test that loads a simulation with a SimSlider, changes the slider value, verifies tick() receives the new value, and verifies a downstream cell can read_all the history. May need a Playwright test or a simulated executor test."

---

# Summary

Add an interactive simulation bus to ironpad: a shared key-value store backed by the JS executor that enables bidirectional communication between simulations, widgets (sliders), and regular cells. Simulations emit named values via `sim::emit` and read them via `sim::read`/`sim::read_all`. Sliders auto-emit to the bus, so `tick()` can consume slider values with zero special wiring. Inter-simulation coupling and downstream time-series consumption fall out for free.

---

# Problem

Currently, simulation `tick()` functions are pure — they receive no input and can only produce a canvas frame. This means:

1. **No interactive parameters**: Users can't adjust simulation constants (e.g., control rod depth, fission cross-section) without editing source code and recompiling.
2. **No inter-simulation coupling**: Two simulations in the same notebook can't exchange data.
3. **No time-series export**: Downstream cells can't access the values a simulation computes each frame (e.g., k_eff history for plotting).

The nuclear reactor notebook is the motivating use case: the user wants sliders for rod depth, rod absorption, fission cross-section, and fuel absorption — all consumable by `tick()` in real time.

---

# Goals

1. Provide `sim::emit<T>(key, &value)` and `sim::read::<T>(key)` APIs in ironpad-cell for named value IPC
2. Maintain a per-notebook key-value bus with ring buffer history (1000 entries per key) in the JS executor
3. Add a `SimSlider` widget that auto-emits to the bus, rendered alongside the simulation canvas
4. Update the nuclear reactor notebook to use 4 interactive sliders
5. Enable downstream (non-simulation) cells to read bus values for plotting / analysis

---

# Technical Approach

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        JS Executor (Bus Store)                  │
│  _simBus: Map<string, { latest: JSON, ring: JSON[1000] }>      │
│                                                                 │
│  ┌──────────┐    sim_emit     ┌───────────┐    sim_emit         │
│  │ Sim Cell │ ──────────────► │  Bus Map  │ ◄────────────────── │
│  │ tick()   │ ◄────────────── │           │    (auto from       │
│  │          │  ironpad_sim_   │           │     slider change)  │
│  │          │  read (import)  │           │                     │
│  └──────────┘                 └───────────┘                     │
│                                    │                            │
│                                    │ ironpad_sim_read_all       │
│                                    ▼                            │
│                              ┌───────────┐                      │
│                              │ Downstream│                      │
│                              │  Cell     │                      │
│                              └───────────┘                      │
└─────────────────────────────────────────────────────────────────┘
```

## Phase 1: Core Bus

### Rust API (`crates/ironpad-cell/src/sim.rs`)

```rust
// ── FFI imports (wasm32 only) ──
extern "C" {
    fn ironpad_sim_read(key_ptr: *const u8, key_len: u32) -> u32;
    fn ironpad_sim_read_all(key_ptr: *const u8, key_len: u32) -> u32;
}

/// Emit a named value to the simulation bus.
pub fn emit<T: serde::Serialize>(key: &str, value: &T) {
    host_message_json(&serde_json::json!({
        "type": "sim_emit",
        "key": key,
        "value": value,
    }));
}

/// Read the latest value for a key. Returns None if not yet emitted.
pub fn read<T: serde::de::DeserializeOwned>(key: &str) -> Option<T> {
    // On wasm32: call ironpad_sim_read FFI, get pointer to JSON bytes
    // On native: return None (no bus available)
}

/// Read all buffered values (ring buffer, oldest first).
pub fn read_all<T: serde::de::DeserializeOwned>(key: &str) -> Vec<T> {
    // On wasm32: call ironpad_sim_read_all FFI, get pointer to JSON array
    // On native: return empty Vec
}
```

### JS Bus Store (`public/executor.js`)

```javascript
// On CellExecutor:
this._simBus = new Map();  // key → { latest: jsonString, ring: string[] }

// Host message handler:
executor.onHostMessage("sim_emit", function(msg, cellId) {
    var key = msg.key;
    var json = JSON.stringify(msg.value);
    var entry = executor._simBus.get(key);
    if (!entry) {
        entry = { latest: json, ring: [] };
        executor._simBus.set(key, entry);
    }
    entry.latest = json;
    entry.ring.push(json);
    if (entry.ring.length > 1000) entry.ring.shift();
});

// WASM import: ironpad_sim_read(key_ptr, key_len) → ptr
//   Reads key string from WASM memory, looks up bus, writes latest JSON
//   back into WASM memory via ironpad_alloc. Returns pointer (0 = no value).

// WASM import: ironpad_sim_read_all(key_ptr, key_len) → ptr
//   Same but writes JSON array of all ring buffer entries.
```

### Return Protocol for `ironpad_sim_read`

The JS function allocates WASM memory via `ironpad_alloc` and writes a length-prefixed JSON payload:

```
[4 bytes: u32 LE length][N bytes: UTF-8 JSON]
```

Returns the pointer to the start. Rust side reads the length, then the JSON bytes, then frees via `ironpad_dealloc`. Returns 0 if no value exists for the key.

## Phase 2: SimSlider Widget

### Rust API

```rust
// In Simulation trait:
fn sliders() -> Vec<SimSliderMeta> { vec![] }

// SimSliderMeta:
pub struct SimSliderMeta {
    pub key: String,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub label: String,
    pub default: f64,
}
```

### SimulationMeta Extension

```rust
pub struct SimulationMeta {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub first_frame: Canvas,
    pub sliders: Vec<SimSliderMeta>,  // NEW
}
```

### Frontend Rendering

`SimulationCanvas` component reads `sliders` from meta and renders `<input type="range">` elements below the canvas. Each slider's `oninput` event calls the executor to emit the value to the bus:

```javascript
// On slider change:
executor._simBus.set(key, {
    latest: JSON.stringify(newValue),
    ring: [...existing.ring, JSON.stringify(newValue)]
});
```

No round-trip through WASM — the executor directly writes to the bus store.

## Phase 3: Downstream Consumption

Wire `ironpad_sim_read` and `ironpad_sim_read_all` imports for all cell types (not just simulations). A regular cell can then do:

```rust
let k_history = sim::read_all::<f64>("k_eff");
// Plot k_history as a chart
```

This enables live dashboards alongside running simulations.

---

# Assumptions

1. PRD-0017 (Animated Cell Output) is complete — `Simulation` trait, `SimulationMeta`, `TickResult`, `cell_tick` export, and `SimulationCanvas` all exist and work.
2. The Web Worker execution path (PRD-0013) is operational — both main-thread and worker paths must support the bus.
3. JSON serialization overhead is acceptable for bus values (typically small scalars or short arrays).
4. Ring buffer of 1000 entries per key is sufficient for time-series plotting at 30fps (~33 seconds of history).

---

# Constraints

1. **No changes to Simulation::tick() signature** — `tick(&mut self) -> Canvas` stays the same. Bus access is via free functions in the `sim` module.
2. **WASM import count** — adding 2 new imports (`ironpad_sim_read`, `ironpad_sim_read_all`) to the `env` namespace. Must be wired in all 3 executor paths (raw, wasm-bindgen, worker).
3. **Memory management** — JS writes JSON into WASM linear memory via `ironpad_alloc`; Rust must `ironpad_dealloc` after reading. Failure to dealloc leaks memory.
4. **CFL note for reactor notebook** — when updating the reactor notebook (T-010), do NOT change the physics sub-step count or diffusion coefficients. Only replace hardcoded constants with `sim::read` calls.

---

# References to Code

- `crates/ironpad-cell/src/lib.rs` — host_message FFI (line 14-48), Simulation trait (line 813-822), SimulationMeta (line 825-830), prelude (line 52-79)
- `crates/ironpad-cell/src/ui.rs` — existing Slider widget (line 32-119), ProgressHandle pattern (line 567-674)
- `crates/ironpad-cell/src/canvas.rs` — Canvas type used by tick()
- `crates/ironpad-app/src/compiler/scaffold.rs` — `is_simulation()` (line 298), `generate_simulation_lib_rs()` (line 443)
- `crates/ironpad-app/src/components/animation_canvas.rs` — SimulationCanvas component (line 288-497)
- `crates/ironpad-app/src/components/executor.rs` — tick_cell bridge (line 194-232)
- `public/executor.js` — CellExecutor, loadBlob env imports (line 96-180), host message dispatch (line 50-86), tick (line 386-426)
- `public/executor-bridge.js` — BridgeExecutor, host message forwarding (line 254-277)
- `public/executor-worker.js` — Worker host message intercept (line 30-55), command handler (line 64+)
- `public/notebooks/nuclear-reactor.ironpad` — motivating notebook for slider integration

---

# Non-Goals (MVP)

- Real-time charting / plotting widget (downstream cells can read_all, but we don't build a chart component)
- Bus persistence across page reloads (bus is ephemeral, in-memory only)
- Bus networking (multi-user bus sync via WebSocket — out of scope)
- Type-safe bus keys (keys are plain strings; type safety is the caller's responsibility)
- Bus value validation or schema enforcement
- Throttling / debouncing of slider emissions (every change event emits immediately)

---

# History

(Entries appended during implementation go below this line.)

---
