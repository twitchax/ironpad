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
| `install-tools`    | Install all required dev tools                     |
| `dev`              | Start cargo-leptos watch (dev server, live reload) |
| `build`            | Release build via cargo-leptos                     |
| `fmt`              | Auto-format all Rust code                          |
| `fmt-check`        | Check formatting (no changes)                      |
| `clippy`           | Run clippy lints (`-D warnings`)                   |
| `test`             | Unit/integration tests via cargo-nextest           |
| `test-integration` | Slow tests (requires wasm32 target)                |
| `ci`               | fmt-check + clippy + test                          |
| `playwright`       | Run Playwright e2e tests                           |
| `uat`              | ci + test-integration + playwright                 |
| `docker-build`     | Build Docker image                                 |
| `docker-up`        | Start container via docker-compose                 |
| `docker-down`      | Stop container                                     |
| `docker-uat`       | Build, start, run Playwright, tear down            |

---

## Architecture Overview

ironpad is a Cargo workspace with 5 crates:

| Crate                | Role                                                                                                                     |
| -------------------- | ------------------------------------------------------------------------------------------------------------------------ |
| **ironpad-app**      | Core crate — compiler pipeline, Leptos UI components, client-side storage (IndexedDB), server functions                  |
| **ironpad-server**   | Axum HTTP server entry point (minimal — `main.rs` + `config.rs`)                                                         |
| **ironpad-frontend** | WASM hydration entry point (minimal — sets up client-side Leptos)                                                        |
| **ironpad-common**   | Shared types: `CompileRequest`, `CompileResponse`, `IronpadNotebook`, `PublicNotebookSummary`, `Diagnostic`, `AppConfig` |
| **ironpad-cell**     | Cell runtime injected into every compiled cell — `CellOutput`, `DisplayPanel`, `From` impls, FFI exports                 |

```
crates/
  ironpad-app/          # Core: compiler, UI, storage, pages
  ironpad-server/       # HTTP server entry
  ironpad-frontend/     # WASM hydration entry
  ironpad-common/       # Shared types
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

## TODO / Future Ideas

- **CI / code coverage / docker**: Set up CI pipeline with GitHub Actions, code coverage reporting, and Docker image publishing
- **Publish to fly.io**: Deploy to fly.io for easy public access and sharing
- **Cell drag-and-drop reordering**: Visual reordering via drag handles
- **Light mode toggle**: Alternative light theme for Monaco + UI
- **Notebook tagging/filtering**: Tags on notebooks for organization, search/filter on home page
- **Progress bars widget**: Visual progress for long executions
- **Auto Run**: Automatically run cells on load or after edits (with debounce)
- **LSP integration**: Full rust-analyzer completions in Monaco (per-cell analysis)
- **Collaboration**: Real-time multi-user editing via WebSocket
