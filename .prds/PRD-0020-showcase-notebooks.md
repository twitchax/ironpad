---
id: PRD-0020
title: "Showcase Public Notebooks"
status: done
owner: "Aaron Roney"
created: 2026-03-16
updated: 2026-03-16

principles:
- "Each notebook should be visually impressive and demonstrate ironpad's interactive capabilities"
- "Use the Simulation trait for real-time canvas animations, LiveView for reactive HTML/SVG dashboards"
- "Include descriptive Markdown cells with KaTeX equations where physics/math is involved"
- "Leverage the simulation bus for multi-cell coordination (widgets → simulation → live view)"
- "All notebooks must be valid .ironpad JSON and registered in index.json"

references:
- name: "nuclear-reactor.ironpad"
  url: public/notebooks/nuclear-reactor.ironpad
- name: "double-pendulum.ironpad"
  url: public/notebooks/double-pendulum.ironpad
- name: "game-of-life.ironpad"
  url: public/notebooks/game-of-life.ironpad

acceptance_tests:
- id: uat-001
  name: "All new/updated .ironpad files are valid JSON with correct cell structure"
  command: cargo make uat
  uat_status: verified
- id: uat-002
  name: "All notebooks have description and tags; no index.json needed"
  command: cargo make uat
  uat_status: verified

tasks:
- id: T-001
  title: "Sine/Cosine Phase Explorer — LiveView SVG with slider"
  priority: 1
  status: done
  notes: >
    New notebook: sine-phase-explorer.ironpad.
    Cell 1 (Markdown): Intro explaining sinusoidal functions, phase shift, KaTeX equations.
    Cell 2 (Code): Slider widget for phase (0 to 2π). Auto-publishes to bus.
    Cell 3 (Code): LiveView trait returning LiveContent::Html with an inline SVG.
    Reads phase from bus via sim::read(). Draws a full sine wave and a full cosine wave
    on the same SVG axes with grid lines, axis labels, and a vertical phase indicator.
    Make it beautiful — use smooth curves (SVG path with cubic beziers or polyline),
    color-coded sine (blue) vs cosine (red), legend, and a subtle animated feel.
    Include description and tags fields in the notebook JSON.

- id: T-002
  title: "Game of Life Gosper Glider Gun — Simulation + LiveView stats"
  priority: 1
  status: done
  notes: >
    New notebook: game-of-life-glider-gun.ironpad (keep existing game-of-life.ironpad as-is).
    Cell 1 (Markdown): Intro explaining Conway's Game of Life, the Gosper Glider Gun,
    and why it was historically significant (first finite pattern with unbounded growth).
    Cell 2 (Code): Simulation trait on a ~80×60 grid. Initialize with the Gosper Glider Gun
    pattern at a reasonable position. Classic B3/S23 rules. Render with Canvas.
    Publish generation count and live cell count to bus.
    Cell 3 (Code): LiveView HTML dashboard reading generation/cell count from bus.
    Show generation number, live cell count, a mini sparkline or bar of recent counts,
    and maybe the glider gun's period (30 generations).
    30 FPS simulation, 10 FPS live view. Include description and tags fields in the notebook JSON.

- id: T-003
  title: "Enhance Double Pendulum — add sliders + LiveView energy dashboard"
  priority: 1
  status: done
  notes: >
    Edit existing double-pendulum.ironpad (currently 2 cells: Markdown + Simulation).
    Add Cell 2 (before simulation): Sliders for L1, L2 (lengths, 50–200), M1, M2 (masses, 1–10),
    and initial angles θ1, θ2 (0–360°). Publish to bus.
    Modify existing simulation cell to read parameters from bus (with sensible defaults if no bus data).
    Add Cell 4 (after simulation): LiveView HTML dashboard showing:
    - Current kinetic energy, potential energy, total energy
    - Angular velocities ω1, ω2
    - A visual energy conservation indicator (how much total E has drifted)
    Publish energy data from simulation to bus for the dashboard to read.
    Update the notebook's description and tags fields if needed.

