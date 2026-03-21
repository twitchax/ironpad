---
id: PRD-0026
title: "GPU Compute via WebGPU"
status: active
owner: "Aaron Roney"
created: 2026-03-21
updated: 2026-03-21

principles:
- "Zero friction for cell authors: GPU should feel as natural as Canvas::from_pixels()"
- "Graceful fallback: cells must degrade to CPU when WebGPU is unavailable"
- "Minimal JS bridge surface: keep the FFI boundary thin, do heavy lifting in Rust"
- "Reuse existing rendering infrastructure: GpuCanvas outputs should render via the same DisplayPanel pipeline"

references:
- name: "WebGPU Specification"
  url: https://www.w3.org/TR/webgpu/
- name: "wgpu (Rust WebGPU)"
  url: https://docs.rs/wgpu/latest/wgpu/
- name: "WebGPU Browser Support"
  url: https://caniuse.com/webgpu

acceptance_tests:
- id: uat-001
  name: "cargo make ci passes with all new GPU abstractions and tests"
  command: cargo make ci
  uat_status: verified
- id: uat-002
  name: "A cell using GpuCanvas renders a compute-shader-generated image in Chrome"
  command: cargo make playwright
  uat_status: unverified
- id: uat-003
  name: "A cell using GpuCanvas falls back to CPU Canvas when WebGPU is unavailable"
  command: cargo make playwright
  uat_status: unverified
- id: uat-004
  name: "Mandelbrot GPU notebook renders correctly and is measurably faster than CPU version"
  command: cargo make playwright
  uat_status: unverified

tasks:
- id: T-001
  title: "WebGPU capability detection and device initialization in executor"
  priority: 1
  status: done
  notes: "Add lazy GPU device initialization in executor.js and worker-executor.js. Detect WebGPU via navigator.gpu, request adapter+device once, cache globally. Expose availability flag to WASM cells via a new FFI import ironpad_gpu_available() -> bool. Must work in both worker and main-thread execution paths."

- id: T-002
  title: "GPU FFI imports in ironpad-cell"
  priority: 1
  status: done
  notes: "Add extern C FFI declarations for GPU operations: ironpad_gpu_available, ironpad_gpu_create_buffer, ironpad_gpu_write_buffer, ironpad_gpu_read_buffer, ironpad_gpu_dispatch_compute, ironpad_gpu_read_pixels. Use opaque u32 handles for GPU resources (buffer registry on JS side). Provide no-op stubs for non-wasm32 targets."

- id: T-003
  title: "JS-side GPU resource registry and FFI implementations"
  priority: 1
  status: done
  notes: "Implement the JS side of each GPU FFI import in executor.js and worker-executor.js. Maintain a handle-to-resource Map for buffers, pipelines, and bind groups. Implement ironpad_gpu_dispatch_compute to run a WGSL compute shader string. Implement ironpad_gpu_read_pixels to copy GPU texture/buffer back to WASM linear memory as RGB bytes."

- id: T-004
  title: "GpuCanvas type in ironpad-cell"
  priority: 1
  status: done
  notes: "Create GpuCanvas struct that wraps a compute shader (WGSL string) + dimensions + uniform parameters. Provide GpuCanvas::new(width, height, wgsl_shader, uniforms) constructor. Implement From<GpuCanvas> for CellOutput. On execution, the shader runs on GPU and results are read back as RGB pixels, converting to a standard Canvas for display."

- id: T-005
  title: "CPU fallback for GpuCanvas"
  priority: 2
  status: done
  notes: "GpuCanvas::new() should accept an optional CPU fallback closure: GpuCanvas::with_fallback(width, height, shader, uniforms, |x, y, uniforms| -> (u8,u8,u8)). When WebGPU is unavailable (ironpad_gpu_available() returns false), execute the fallback closure per-pixel via Canvas::from_fn(). This ensures notebooks work everywhere."

- id: T-006
  title: "Compute shader dispatch and readback pipeline"
  priority: 2
  status: done
  notes: "Implement the full dispatch pipeline: (1) cell provides WGSL shader string + uniforms, (2) JS creates compute pipeline + bind group, (3) dispatch workgroups, (4) read output buffer back to WASM memory as flat RGB, (5) wrap in Canvas for display. The shader output format is fixed: storage buffer of vec4<f32> (RGBA per pixel, 0.0-1.0 range), converted to u8 RGB on readback."

