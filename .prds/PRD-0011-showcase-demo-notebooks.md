---
id: PRD-0011
title: "Showcase Demo Notebooks: Fractals, Automata, Physics, and More"
status: done
owner: "Aaron Roney"
created: 2026-03-13
updated: 2026-03-14

principles:
- "Each demo is a standalone public notebook — self-contained and impressive"
- "Mix rendering approaches: plotters for static/chart output, canvas API for animated/interactive demos"
- "Demos should compile and run quickly — keep dependencies minimal"
- "Code should be readable and educational, with markdown cells explaining the concepts"
- "Showcase ironpad's unique strengths: Rust + WASM performance, interactivity, data piping between cells"

references:
- name: "Public notebooks directory"
  url: public/notebooks/
- name: "Public notebook index"
  url: public/notebooks/index.json
- name: "ironpad-cell crate (display/widget API)"
  url: crates/ironpad-cell/
- name: "Existing welcome notebook (plotters example)"
  url: public/notebooks/welcome.ironpad
- name: "Interactive widgets notebook"
  url: public/notebooks/interactive-widgets.ironpad

acceptance_tests:
- id: uat-001
  name: "All new demo notebooks appear in the public notebook index"
  command: cargo make uat
  uat_status: verified
- id: uat-002
  name: "Each demo notebook loads and renders without errors in the browser"
  command: cargo make uat
  uat_status: unverified
- id: uat-003
  name: "All demo notebook JSON files are valid IronpadNotebook format"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "Create Mandelbrot Set notebook"
  priority: 1
  status: done
  notes: "Fractal visualization of the Mandelbrot set. Cells: (1) markdown intro explaining the math, (2) compute iteration counts for a grid, (3) render as a color-mapped image using plotters or canvas. Include zoom levels showing detail. Use escape-time algorithm with configurable max iterations."

- id: T-002
  title: "Create Julia Set notebook"
  priority: 1
  status: done
  notes: "Julia set visualization with configurable c parameter. Cells: (1) markdown explaining relationship to Mandelbrot, (2) compute and render Julia set for a visually striking c value (e.g., c = -0.7 + 0.27015i), (3) show multiple c values side by side. Use similar rendering approach to Mandelbrot."

- id: T-003
  title: "Create Sierpinski Triangle notebook"
  priority: 2
  status: done
  notes: "Sierpinski triangle via chaos game algorithm. Cells: (1) markdown explaining the chaos game, (2) implement iterative point plotting (pick random vertex, move halfway), (3) render accumulated points. Show progressive build-up. Could also show recursive subdivision approach in a separate cell."

- id: T-004
  title: "Create Conway's Game of Life notebook"
  priority: 1
  status: done
  notes: "Classic cellular automaton. Cells: (1) markdown explaining rules (birth/survival), (2) implement grid with step function, (3) render animated frames using canvas API or multi-frame output. Start with interesting seed patterns (glider gun, pulsar, etc.). Show multiple generations."

- id: T-005
  title: "Create Rule 110 notebook"
  priority: 2
  status: done
  notes: "1D elementary cellular automaton (Turing-complete). Cells: (1) markdown explaining elementary CA and Rule 110, (2) implement rule application over generations, (3) render as a 2D image where each row is a generation. Classic black-and-white pixel art aesthetic."

- id: T-006
  title: "Create Langton's Ant notebook"
  priority: 2
  status: done
  notes: "Langton's Ant Turing machine on a 2D grid. Cells: (1) markdown explaining the simple rules, (2) simulate N steps, (3) render the grid showing the emergent highway pattern. Run enough steps (~10k+) to show the transition from chaos to order."

- id: T-007
  title: "Create N-Body Gravity Simulation notebook"
  priority: 1
  status: done
  notes: "Gravitational N-body simulation. Cells: (1) markdown on gravitational mechanics, (2) implement Euler or Verlet integration for N particles, (3) render particle positions over time frames. Start with interesting initial conditions (binary star, solar system, galaxy collision). Show orbital paths."