- id: T-004
  title: "Fractal Tree — LiveView SVG with sliders"
  priority: 1
  status: done
  notes: >
    New notebook: fractal-tree.ironpad.
    Cell 1 (Markdown): Intro explaining recursive fractal trees, L-systems, self-similarity.
    Cell 2 (Code): Sliders for branch angle (10–80°), recursion depth (2–12),
    length ratio (0.5–0.85), and optionally a wind/sway parameter.
    Cell 3 (Code): LiveView SVG. Reads slider values from bus. Draws a recursive binary tree
    using SVG lines/paths. Color gradient from brown (trunk) to green (leaves).
    Branch thickness decreases with depth. If wind parameter exists, add slight
    angular perturbation that varies by depth. Make it gorgeous.
    Include description and tags fields in the notebook JSON.

- id: T-005
  title: "Sorting Visualizer — Simulation canvas + LiveView stats"
  priority: 1
  status: done
  notes: >
    New notebook: sorting-visualizer.ironpad.
    Cell 1 (Markdown): Intro explaining comparison-based sorting, O(n log n) bounds,
    and the algorithms being visualized.
    Cell 2 (Code): Simulation trait. Maintain an array of ~100 values (random shuffle).
    Each tick performs one or a few steps of the sorting algorithm (e.g., quicksort with
    visible partition/pivot highlighting, or merge sort with merge highlighting).
    Render as vertical bars with color coding: default, comparing, swapping, sorted.
    Publish comparisons count, swaps count, and algorithm phase to bus.
    Cell 3 (Code): LiveView HTML dashboard. Show comparisons, swaps, current algorithm
    phase/step, and maybe a progress percentage. Style it like a control panel.
    Target 30 FPS. When sort completes, show a celebration (green sweep).
    Include description and tags fields in the notebook JSON.

- id: T-006
  title: "Fourier Series Visualizer — LiveView SVG with epicycles"
  priority: 2
  status: done
  notes: >
    New notebook: fourier-series.ironpad.
    Cell 1 (Markdown): Intro explaining Fourier series, how any periodic function can be
    decomposed into sine/cosine harmonics. KaTeX equations for the series.
    Cell 2 (Code): Slider for number of terms (1–20), and a dropdown or slider to select
    target waveform (square, sawtooth, triangle). Publish to bus.
    Cell 3 (Code): LiveView SVG. Left side: animated epicycle visualization — circles
    rotating at harmonic frequencies, with the tip tracing out the approximated waveform.
    Right side: the resulting waveform plotted against the ideal target waveform.
    Show how adding more terms improves the approximation. Color each harmonic differently.
    This is a Simulation (canvas or SVG via LiveView) because it animates over time.
    Use Simulation for the animation tick (epicircle rotation) and publish the current
    waveform data; or do it all as a LiveView with an internal phase counter.
    Include description and tags fields in the notebook JSON.

- id: T-007
  title: "Lorenz Attractor — Simulation + LiveView coordinates"
  priority: 1
  status: done
  notes: >
    New notebook: lorenz-attractor.ironpad.
    Cell 1 (Markdown): Intro explaining the Lorenz system, deterministic chaos, sensitivity
    to initial conditions, and the butterfly shape. KaTeX for the three ODEs.
    Cell 2 (Code): Sliders for σ (sigma, 1–30, default 10), ρ (rho, 1–50, default 28),
    β (beta, 0.1–10, default 8/3). Publish to bus.
    Cell 3 (Code): Simulation trait. RK4 integration of Lorenz equations with multiple
    sub-steps per frame. Maintain a trail buffer (~2000 points). Project 3D→2D using
    a simple rotation/perspective. Render trail with fading opacity (oldest = faint,
    newest = bright). Color by z-value or speed. Read σ, ρ, β from bus.
    Publish current (x,y,z) and max Lyapunov exponent estimate to bus.
    Cell 4 (Code): LiveView HTML dashboard. Show current coordinates, parameter values,
    a phase-space indicator, and trail length. Style with a dark sci-fi aesthetic.
    60 FPS simulation, 10 FPS dashboard. Include description and tags fields in the notebook JSON.