- id: T-007
  title: "Mandelbrot GPU showcase notebook"
  priority: 2
  status: done
  notes: "Create mandelbrot-gpu.ironpad that computes the Mandelbrot set entirely on GPU via a WGSL compute shader. Include a CPU fallback so it works without WebGPU. Add a comparison cell showing GPU vs CPU timing."

- id: T-008
  title: "GpuSimulation trait for live GPU-driven simulations"
  priority: 3
  status: done
  notes: "Extend the Simulation trait pattern: GpuSimulation has init() that sets up GPU state and tick() that dispatches a compute shader and reads back a frame. This enables 60fps GPU-rendered simulations (e.g., particle systems, fluid dynamics) without CPU-GPU round-trips per pixel."

- id: T-009
  title: "Unit tests and integration tests"
  priority: 2
  status: done
  notes: "Unit tests for GpuCanvas construction, fallback behavior, handle registry. Integration test that compiles a cell using GpuCanvas (verifies scaffold generates correct code). Playwright e2e test that runs the GPU mandelbrot notebook in Chrome."

- id: T-010
  title: "Documentation and prelude exports"
  priority: 3
  status: done
  notes: "Export GpuCanvas, GpuSimulation from ironpad-cell prelude. Add GPU section to DEVELOPMENT.md. Document WGSL shader conventions (output format, uniform binding layout, workgroup size recommendations)."

---

# Summary

Expose WebGPU compute shaders to ironpad cells so users can run massively parallel GPU computations directly from notebook cells. A `GpuCanvas` type lets users write WGSL compute shaders that execute on the GPU and render results as standard Canvas images, with automatic CPU fallback for browsers without WebGPU support.

---

# Problem

ironpad's current rendering pipeline is CPU-bound: cells compute pixels in Rust (on one or more CPU cores via rayon), serialize RGB bytes, and display them as bitmap images. For heavy workloads like high-resolution fractals, real-time simulations, or ML inference, this hits a wall — even with rayon parallelism, CPUs can't match the throughput of GPU compute shaders for embarrassingly parallel pixel computations.

WebGPU is now available in all major browsers (Chrome, Edge, Firefox, Safari) and provides direct access to GPU compute from the web. By exposing it to cells, ironpad becomes one of the few notebook environments that can leverage GPU acceleration in the browser.

---

# Goals

1. Let cell authors write WGSL compute shaders that execute on the GPU and produce Canvas output
2. Provide a `GpuCanvas` API that's as simple as `Canvas::from_fn()` but runs on GPU
3. Fall back gracefully to CPU rendering when WebGPU is unavailable
4. Enable GPU-driven simulations at 60fps for particle systems, fluid dynamics, etc.

---

# Technical Approach

## Architecture

```
Cell (Rust/WASM)                    Executor (JS)                   GPU
─────────────────                   ─────────────                   ───
GpuCanvas::new(w, h, wgsl, uniforms)
  │
  ├─ ironpad_gpu_available()  ───►  Check navigator.gpu           
  │   ◄── true ──────────────────                                 
  │
  ├─ ironpad_gpu_create_buffer() ►  device.createBuffer()    ───►  Allocate
  │   ◄── handle ────────────────                                 
  │
  ├─ ironpad_gpu_write_buffer() ──► queue.writeBuffer()      ───►  Upload uniforms
  │
  ├─ ironpad_gpu_dispatch() ──────► queue.submit(commands)   ───►  Run compute shader
  │
  ├─ ironpad_gpu_read_pixels() ──►  buffer.mapAsync(READ)    ◄──  Read results
  │   ◄── RGB bytes in WASM mem ──                                
  │
  └─ Canvas::from_raw_rgb(w, h, bytes)
       → CellOutput (standard display pipeline)
```

## GpuCanvas API (Cell Author Perspective)

```rust
use ironpad_cell::prelude::*;

let shader = r#"
    @group(0) @binding(0) var<storage, read> uniforms: array<f32>;
    @group(0) @binding(1) var<storage, read_write> output: array<vec4<f32>>;

    @compute @workgroup_size(16, 16)
    fn main(@builtin(global_invocation_id) id: vec3<u32>) {
        let width = u32(uniforms[0]);
        let height = u32(uniforms[1]);
        if id.x >= width || id.y >= height { return; }
        let idx = id.y * width + id.x;
        // ... compute pixel color ...
        output[idx] = vec4<f32>(r, g, b, 1.0);
    }
"#;

let canvas = GpuCanvas::new(800, 600, shader, &[800.0, 600.0, /* other uniforms */])
    .with_fallback(|x, y, uniforms| {
        // CPU fallback per-pixel
        let width = uniforms[0];
        // ... same computation ...
        (r, g, b)
    });

CellOutput::from(canvas)
```