- id: T-008
  title: "Create Double Pendulum notebook"
  priority: 1
  status: done
  notes: "Chaotic double pendulum simulation. Cells: (1) markdown explaining Lagrangian mechanics and chaos, (2) implement RK4 integration for the equations of motion, (3) render the pendulum trajectory. Show sensitivity to initial conditions by overlaying two pendulums with slightly different starting angles."

- id: T-009
  title: "Create Wave Equation notebook"
  priority: 2
  status: done
  notes: "1D or 2D wave equation simulation. Cells: (1) markdown on the wave equation PDE, (2) implement finite difference method, (3) render wave propagation as animated frames or a space-time plot. Show reflection, interference, standing waves."

- id: T-010
  title: "Create Ray Marching notebook"
  priority: 2
  status: done
  notes: "Simple ray marching renderer. Cells: (1) markdown on signed distance functions and ray marching, (2) implement SDF primitives (sphere, box, torus) with CSG operations, (3) render a scene with lighting and shadows. Output as a pixel buffer rendered to canvas or plotters image."

- id: T-011
  title: "Create Particle System notebook"
  priority: 2
  status: done
  notes: "GPU-style particle system on CPU. Cells: (1) markdown on particle systems, (2) implement emitter with velocity, gravity, lifetime, (3) render particles as colored dots over multiple frames. Show fountain, explosion, or fire effect."

- id: T-012
  title: "Create Maze Generator notebook"
  priority: 2
  status: done
  notes: "Maze generation and solving. Cells: (1) markdown on maze algorithms, (2) generate maze using recursive backtracker or Kruskal's algorithm, (3) solve with A* or BFS and render solution path. Show the maze grid with walls and the solution highlighted."

- id: T-013
  title: "Update public notebook index with all new demos"
  priority: 1
  status: done
  notes: "Add all 12 new notebooks to public/notebooks/index.json with appropriate titles, descriptions, cell counts, and tags. Group them logically (fractals, cellular automata, physics, other). Verify the index loads correctly on the home page."
---

# Summary

Create 12 new public demo notebooks showcasing ironpad's capabilities through visually impressive simulations: fractals (Mandelbrot, Julia, Sierpinski), cellular automata (Game of Life, Rule 110, Langton's Ant), physics simulations (N-body gravity, double pendulum, wave equation), and creative coding (ray marching, particle system, maze generation).

---

# Problem

ironpad currently has 9 public notebooks focused on introductory features (tutorials, charts, widgets). There are no visually striking demos that showcase the platform's key differentiator: running real Rust code compiled to WASM in the browser with near-native performance. Impressive demos are critical for attracting users and demonstrating that ironpad is more than a toy.

---

# Goals

1. Create 12 standalone demo notebooks covering fractals, cellular automata, physics, and creative coding.
2. Each notebook is self-contained, educational (with markdown explanations), and visually impressive.
3. Demos showcase ironpad's strengths: Rust performance, WASM execution, interactivity, cell data piping.
4. All demos appear in the public notebook index on the home page.

---

# Technical Approach

## Rendering Strategy

Use a mix of rendering approaches depending on the demo:

- **plotters crate**: Best for static images, charts, and single-frame renders (Mandelbrot, Julia, Sierpinski, Rule 110, maze).
- **Canvas/HTML via ironpad-cell display API**: Best for animated or interactive demos (Game of Life, N-body, double pendulum, particles, wave equation).
- **SVG output**: Good for vector graphics (Langton's Ant grid, maze).

## Notebook Structure Pattern

Each notebook follows a consistent structure:
1. **Intro cell** (Markdown): Explain the concept, math, and what the user will see.
2. **Implementation cell(s)** (Code): Core algorithm, well-commented for education.
3. **Render cell** (Code): Visualization output, may use data piped from previous cells.
4. **Exploration cell** (Markdown or Code): Suggestions for modifications, parameter tweaks.

## Dependencies

Keep dependencies minimal per notebook:
- `plotters` for static rendering (already in shared deps of welcome notebook).
- `image` crate if raw pixel buffer manipulation is needed.
- No external HTTP dependencies — all computation is local.

## File Format

Each notebook is a `.ironpad` JSON file in `public/notebooks/` following the existing `IronpadNotebook` schema. Include appropriate `shared_cargo_toml` for common dependencies across cells.

---

# Assumptions

- The ironpad-cell display/widget API supports rendering images and canvas output.
- plotters crate works in the WASM compilation pipeline.
- Public notebook loading and index mechanisms work correctly.

---

# Constraints

- Each notebook must compile and render within reasonable time (~30s compile, instant render).
- No external network dependencies at runtime — all computation is local.
- Code must be readable and educational, not just correct.
- Total new content should not bloat the repo excessively (JSON files are text, but large pixel data should be avoided in favor of procedural generation).

---

# References to Code

- `public/notebooks/` — Existing public notebook files.
- `public/notebooks/index.json` — Public notebook index.
- `crates/ironpad-cell/` — Cell runtime with display/widget API.
- `crates/ironpad-common/src/types.rs` — `IronpadNotebook`, `IronpadCell`, `CellType` definitions.
- `public/notebooks/welcome.ironpad` — Example of plotters usage.
- `public/notebooks/interactive-widgets.ironpad` — Example of interactive widget usage.

---

# Non-Goals (MVP)

- Real-time interactive parameter tuning (sliders, etc.) — static or pre-animated output is fine for MVP.
- GPU-accelerated rendering — CPU computation rendered to canvas/plotters is sufficient.
- Mobile-optimized rendering — desktop browser is the target.
- Audio output or 3D rendering.

---

# History

## 2026-03-14 — Batch Execution (T-001 through T-013)
- **Tasks completed**: T-001, T-002, T-003, T-004, T-005, T-006, T-007, T-008, T-009, T-010, T-011, T-012, T-013
- **Changes**:
  - T-001: Created `mandelbrot.ironpad` — escape-time with color mapping via SVG, 4 cells
  - T-002: Created `julia-set.ironpad` — configurable c parameter, SVG rendering, 4 cells
  - T-003: Created `sierpinski.ironpad` — chaos game algorithm, SVG output, 4 cells
  - T-004: Created `game-of-life.ironpad` — classic CA with glider gun seed, canvas rendering, 4 cells
  - T-005: Created `rule-110.ironpad` — 1D elementary CA, SVG space-time plot, 3 cells
  - T-006: Created `langtons-ant.ironpad` — emergent highway pattern after 11k steps, SVG, 3 cells
  - T-007: Created `n-body.ironpad` — gravitational simulation with Verlet integration, SVG, 4 cells
  - T-008: Created `double-pendulum.ironpad` — RK4 integration showing chaos, SVG trajectory, 5 cells
  - T-009: Created `wave-equation.ironpad` — 1D finite difference PDE, SVG space-time heatmap, 4 cells
  - T-010: Created `ray-marching.ironpad` — SDF primitives with lighting/shadows, shared_source for Vec3, 4 cells
  - T-011: Created `particle-system.ironpad` — fountain emitter with gravity/lifetime, shared_source, 5 cells
  - T-012: Created `maze-generator.ironpad` — recursive backtracker + BFS solver, shared_source, 5 cells
  - T-013: Updated `index.json` with 12 new entries (21 total), organized by category
- **Test results**: All notebooks are valid JSON, index loads correctly
- **UATs verified**: uat-001, uat-003
- **UATs deferred**: uat-002 (browser rendering requires manual verification)
- **Constitution compliance**: No violations. All notebooks use deterministic xorshift PRNGs for reproducibility.

---
