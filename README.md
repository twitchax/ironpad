[![Build and Test](https://github.com/twitchax/ironpad/actions/workflows/build.yml/badge.svg)](https://github.com/twitchax/ironpad/actions/workflows/build.yml)
[![codecov](https://codecov.io/gh/twitchax/ironpad/graph/badge.svg)](https://codecov.io/gh/twitchax/ironpad)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

# ironpad

An interactive Rust notebook environment that compiles cells to WebAssembly and executes them entirely in the browser ([live playground](https://ironpad.twitchax.com)).

## Features

- **Interactive Rust notebooks** — write and iterate on Rust code in a browser-based notebook environment with a Monaco editor
- **WebAssembly execution** — cells compile to WASM and run entirely in the browser; no server-side execution
- **Rich output** — HTML, Canvas, animations, interactive widgets, and real-time simulations
- **Data piping** — pass bincode-serialized data between cells with typed `cell0`, `cell1`, ... variables
- **45 showcase notebooks** — Fourier series, nuclear reactor sim, double pendulum, Game of Life, ray marching, sorting visualizer, and [many more](https://ironpad.twitchax.com)
- **Shareable notebooks** — share any notebook via immutable content-addressed links, or publish a mutable share you can push updates to
- **AI agent collaboration** — real-time co-editing via WebSocket relay + CLI daemon
- **Self-hostable** — run your own instance with a single Docker command

## Quick Start

### Docker

```bash
docker run -p 3111:3111 ghcr.io/twitchax/ironpad:latest
```

Then open [http://localhost:3111](http://localhost:3111).

For persistent data (notebooks and compiled cell cache):

```bash
docker run -p 3111:3111 \
  -v ironpad:/ironpad \
  ghcr.io/twitchax/ironpad:latest
```

The image exposes a single `/ironpad` volume (notebooks live in `/ironpad/data`, the compiled cell cache in `/ironpad/cache`), so one named volume persists both.

### From Source

Prerequisites: Rust 1.93+, Node.js 18+, and `wasm32-unknown-unknown`.

```bash
cargo make install-tools   # install dev tools + wasm target
cargo make dev             # start dev server at http://localhost:3111
```

## Showcase Notebooks

Every notebook below is available in the [live playground](https://ironpad.twitchax.com) and ships as a static `.ironpad` file in [`public/notebooks/`](public/notebooks/).

| Category                     | Notebooks                                                                                                   |
| ---------------------------- | ----------------------------------------------------------------------------------------------------------- |
| **Fractals & Visualization** | Mandelbrot, Julia Set, Sierpinski Triangle, Fractal Tree, Ray Marching, Particle System                     |
| **Simulations**              | N-Body, Double Pendulum, Nuclear Reactor, Game of Life (+ Glider Gun), Lorenz Attractor, Spring-Mass-Damper |
| **PDEs & Math**              | Heat Equation, Wave Equation, Fourier Series, Sine Phase Explorer                                           |
| **Interactive**              | Buttons & Widgets, Progress Bar, Charts with Plotters, Sorting Visualizer, Async HTTP                       |
| **Cellular Automata**        | Langton's Ant, Rule 110, Maze Generator                                                                     |
| **Data & Utilities**         | Working with JSON, Shared Code                                                                              |

## Embedding

Any shared or public notebook drops into a blog post or docs page as a **live, runnable embed**: readers get the full notebook (Monaco, cell execution, rendered output) inside an iframe, not a screenshot. Two snippet styles, both copyable from the **Embed** button on any notebook's view page:

```html
<!-- One line, auto-resizing -->
<script src="https://ironpad.twitchax.com/embed.js"
        data-notebook="public/mandelbrot.ironpad" async></script>

<!-- Or a plain iframe, if you'd rather own the sizing -->
<iframe src="https://ironpad.twitchax.com/embed/public/mandelbrot.ironpad"
        style="width:100%;border:0;" height="600" loading="lazy"></iframe>
```

The script variant scans for `.ironpad-embed` placeholder divs too, so one script tag can mount any number of notebooks. Embeds are **click-to-run by default**; add `data-autorun` to the script or placeholder to run cells on load (honored only for public notebooks: shared notebooks are arbitrary user content and never auto-execute, anywhere). One honest limitation: **threaded (rayon) cells can't run inside a cross-origin embed** (they need `SharedArrayBuffer`, which requires a cross-origin-isolated page); plain and async cells run fine, and the embed says so rather than failing quietly.

## How It Works

ironpad compiles each cell into a standalone WASM module via a 5-stage pipeline:

1. **Scaffold** — generates a micro-crate from user source + `Cargo.toml` + `ironpad-cell` runtime
2. **Cache** — blake3 hash lookup; identical cells compile once
3. **Build** — `cargo build --target wasm32-unknown-unknown --release`
4. **Diagnostics** — parses rustc JSON output and maps errors back to user source lines
5. **Optimize** — best-effort `wasm-opt -O3` (binaryen; runtime speed over size)

The compiled WASM is sent to the browser, instantiated, and executed — all output (text, HTML, Canvas) renders inline.

### Cell I/O

Cells can pipe data to downstream cells using bincode serialization.  Each cell's output is available to subsequent cells as typed variables (`cell0`, `cell1`, ..., `last`):

```rust
// Cell 0: produce data.
let data = vec![1, 2, 3, 4, 5];
data
```

```rust
// Cell 1: consume data from cell 0.
let numbers: Vec<i32> = cell0;
let sum: i32 = numbers.iter().sum();

CellOutput::text(format!("Sum: {sum}"))
```

## CLI Usage

The `ironpad-cli` binary connects to a running ironpad server as an AI agent collaborator.  It maintains a warm WebSocket connection via a background daemon for fast, low-latency interactions.

### Connect to a Session

```bash
$ ironpad-cli --host ws://localhost:3111 --token <session-token> status

Status
  Daemon:     running (pid 12345)
  Connection: connected
  Notebook:   "My Notebook" (3 cells)
```

### Manage Cells

```bash
# List all cells.
$ ironpad-cli cells list

# Get a cell's source.
$ ironpad-cli cells get <cell-id>

# Add a new cell.
$ ironpad-cli cells add --source 'let x = 42; x' --label "answer"

# Update a cell.
$ ironpad-cli cells update <cell-id> --source 'let x = 43; x'

# Delete a cell.
$ ironpad-cli cells delete <cell-id>

# Reorder cells.
$ ironpad-cli cells reorder <cell-id-1> <cell-id-2> <cell-id-3>
```

### Agent Architecture

```
Browser (model) ←→ WS ←→ API Server (relay) ←→ WS ←→ CLI Daemon ←→ Agent
```

The browser is the authoritative model server.  The API server is a **stateless message relay**: it holds only ephemeral session and token state, never notebook content.  The CLI daemon caches notebook state locally so read queries resolve instantly without a network round-trip.

## Architecture

```
crates/
  ironpad-app/       # Core: compiler pipeline, UI components, storage, model
  ironpad-server/    # Axum HTTP server + WebSocket relay + session management
  ironpad-proxy/     # Domain-filtering forward proxy (sandboxes cell-compile network access)
  ironpad-cli/       # CLI daemon + subcommands for agent collaboration
  ironpad-frontend/  # WASM hydration entry point (Leptos)
  ironpad-common/    # Shared types (IronpadNotebook, protocol messages)
  ironpad-cell/      # Cell runtime (injected into every compiled cell)
```

**Frontend**: Leptos 0.8 with SSR + WASM hydration, Monaco editor, IndexedDB storage.

**Backend**: Axum 0.8 with Tokio, WebSocket relay for real-time collaboration.

**Storage**: private notebooks in browser IndexedDB; shared notebooks via content-addressed blake3 hashes on the server.

## Development

```bash
cargo make install-tools    # install dev tools + wasm target
cargo make dev              # start dev server with hot reload
cargo make ci               # fmt-check + gen-completions-check + clippy + tests
cargo make test-integration # full compiler pipeline tests (slow)
cargo make playwright       # e2e browser tests
cargo make uat              # the one true gate: ci + integration + e2e
```

See [DEVELOPMENT.md](DEVELOPMENT.md) for the full development guide, architecture details, and contribution workflow.

## License

MIT. Third-party notices for bundled assets (Lucide icons, Monaco, KaTeX,
SortableJS, embedded fonts) are in [THIRD-PARTY.md](THIRD-PARTY.md).