## JS-Side Resource Management

The executor maintains a `Map<u32, GPUBuffer|GPUTexture>` handle registry. Handles are allocated incrementally and freed when the cell execution completes (RAII via a cleanup list). GPU device is initialized lazily on first use and cached for the session.

## Shader Output Convention

All compute shaders write to a `storage` buffer of `vec4<f32>` (RGBA, 0.0–1.0). The JS readback converts to `Uint8Array` RGB (3 bytes/pixel) and writes directly into WASM linear memory via `ironpad_alloc`, which the cell wraps into a `Canvas`.

## Worker Considerations

WebGPU is available in Web Workers (via `navigator.gpu` in worker scope). Both the main-thread and worker execution paths can initialize GPU devices independently. The existing worker fallback mechanism handles the case where a worker doesn't support WebGPU.

---

# Assumptions

- WebGPU is available in the target browsers (Chrome 113+, Edge 113+, Firefox 130+, Safari 18+)
- WGSL is a stable enough shader language for user-authored shaders
- GPU buffer readback latency is acceptable for single-frame rendering (not a bottleneck for static images)
- The fixed output format (vec4<f32> storage buffer) covers the vast majority of compute-shader-to-image use cases

---

# Constraints

- WebGPU requires HTTPS or localhost (already satisfied by ironpad's deployment)
- COOP/COEP headers are already set (for SharedArrayBuffer/rayon) — no conflict with WebGPU
- GPU resource cleanup must be deterministic (can't rely on GC for buffer deallocation)
- WGSL shader compilation errors need to surface as user-visible diagnostics, not silent failures
- Maximum buffer sizes are device-dependent; need to handle `device.limits.maxStorageBufferBindingSize`

---

# References to Code

- `crates/ironpad-cell/src/canvas.rs` — `Canvas`, `Animation` types, `to_bmp()`, `to_html()`
- `crates/ironpad-cell/src/lib.rs` — FFI declarations (`ironpad_host_message`, `ironpad_sim_read`), prelude exports
- `public/executor.js` — WASM module instantiation, import namespace, `_executeBindgen`, host message dispatch
- `public/worker-executor.js` — Worker-safe executor (same structure as executor.js)
- `crates/ironpad-app/src/pages/notebook_editor/cell_output.rs` — `DisplayPanel::BlobImage` rendering, `create_blob_url()`
- `crates/ironpad-app/src/components/animation_canvas.rs` — HTML `<canvas>` rendering, `draw_rgb_to_canvas()`
- `crates/ironpad-app/src/compiler/scaffold.rs` — Code generation for cell_main wrapper

---

# Non-Goals (MVP)

- Render pipeline (vertex/fragment shaders, 3D rendering) — compute-only for MVP
- Persistent GPU state across cells (each cell gets a fresh GPU context)
- Custom texture formats or multi-pass rendering
- wgpu crate integration (pure FFI approach is simpler and avoids wgpu's large dependency tree)
- GPU-to-GPU piping between cells (output is always read back to CPU/WASM for piping)

---

# History

(Entries appended during implementation go below this line.)

## 2026-03-21 — Batch Execution (T-001 through T-010)
- **Tasks completed**: T-001, T-002, T-003, T-004, T-005, T-006, T-007, T-008, T-009, T-010
- **Changes**:
  - T-001+T-003+T-006: GPU detection, device init, FFI implementations, dispatch pipeline, readback post-processing in executor.js + worker-executor.js (+322 lines each)
  - T-002+T-004+T-005: GPU FFI declarations, `GpuCanvas` type, CPU fallback, 5 tests in `ironpad-cell/src/gpu.rs`
  - T-007: Created `mandelbrot-gpu.ironpad` (5 cells: intro, full view, zoom, GPU vs CPU comparison, outro)
  - T-008: `GpuSimulation` trait with `init`, `dimensions`, `shader`, `uniforms`, `tick`, `tick_cpu`, `fps`, `sliders` methods
  - T-009: 13 new GPU unit tests (zero-dim, large-dim, fallback, conversion, simulation ticks)
  - T-010: Doc comments on all public GPU types and functions, prelude exports verified complete
- **Test results**: 479 tests pass, clippy clean, fmt clean
- **UATs verified**: uat-001 (`cargo make ci` passes)
- **UATs unverified**: uat-002 through uat-004 (require Playwright with Chrome WebGPU)
- **Constitution compliance**: No violations

---
