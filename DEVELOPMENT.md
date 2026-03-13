# Development Guide

This guide covers everything you need to contribute to ironpad. For a detailed technical reference (project layout, key types, caching strategy, CLI flags, troubleshooting), see [README.md](README.md).

---

## Getting Started

### Prerequisites

- **Rust** 1.93+ (nightly toolchain)
- **Node.js** 18+ (for Monaco editor + Playwright)
- **wasm32-unknown-unknown** target: `rustup target add wasm32-unknown-unknown`
- **LLVM tools** (for `rust-lld`): `rustup component add llvm-tools-preview`

### Quick Start

```bash
# Install dev tools (cargo-leptos, cargo-nextest, cargo-make, Playwright)
cargo make install-tools

# Start development server with hot reload (http://localhost:3111)
cargo make dev

# Run CI locally (formatting + clippy + tests)
cargo make ci

# Full validation gate (CI + integration tests + Playwright)
cargo make uat
```

### All cargo-make Tasks

| Task               | Purpose                                            |
| ------------------ | -------------------------------------------------- |
| `install-tools`    | Install all required dev tools + wasm target       |
| `dev`              | Start cargo-leptos watch (dev server, live reload) |
| `build`            | Release build via cargo-leptos                     |
| `build-cli`        | Build ironpad-cli binary (release)                 |
| `fmt`              | Auto-format all Rust code                          |
| `fmt-check`        | Check formatting (no changes)                      |
| `clippy`           | Run clippy lints (`-D warnings`)                   |
| `test`             | Unit/integration tests via cargo-nextest           |
| `test-integration` | Slow tests (requires wasm32 target)                |
| `ci`               | fmt-check + clippy + test                          |
| `playwright`       | Build CLI + run Playwright e2e tests               |
| `uat`              | ci + test-integration + playwright                 |
| `docker-build`     | Build Docker image                                 |
| `docker-up`        | Start container via docker-compose                 |
| `docker-down`      | Stop container                                     |
| `docker-uat`       | Build, start, run Playwright, tear down            |

---

## Architecture Overview

ironpad is a Cargo workspace with 6 crates:

| Crate                | Role                                                                                                                     |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| **ironpad-app**      | Core crate — compiler pipeline, Leptos UI components, notebook model, session management, client-side storage            |
| **ironpad-server**   | Axum HTTP server — Leptos SSR, WebSocket relay for agent collaboration, session/token management                         |
| **ironpad-frontend** | WASM hydration entry point (minimal — sets up client-side Leptos)                                                        |
| **ironpad-common**   | Shared types: `CompileRequest`, `IronpadNotebook`, `Diagnostic`, `AppConfig`, collaboration protocol (`protocol.rs`)     |
| **ironpad-cell**     | Cell runtime injected into every compiled cell — `CellOutput`, `DisplayPanel`, `From` impls, FFI exports                 |
| **ironpad-cli**      | CLI daemon + agent commands for programmatic notebook interaction via WebSocket                                           |

```
crates/
  ironpad-app/          # Core: compiler, UI, storage, pages, model, session
  ironpad-cli/          # CLI daemon + agent commands
  ironpad-server/       # HTTP server + WebSocket relay
  ironpad-frontend/     # WASM hydration entry
  ironpad-common/       # Shared types + collaboration protocol
  ironpad-cell/         # Cell runtime (injected into every cell)
```

