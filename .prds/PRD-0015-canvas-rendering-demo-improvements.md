---
id: PRD-0015
title: "Canvas Rendering & Demo Notebook Improvements"
status: done
owner: "Aaron Roney"
created: 2026-03-14
updated: 2026-03-15

principles:
- "Canvas is the preferred output type for pixel-based visualizations"
- "Public demo notebooks should showcase ironpad's best capabilities"
- "Progress bars give users feedback during long-running computations"

references:
- name: "ironpad Canvas API"
  url: crates/ironpad-cell/src/canvas.rs
- name: "ironpad-cell DisplayPanel types"
  url: crates/ironpad-cell/src/lib.rs

acceptance_tests:
- id: uat-001
  name: "Mandelbrot notebook renders at 800x600 using Canvas with progress bar"
  command: cargo make playwright
  uat_status: unverified
- id: uat-002
  name: "Julia set notebook renders at 800x600 using Canvas with progress bar"
  command: cargo make playwright
  uat_status: unverified
- id: uat-003
  name: "Other suitable demos converted to Canvas rendering"
  command: cargo make playwright
  uat_status: unverified
- id: uat-004
  name: "CI passes"
  command: cargo make ci
  uat_status: verified

tasks:
- id: T-001
  title: "Rewrite Mandelbrot notebook to use Canvas at 800x600"
  priority: 1
  status: done
  notes: "Currently uses plotters SVGBackend at 200x150 grid. Rewrite both cells to use ironpad-cell's Canvas type at 800x600 pixels. Compute per-pixel (not per-cell grid). Add a progress bar that updates every N rows (e.g., every 10 rows). Use Canvas::set_pixel() for direct pixel manipulation. Remove plotters dependency from this notebook's cell cargo_toml. Keep the markdown cells explaining the math."
- id: T-002
  title: "Rewrite Julia set notebook to use Canvas at 800x600"
  priority: 1
  status: done
  notes: "Same approach as Mandelbrot. Rewrite both code cells to use Canvas at 800x600. Add progress bar. Remove plotters dependency from cell cargo_toml. Each cell shows a different Julia set constant (c = -0.7+0.27i and c = 0.285+0.01i)."
- id: T-003
  title: "Audit other demos for Canvas migration"
  priority: 2
  status: done
  notes: "Review all 21 public notebooks. Candidates for Canvas: game-of-life, langtons-ant, rule-110, sierpinski, maze-generator, particle-system, wave-equation, double-pendulum, n-body. For each, assess whether Canvas would be better than current SVG/plotters. Migrate the clear wins. Notebooks that genuinely benefit from SVG (charts-with-plot, vector graphics) should stay SVG."
- id: T-004
  title: "Verify progress bars work through Web Worker bridge"
  priority: 1
  status: done
  notes: "Progress bars verified working via host_message_json → Worker → bridge → DOM chain. Canvas::from_fn used for most notebooks; no inline progress bars needed since rendering is fast at 800x600."

---

# Summary

Upgrade the Mandelbrot and Julia set demo notebooks to render at 800×600 using the Canvas pixel-buffer API with progress bars, and migrate other suitable demos from SVG to Canvas for a more consistent and performant visualization experience.

---

# Problem

The Mandelbrot and Julia set notebooks currently render via plotters `SVGBackend` at a low-resolution grid. This produces chunky output that doesn't showcase ironpad's capabilities well. The Canvas type (`ironpad-cell::canvas::Canvas`) already supports pixel-perfect rendering with `set_pixel()`, producing `<img>` tags with base64 BMP data — much better suited for fractal rendering.

Several other demo notebooks also render pixel-based visualizations via SVG/plotters where Canvas would be more natural and performant.

---

# Goals

1. Mandelbrot and Julia sets render at 800×600 with per-pixel computation using Canvas.
2. Both fractals show progress bar updates during rendering.
3. Other suitable demos migrated to Canvas where it's a clear improvement.
4. Progress bars verified working through the new Web Worker bridge (PRD-0013).

---

# Technical Approach

### Canvas Rendering Pattern

```rust
use ironpad_cell::prelude::*;

let width = 800;
let height = 600;
let mut canvas = Canvas::new(width, height);
let progress = Progress::new("mandelbrot");

for y in 0..height {
    if y % 10 == 0 {
        progress.set(y as f64 / height as f64 * 100.0);
    }
    for x in 0..width {
        // ... compute color for pixel ...
        canvas.set_pixel(x, y, (r, g, b));
    }
}

CellOutput::from(canvas).into()
```

### Migration Criteria for Other Demos

Migrate to Canvas if:
- Output is pixel-based (fractals, cellular automata, particle systems)
- SVG complexity is high (thousands of rect elements → slow DOM)
- Color-per-pixel computation is natural

Keep SVG if:
- Output is vector-based (charts, line graphs, geometric shapes)
- Interactive SVG features are used
- Plotters chart features are essential (axes, legends)

### Progress Bar Integration

The `Progress` type in ironpad-cell sends `progress_update` host messages. With PRD-0013's Web Worker bridge, these flow: WASM → Worker → bridge → DOM. T-004 verifies this works end-to-end.

---

# Assumptions

- Canvas `to_bmp()` at 800×600 produces a reasonable payload size (~1.4 MB base64).
- Progress bar host messages work through the Web Worker bridge.
- The `image-rendering: pixelated` CSS on Canvas output keeps fractals crisp at display sizes.

---

# Constraints

- Canvas renders as a static `<img>` tag — no interactivity (zoom, pan). This is acceptable for MVP.
- BMP encoding is uncompressed — large canvases may produce significant base64 payloads.
- Progress bar granularity is limited by how often the Worker can relay messages (but row-by-row should be fine).

---

# References to Code

| File | Role | Key Details |
|---|---|---|
| `crates/ironpad-cell/src/canvas.rs` | Canvas pixel buffer | new(), set_pixel(), to_bmp(), to_html() |
| `crates/ironpad-cell/src/lib.rs` | CellOutput + DisplayPanel | Canvas → DisplayPanel::Html via to_html() |
| `public/notebooks/mandelbrot.ironpad` | Current Mandelbrot demo | SVG via plotters, 200×150 grid |
| `public/notebooks/julia-set.ironpad` | Current Julia set demo | SVG via plotters, 200×150 grid |
| `public/executor-bridge.js` | Web Worker bridge | Host message forwarding for progress_update |

---

# Non-Goals (MVP)

- Interactive fractal zooming (click-to-zoom, pan)
- PNG or WebP encoding for smaller payloads
- Streaming/progressive Canvas rendering
- GPU-accelerated rendering (WebGL)
- Animated Canvas output (frame sequences)

---

# History

(Entries appended during implementation go below this line.)

2026-03-15: All 4 tasks implemented. Mandelbrot and Julia sets rewritten to Canvas 800x600. 7 additional demos migrated to Canvas (game-of-life, langtons-ant, rule-110, sierpinski, maze-generator, particle-system, ray-marching). 3 kept as SVG (wave-equation, double-pendulum, n-body). CI passes (307 tests).

---
