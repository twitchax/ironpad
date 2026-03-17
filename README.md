# ironpad — Interactive Rust Notebooks

[![Build and Test](https://github.com/twitchax/ironpad/actions/workflows/build.yml/badge.svg)](https://github.com/twitchax/ironpad/actions/workflows/build.yml)
[![codecov](https://codecov.io/gh/twitchax/ironpad/graph/badge.svg?token=PLACEHOLDER)](https://codecov.io/gh/twitchax/ironpad)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

An interactive Rust notebook environment that compiles cells to WebAssembly and executes them in the browser ([live playground](https://ironpad.twitchax.com)).

## Try It Now

Visit [ironpad.twitchax.com](https://ironpad.twitchax.com) to start writing Rust notebooks immediately — no install required. Explore the public showcase notebooks, create your own, and share them via link.

## Features

- **Interactive Rust notebooks** — write and iterate on Rust code in a browser-based notebook environment with a Monaco editor
- **WebAssembly execution** — cells compile to WASM and run entirely in the browser
- **Rich output** — HTML, Canvas, animations, interactive widgets, and real-time simulations
- **Data piping** — pass bincode-serialized data between cells
- **Public showcase notebooks** — Fourier series, nuclear reactor sim, double pendulum, Game of Life, ray marching, sorting visualizer, and [many more](https://ironpad.twitchax.com)
- **Shareable notebooks** — share any notebook via content-addressed links
- **AI agent collaboration** — real-time co-editing via WebSocket + CLI daemon
- **Self-hostable** — run your own instance with a single Docker command

## Quick Start

The fastest way to run ironpad is with Docker:

```bash
docker run -p 3111:3111 ghcr.io/twitchax/ironpad:latest
```

Then open [http://localhost:3111](http://localhost:3111) in your browser.

For persistent data (notebooks and compiled cell cache):

```bash
docker run -p 3111:3111 \
  -v ironpad-data:/data \
  -v ironpad-cache:/cache \
  ghcr.io/twitchax/ironpad:latest
```

## Development

Prerequisites: Rust 1.93+, Node.js 18+, and the `wasm32-unknown-unknown` target.

```bash
cargo make install-tools   # install dev tools
cargo make dev             # start dev server at http://localhost:3111
cargo make ci              # fmt-check + clippy + tests
cargo make uat             # full validation gate
```

See [DEVELOPMENT.md](DEVELOPMENT.md) for the full development guide, architecture details, and contribution workflow.

## License

MIT