For a detailed file-by-file breakdown and project tree, see [README.md § Project Layout](README.md#project-layout).

---

## Compilation Pipeline

Each cell goes through a multi-stage WASM compilation pipeline:

```
scaffold → cache check → cargo build → diagnostics → wasm-opt
```

1. **Scaffold** (`compiler/scaffold.rs`) — Generates a micro-crate that wraps user code in a `cell_main` FFI function. Injects `ironpad-cell` as a path dependency and adds the ironpad prelude.

2. **Cache Check** (`compiler/cache.rs`) — Computes a blake3 content hash over (source ‖ Cargo.toml ‖ previous cell types ‖ shared deps). On cache hit, skips compilation entirely.

3. **Build** (`compiler/build.rs`) — Runs `cargo build --target wasm32-unknown-unknown --release` with JSON message output and a 30-second timeout.

4. **Diagnostics** (`compiler/diagnostics.rs`) — Parses rustc JSON output and adjusts line numbers by subtracting `WRAPPER_PREAMBLE_LINES` (4) to map errors back to user source.

5. **Optimize** (`compiler/optimize.rs`) — Best-effort `wasm-opt -Oz` (binaryen). Failures are non-fatal.

### Key Details

- **Typed injection**: Types from previous cells are injected as typed variables into the scaffold, enabling inter-cell data flow.
- **Content hash inputs**: source + dependencies + previous cell types + shared deps — any change triggers recompilation.
- **Cell I/O**: Cells communicate via bincode 2.0 serialized data piped through WASM memory.

---

## Frontend Architecture

- **Leptos 0.8** with SSR + WASM hydration — server renders HTML, client hydrates into a reactive SPA.
- **Monaco editor** with a custom dark theme, Rust syntax highlighting, and inline diagnostic markers. Loaded from `public/monaco/` via AMD loader.
- **Cell execution** runs entirely in the browser — compiled WASM modules are loaded and invoked via `public/executor.js`, with FFI-based memory management for I/O piping.

### Client-Side APIs

| Namespace                  | Purpose                                                         |
| -------------------------- | --------------------------------------------------------------- |
| `window.IronpadMonaco.*`   | Monaco editor JS bridge (create, get/set content, set markers)  |
| `window.IronpadExecutor.*` | WASM executor (load module, execute `cell_main`, manage memory) |
| `window.IronpadStorage.*`  | IndexedDB storage (notebook CRUD, from `public/storage.js`)     |

Feature flags split `ironpad-app` between server (`ssr`) and client (`hydrate`) code paths.

---

## Agent Collaboration

ironpad supports real-time collaboration between a human user in the browser and AI agents connected via CLI.

### Architecture

```
Browser (model server) ←→ WebSocket ←→ API Server (relay) ←→ WebSocket ←→ CLI Daemon ←→ Agent
```

- The **browser** owns the notebook state (IndexedDB). It is the authoritative model server.
- The **API server** relays messages and enforces session/token-based access control.
- The **CLI daemon** maintains a warm WebSocket connection and caches notebook state for fast reads.

### Quick Start

```bash
# 1. Start the dev server
cargo make dev

# 2. Open a notebook in the browser and click "Start Agent Session"
# 3. Copy the token

# 4. In another terminal, start the CLI daemon
cargo make build-cli
./target/release/ironpad-cli --host ws://localhost:3111 --token <TOKEN> daemon

# 5. In a third terminal, interact with the notebook
./target/release/ironpad-cli cells list
./target/release/ironpad-cli cells add --source 'let x = 42;' --label "My Cell"
./target/release/ironpad-cli cells update <CELL_ID> --source 'let x = 99;'
```

### WebSocket Routes

| Route | Purpose |
|-------|---------|
| `GET /ws/host?notebook_id=<id>` | Browser connects as session host |
| `GET /ws/connect?token=<token>` | CLI connects as session guest |

### CLI Environment Variables

| Variable | Purpose |
|----------|---------|
| `IRONPAD_HOST` | Server WebSocket URL (e.g. `ws://localhost:3111`) |
| `IRONPAD_TOKEN` | Session token |

---

## TODO / Future Ideas

- **CI / code coverage / docker**: Set up CI pipeline with GitHub Actions, code coverage reporting, and Docker image publishing
- **Publish to fly.io**: Deploy to fly.io for easy public access and sharing
- **Notebook tagging/filtering**: Tags on notebooks for organization, search/filter on home page
- **LSP integration**: Full rust-analyzer completions in Monaco (per-cell analysis)
- **Collaboration**: Real-time multi-user editing via WebSocket