- id: T-008
  title: "Spring-Mass-Damper System — Simulation + LiveView"
  priority: 2
  status: done
  notes: >
    New notebook: spring-mass-damper.ironpad.
    Cell 1 (Markdown): Intro explaining the spring-mass-damper system, SHM, damping ratios
    (underdamped, critically damped, overdamped). KaTeX for mẍ + cẋ + kx = F(t).
    Cell 2 (Code): Sliders for mass m (0.1–10 kg), spring constant k (1–100 N/m),
    damping coefficient c (0–20 Ns/m), and an optional forcing amplitude/frequency.
    Checkbox for external forcing on/off. Publish to bus.
    Cell 3 (Code): Simulation trait. Animate the spring-mass system visually on canvas:
    draw the spring as a zigzag, the mass as a block, the damper as a dashpot symbol,
    and the wall anchor. Show the displacement over time as a trailing waveform below.
    Read parameters from bus. Publish displacement, velocity, energy to bus.
    Cell 4 (Code): LiveView HTML dashboard. Show current displacement, velocity,
    kinetic/potential/total energy, damping ratio ζ = c/(2√(mk)), natural frequency
    ωn = √(k/m), and classify the regime (underdamped/critical/overdamped).
    30 FPS simulation, 10 FPS dashboard. Include description and tags fields in the notebook JSON.

- id: T-009
  title: "Eliminate index.json — enumerate .ironpad files at runtime"
  priority: 0
  status: done
  notes: >
    Remove the static index.json and derive the public notebook listing from the files themselves.
    Steps:
    1. Add optional `description: Option<String>` and `tags: Option<Vec<String>>` fields to
       `IronpadNotebook` in `crates/ironpad-common/src/types.rs` (with `#[serde(default, skip_serializing_if)]`).
    2. Backfill `description` and `tags` into every existing `.ironpad` file in `public/notebooks/`
       using the values currently in `index.json`.
    3. Rewrite `list_public_notebooks()` in `crates/ironpad-app/src/server_fns.rs` to enumerate
       `*.ironpad` files in the notebooks directory, parse each one, and build `PublicNotebookSummary`
       from the notebook's own fields (title, description, tags, cells.len()). Sort alphabetically by title.
    4. Remove `PublicNotebookIndex` struct from types.rs. `PublicNotebookSummary` stays (it's the API shape).
    5. Delete `public/notebooks/index.json`.
    6. Update the schema conformance test in types.rs — remove the index.json validation block;
       the test should still validate every .ironpad file's schema but no longer check cross-references
       with an index file.
    7. Make sure `description` and `tags` default gracefully (empty string / empty vec) for notebooks
       that omit them (e.g., user-created private notebooks).
    This MUST run before T-001..T-008 so those tasks don't need to touch index.json at all.

---

# Summary

Create 7 new showcase public notebooks (and enhance 1 existing one) that demonstrate ironpad's full interactive capabilities: Simulation trait for real-time canvas animations, LiveView trait for reactive HTML/SVG dashboards, widget sliders for parameter control, simulation bus for cross-cell communication, and KaTeX-rendered equations in markdown cells.

---

# Problem

The current public notebook collection covers basics well but doesn't fully showcase ironpad's newest features: LiveView SVG/HTML output, simulation bus coordination, and interactive widget-driven simulations. Adding visually impressive, physics-rich notebooks will delight users and serve as living documentation of what's possible.

---

# Goals

1. Demonstrate LiveView SVG (sine phase explorer, fractal tree, Fourier epicycles)
2. Demonstrate Simulation + LiveView HTML coordination (Game of Life, sorting, Lorenz, spring-mass-damper)
3. Enhance existing double pendulum with interactive parameter control and energy dashboard
4. Each notebook is self-contained, educational (with KaTeX math), and visually stunning
5. All notebooks registered in index.json with accurate metadata

---

# Technical Approach

Each notebook follows the pattern established by nuclear-reactor.ironpad:

```
[Markdown intro with KaTeX] → [Widget sliders] → [Simulation/LiveView core] → [LiveView dashboard]
```

**Simulation Bus** coordinates data flow: widget cells auto-publish slider values; simulation cells read parameters and publish computed state; LiveView cells read state and render dashboards.

**LiveView SVG** (T-001, T-004, T-006): The `LiveView` trait's `tick()` returns `LiveContent::Html(String)` containing inline `<svg>` markup. This allows fully reactive vector graphics driven by slider input.

**Simulation + LiveView** (T-002, T-005, T-007, T-008): Simulation renders to canvas at high FPS; a separate LiveView cell reads published bus data and renders an HTML stats dashboard at lower FPS.

**Physics integrators**: RK4 for ODEs (Lorenz, double pendulum, spring-mass-damper), direct grid rules for cellular automata (Game of Life).

All notebooks use only `std` library + built-in ironpad types (Canvas, Simulation, LiveView, LiveContent, Plot, ui::*, sim::*).

---

# Assumptions

- LiveView trait, Simulation trait, and simulation bus are all working (PRD-0018, PRD-0019 complete)
- KaTeX rendering works in markdown cells
- Slider/widget auto-publish to simulation bus is functional
- The `shared_cargo_toml` with `opt-level = 1` is sufficient for all notebooks

---

# Constraints

- No external crate dependencies — all notebooks use only std + ironpad built-ins
- Notebooks must be valid `.ironpad` JSON
- SVG rendering in LiveView is string-based (no DOM manipulation, just returned HTML strings)
- Canvas resolution should stay reasonable for performance (~800×600 max for simulations)
- Each notebook should load and compile in reasonable time

---

# References to Code

- `public/notebooks/nuclear-reactor.ironpad` — canonical example of Simulation + LiveView + widgets + bus
- `public/notebooks/double-pendulum.ironpad` — existing notebook to enhance (T-003)
- `public/notebooks/game-of-life.ironpad` — existing GoL (keep as-is, new one is separate)
- `public/notebooks/index.json` — notebook registry
- `crates/ironpad-cell/src/lib.rs` — Simulation trait, LiveView trait, Canvas, LiveContent, widget types
- `crates/ironpad-app/src/compiler/scaffold.rs` — how traits are detected and scaffolded

---

# Non-Goals (MVP)

- 3D rendering or WebGL integration
- User-editable initial conditions via click-to-place (would need mouse event support)
- Sound/audio output
- Multi-algorithm comparison in sorting visualizer (one algorithm is fine)
- Saving/loading simulation state

---

# History

## 2026-03-17 — Full Execution (T-009, T-001–T-008)
- **Tasks completed**: T-009, T-001, T-002, T-003, T-004, T-005, T-006, T-007, T-008
- **Changes**:
  - T-009: Eliminated index.json — added description/tags to IronpadNotebook, rewrote list_public_notebooks() to enumerate files, backfilled all existing notebooks, updated schema test
  - T-001: Created sine-phase-explorer.ironpad — LiveView SVG with phase slider, dual sine/cosine waves
  - T-002: Created game-of-life-glider-gun.ironpad — Simulation with Gosper Glider Gun + LiveView stats dashboard
  - T-003: Enhanced double-pendulum.ironpad — added 6 parameter sliders + LiveView energy dashboard
  - T-004: Created fractal-tree.ironpad — LiveView SVG recursive tree with angle/depth/ratio/wind sliders
  - T-005: Created sorting-visualizer.ironpad — Quicksort Simulation with color-coded bars + LiveView stats
  - T-006: Created fourier-series.ironpad — Simulation with animated epicycles, waveform overlay, 3 waveform types
  - T-007: Created lorenz-attractor.ironpad — Simulation with RK4 integration, spectral trail rendering + LiveView dashboard
  - T-008: Created spring-mass-damper.ironpad — Simulation with spring/damper visual + waveform + LiveView regime dashboard
- **Test results**: CI passes (349 tests, clippy clean, fmt clean)
- **UATs verified**: uat-001 (all .ironpad valid JSON), uat-002 (no index.json needed)
- **Constitution compliance**: No violations

---
