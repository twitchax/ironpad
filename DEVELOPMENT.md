# Development Guide

This guide covers everything you need to contribute to ironpad, including architecture, key types, compilation pipeline, caching strategy, CLI flags, and troubleshooting.

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

# Run CI locally (fmt-check + gen-completions-check + clippy + tests)
cargo make ci

# Full validation gate (CI + integration tests + Playwright)
cargo make uat
```

### All cargo-make Tasks

| Task                    | Purpose                                                      |
| ----------------------- | ------------------------------------------------------------ |
| `install-tools`         | Install all required dev tools + wasm target                 |
| `setup-monaco`          | Install monaco-editor from npm, copy dist to `public/monaco/` |
| `dev`                   | Start cargo-leptos watch (dev server, live reload)           |
| `build`                 | Release build via cargo-leptos                               |
| `build-cli`             | Build ironpad-cli binary (release)                           |
| `fmt`                   | Auto-format all Rust code                                    |
| `fmt-check`             | Check formatting (no changes)                                |
| `gen-completions`       | Regenerate the Monaco completions index from ironpad-cell source |
| `gen-completions-check` | Fail if the committed completions index is stale             |
| `clippy`                | Run clippy lints (`-D warnings`)                             |
| `test`                  | Unit/integration tests via cargo-nextest                     |
| `test-integration`      | Slow tests (requires wasm32 target)                          |
| `warmup-atomics`        | Pre-build std with atomics for rayon cells (one-time)        |
| `warm-prod`             | Converge a deployed instance's compile/check caches          |
| `ci`                    | fmt-check + gen-completions-check + clippy + test            |
| `playwright-install`    | Install Playwright browsers                                  |
| `playwright`            | Build CLI + run Playwright e2e tests                         |
| `uat`                   | ci + test-integration + playwright                           |
| `coverage`              | Coverage report via cargo-llvm-cov                           |
| `docker-build`          | Build Docker image                                           |
| `docker-up`             | Start container via docker-compose                           |
| `docker-down`           | Stop container                                               |
| `docker-uat`            | Build, start, run Playwright, tear down                      |

---

## Architecture Overview

ironpad is a Cargo workspace with 7 crates:

| Crate                | Role                                                                                                                 |
| -------------------- | -------------------------------------------------------------------------------------------------------------------- |
| **ironpad-app**      | Core crate — compiler pipeline, Leptos UI components, notebook model, session management, client-side storage        |
| **ironpad-server**   | Axum HTTP server — Leptos SSR, WebSocket relay for agent collaboration, session/token management                     |
| **ironpad-proxy**    | Domain-filtering forward proxy that sandboxes network access during cell compilation (PRD-0023)                      |
| **ironpad-frontend** | WASM hydration entry point (minimal — sets up client-side Leptos)                                                    |
| **ironpad-common**   | Shared types: `CompileRequest`, `IronpadNotebook`, `Diagnostic`, `AppConfig`, collaboration protocol (`protocol.rs`) |
| **ironpad-cell**     | Cell runtime injected into every compiled cell — `CellOutput`, `DisplayPanel`, `From` impls, FFI exports             |
| **ironpad-cli**      | CLI daemon + agent commands for programmatic notebook interaction via WebSocket                                      |

```
crates/
  ironpad-app/          # Core: compiler, UI, storage, pages, model, session
  ironpad-cli/          # CLI daemon + agent commands
  ironpad-server/       # HTTP server + WebSocket relay
  ironpad-proxy/        # Domain-filtering forward proxy (cell-compile network sandbox)
  ironpad-frontend/     # WASM hydration entry
  ironpad-common/       # Shared types + collaboration protocol
  ironpad-cell/         # Cell runtime (injected into every cell)
```

### Detailed Crate Reference

#### ironpad-app

The heart of the application, split between SSR server code and hydrate (client) code.

**Key modules**:
- `compiler/` — Full WASM compilation pipeline (scaffold → cache → build → diagnostics → optimize)
- `components/` — Leptos UI components (Monaco editor, executor, error panel, layout, view-only notebook, session panel, social metadata)
- `storage/` — Client-side IndexedDB bindings (wasm-bindgen to `window.IronpadStorage`)
- `pages/` — Route pages: home, notebook editor, public/shared/mutable notebook viewers, embed variants
- `session/` — Browser-side WebSocket session management for agent collaboration
- `server_fns.rs` — Leptos server functions for compilation, live checks, public notebooks, sharing, and mutable shares

#### ironpad-server

Binary that starts the Axum + Leptos SSR server, the WebSocket relay, and the social-preview endpoints.

**Files**:
- `main.rs`: Tokio runtime + route assembly (bin modules: `cache_valve.rs` boot pressure valve, `http_policy.rs` CORP/cache-control/`/share-blobs/` handler + CSP, `otel.rs` OTLP wiring)
- `config.rs`: CLI argument parsing (data dir, cache dir, port, cell path, proxy, public URL, guest limits)
- `ws.rs` / `sessions.rs` / `state.rs`: WebSocket relay, session/token store, shared server state
- `og/` / `crawl.rs`: generated social-preview cards, robots.txt + sitemap.xml (PRD-0050)

#### ironpad-frontend

Minimal crate that hydrates the Leptos app into the browser.

**Files**:
- `lib.rs` — Hydration entry point with panic hook setup

#### ironpad-common

Types used by both server and client (compile requests/responses, notebook format, diagnostics, etc.).

**Key types**:
- `CompileRequest` / `CompileResponse` — RPC contract
- `Diagnostic` / `Severity` / `Span` — Compiler diagnostics with source mapping
- `IronpadNotebook` / `IronpadCell` / `IronpadMarkdownCell` — Canonical notebook JSON format
- `PublicNotebookSummary` — Public notebook listing entry
- `ExecutionResult` — Execution output with timing
- `AppConfig` — Server configuration

#### ironpad-cell

Injected into every compiled cell as a path dependency. Provides the FFI layer for I/O.

**Key types**:
- `CellInput` — Read-only view over previous cell's output (deserialize via bincode)
- `CellOutput` — Output builder with optional display text and binary payload
- `CellResult` — FFI-compatible struct returned from `cell_main` (`#[repr(C)]`)
- `ironpad_alloc` / `ironpad_dealloc` — WASM memory management exports

### Project Layout

```
ironpad/
├── Cargo.toml                      # Workspace manifest (dependencies, profiles)
├── Makefile.toml                   # cargo-make task definitions
├── playwright.config.ts            # Playwright test config
├── package.json                    # npm: monaco-editor + @playwright/test
│
├── crates/
│   ├── ironpad-app/                # Core application crate
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs              # App root (shell + routes)
│   │       ├── server_fns.rs       # Leptos server functions (RPC endpoints)
│   │       ├── model.rs            # NotebookModel (all mutations, UI + agent)
│   │       ├── blob_cache.rs       # Client-side IndexedDB blob cache (PRD-0047)
│   │       ├── sanitize.rs         # HTML sanitization (ammonia)
│   │       ├── compiler/           # WASM compilation pipeline
│   │       │   ├── mod.rs          # Pipeline integration + tests
│   │       │   ├── scaffold.rs     # Micro-crate generation
│   │       │   ├── cache.rs        # blake3 caching
│   │       │   ├── build.rs        # cargo build invocation
│   │       │   ├── diagnostics.rs  # rustc JSON parsing
│   │       │   ├── optimize.rs     # wasm-opt
│   │       │   └── toolchain.rs    # Toolchain fingerprinting
│   │       ├── components/         # UI components
│   │       │   ├── monaco_editor.rs
│   │       │   ├── executor.rs     # WASM executor bindings
│   │       │   ├── error_panel.rs
│   │       │   ├── markdown_cell.rs
│   │       │   ├── view_only_notebook.rs  # Read-only notebook viewer
│   │       │   ├── session_panel.rs       # Agent session UI
│   │       │   ├── social_meta.rs         # Open Graph/Twitter metadata (PRD-0050)
│   │       │   └── app_layout.rs
│   │       ├── storage/            # Client-side storage
│   │       │   ├── client.rs       # IndexedDB bindings (wasm-bindgen)
│   │       │   └── validate.rs     # Notebook import validation
│   │       ├── session/            # Browser-side WebSocket session management
│   │       └── pages/              # Routes
│   │           ├── home_page.rs
│   │           ├── notebook_editor/
│   │           ├── public_notebook.rs
│   │           ├── shared_notebook.rs
│   │           ├── mutable_notebook.rs
│   │           └── embed_notebook.rs
│   │
│   ├── ironpad-server/             # HTTP server entry
│   │   ├── Cargo.toml
│   │   ├── assets/fonts/           # Embedded fonts for preview cards (PRD-0050)
│   │   └── src/
│   │       ├── main.rs             # Tokio + Axum + Leptos setup (bin mods: cache_valve, http_policy, otel)
│   │       ├── config.rs           # CLI args
│   │       ├── ws.rs               # WebSocket relay handlers
│   │       ├── sessions.rs         # Session store + token management
│   │       ├── state.rs            # Shared server state
│   │       ├── og/                 # Social-preview card renderer (PRD-0050)
│   │       └── crawl.rs            # robots.txt + sitemap.xml (PRD-0050)
│   │
│   ├── ironpad-proxy/              # Domain-filtering forward proxy
│   │   ├── Cargo.toml
│   │   └── src/main.rs             # CONNECT-only proxy with allowlist
│   │
│   ├── ironpad-cli/                # CLI daemon + agent commands
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs             # CLI subcommands
│   │       ├── daemon.rs           # WS connection, state cache
│   │       └── ipc.rs              # Unix socket IPC
│   │
│   ├── ironpad-frontend/           # WASM hydration
│   │   ├── Cargo.toml
│   │   └── src/lib.rs              # Hydration entry
│   │
│   ├── ironpad-common/             # Shared types
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── types.rs            # IronpadNotebook, CompileRequest/Response, etc.
│   │       ├── config.rs           # AppConfig
│   │       ├── protocol.rs         # Collaboration protocol messages
│   │       └── cache_key.rs        # Shared cache-key recipe + CACHE_EPOCH
│   │
│   └── ironpad-cell/               # Cell runtime (injected as dep)
│       ├── Cargo.toml
│       └── src/                    # lib.rs (CellInput/CellOutput, FFI) + feature
│                                   # modules (canvas, ui, plot, sim, gpu, http,
│                                   # blocking, timing)
│
├── docker/
│   ├── Dockerfile                  # Multi-stage build (cargo-chef)
│   ├── docker-compose.yml
│   ├── entrypoint.sh               # Seeds warmed caches into the volume
│   ├── warmup-Cargo.toml           # Cargo cache warmup (default target)
│   └── warmup-atomics-Cargo.toml   # Cargo cache warmup (atomics target)
│
├── public/
│   ├── executor-bridge.js          # window.IronpadExecutor (delegates to worker)
│   ├── executor-core.js            # Executor class + cell ABI (shared by main thread + worker)
│   ├── executor-gpu.js             # WebGPU runtime (loaded before core)
│   ├── executor-glue.js            # env import table + wasm-bindgen glue rewriting (loaded before core)
│   ├── executor-worker.js          # Web Worker entry (+ executor-worker-core.js)
│   ├── executor.js                 # Main-thread executor wrapper
│   ├── embed.js                    # Third-party embed loader (+ embed-frame.js)
│   ├── storage.js                  # IndexedDB storage API (IIFE)
│   ├── katex/                      # Math rendering for markdown cells
│   ├── prism/                      # Syntax highlighting for markdown cells
│   ├── notebooks/                  # Static public .ironpad files (no index file)
│   │   ├── welcome.ironpad
│   │   ├── tutorial.ironpad
│   │   └── ... (45 in total)
│   └── monaco/
│       ├── vs/                     # Monaco dist (copied from npm)
│       ├── init.js                 # AMD loader config
│       ├── languages.js            # Language definitions
│       ├── completions-index.json  # Generated completions (gen-completions)
│       └── bridge.js               # JS ↔ Rust FFI bridge
│
├── style/
│   └── main.scss                   # Dark theme styles
│
├── data/                           # Runtime server data ({data_dir}, not tracked)
│   ├── shares/                     # Shared notebook JSON + blobs/ snapshots
│   ├── ironpad.db/                 # Embedded SurrealDB (accounts + mutable shares, PRD-0053)
│   └── og/                         # Cached social-preview PNGs (PRD-0050)
│
├── tests/
│   └── e2e/                        # Playwright specs (home, notebook, execution,
│                                   # embed, mutable-shares, social-preview, ...)
│
├── CLAUDE.md                       # Agent guidance
└── DEVELOPMENT.md                  # This file
```

---

## Compilation Pipeline

Each cell goes through a multi-stage WASM compilation pipeline:

```
scaffold → cache check → cargo build → diagnostics → wasm-opt
```

1. **Scaffold** (`compiler/scaffold.rs`) — Generates a micro-crate that wraps user code in a `cell_main` FFI function. Injects `ironpad-cell` as a path dependency and adds the ironpad prelude.

2. **Cache Check** (`compiler/cache.rs`) — Computes a blake3 content hash over (source ‖ Cargo.toml ‖ previous cell types ‖ shared deps). On cache hit, skips compilation entirely.

3. **Build** (`compiler/build.rs`) — Runs `cargo build --target wasm32-unknown-unknown --release` with JSON message output and a 300-second timeout (override with `IRONPAD_BUILD_TIMEOUT_SECS`).

4. **Diagnostics** (`compiler/diagnostics.rs`) — Parses rustc JSON output and adjusts line numbers by subtracting `WRAPPER_PREAMBLE_LINES` (4) to map errors back to user source.

5. **Optimize** (`compiler/optimize.rs`) — Best-effort `wasm-opt -O3` (binaryen; runtime speed over size). Failures are non-fatal.

### Key Details

- **Typed injection**: Types from previous cells are injected as typed variables into the scaffold, enabling inter-cell data flow.
- **Content hash inputs**: source + dependencies + previous cell types + shared deps — any change triggers recompilation. The hash recipe (and `CACHE_EPOCH`) lives in `ironpad-common/src/cache_key.rs` so the browser computes identical keys for its local IndexedDB blob cache (PRD-0047); the server binds it to the toolchain fingerprint, the client fetches that fingerprint once per session.
- **Blob delivery (PRD-0047)**: shares snapshot their compiled blobs into `{data_dir}/shares/blobs/` at share time (cache hits only) with a per-share manifest sidecar; viewers replay from the immutable `/share-blobs/` route with zero compile calls, and the editor/viewer probe a local IndexedDB blob store before paying the `compile_cell` round trip. Force Recompile bypasses all layers and overwrites the local entry.
- **Cell I/O**: Cells communicate via bincode 2.0 serialized data piped through WASM memory.

### End-to-End Compilation Example

**User Input**:
```rust
// Cell source
let fibs: Vec<u64> = vec![0, 1, 1, 2, 3, 5];
CellOutput::new(&fibs)?.with_display(format!("{:?}", fibs)).into()

// Cell Cargo.toml
[dependencies]
serde = "1"
```

**Generated Micro-Crate** (`src/lib.rs`):
```rust
use ironpad_cell::prelude::*;

#[no_mangle]
pub extern "C" fn cell_main(input_ptr: *const u8, input_len: usize) -> CellResult {
    let fibs: Vec<u64> = vec![0, 1, 1, 2, 3, 5];
    CellOutput::new(&fibs)?.with_display(format!("{:?}", fibs)).into()
}
```

**Compilation**:
```bash
cd {cache_dir}/workspaces/{session}/cell_id
CARGO_HOME={cache_dir}/registry \
CARGO_TARGET_DIR={cache_dir}/targets/{session} \
cargo build --target wasm32-unknown-unknown --release --message-format=json
```

**Output**:
- **Success**: `.wasm` blob at `{CARGO_TARGET_DIR}/wasm32-unknown-unknown/release/cell_*.wasm`
- **Failure**: JSON diagnostics in stdout, mapped back to user code

### WASM Compilation Target

All cells compile to **`wasm32-unknown-unknown`**:
- No WASI or browser APIs
- Self-contained binary with exports: `memory`, `ironpad_alloc`, `ironpad_dealloc`, `cell_main`
- Optimized with `wasm-opt -O3` for runtime speed (cells execute repeatedly in the browser, so size matters less)

### Caching Strategy

- **Cache key**: blake3 over source, Cargo.toml, previous cell types, shared source/Cargo.toml, and the feature-detection flags (atomics, simd, autodiff, ...), bound to a toolchain fingerprint and `CACHE_EPOCH` (recipe in `ironpad-common/src/cache_key.rs`)
- **Cache path**: `{cache_dir}/blobs/{hash}.wasm` (plus `.js` glue and `.diag.json` sidecars where applicable)
- **Hit rate**: High for deterministic user code; misses trigger full compilation (300-second timeout)

### Diagnostic Mapping

- Compiler reports spans in wrapper lines (`src/lib.rs` generated code)
- `WRAPPER_PREAMBLE_LINES = 4` hardcoded offset
- Diagnostic parser adjusts all line numbers: `user_line = rustc_line - 4`
- Error codes extracted for rust error index linking

---

## Cell I/O Pipeline

### Memory Model

Cells use **linear WASM memory** with FFI at the boundaries:

```javascript
// Public executor API (JavaScript)
loadBlob(cellId, hash, wasmBytes, jsGlue)   // Load + instantiate module
execute(cellId, inputBytes) -> result        // Run cell_main
  -> { outputBytes: Uint8Array, displayText: string | null }
```

### Cell Execution Flow

1. **Input serialization**: Previous cell's output → bincode bytes
2. **Memory allocation**: `ironpad_alloc(len)` allocates space in WASM linear memory
3. **FFI call**: `cell_main(input_ptr, input_len) -> CellResult`
4. **Output extraction**: Read CellResult struct from memory
5. **Deserialization**: Output bytes → next cell's input
6. **Memory deallocation**: `ironpad_dealloc(ptr, len)` frees all allocations

### CellResult FFI Layout (`#[repr(C)]`)

```rust
pub struct CellResult {
    pub output_ptr: *mut u8,      // offset 0
    pub output_len: usize,         // offset 4/8
    pub display_ptr: *mut u8,      // offset 8/16
    pub display_len: usize,        // offset 12/24
}
```

On wasm32, multi-return values exceeding one i32 use "sret" (structural return) convention:
- 3+ parameters → `cell_main(retptr, input_ptr, input_len) -> void`
- 2 parameters → `cell_main(input_ptr, input_len) -> *const CellResult`

The JS executor detects this by inspecting function arity.

### Bincode Serialization

Uses **bincode 2.0** with standard config for compact binary encoding:
```rust
let bytes = bincode::serde::encode_to_vec(&value, bincode::config::standard())?;
let decoded: T = bincode::serde::decode_from_slice(&bytes, bincode::config::standard())?;
```

---

## Rayon / Multi-Core Parallelism

ironpad supports multi-core parallelism in cells via [rayon](https://docs.rs/rayon) and [wasm-bindgen-rayon](https://github.com/RReverser/wasm-bindgen-rayon).

### How It Works

1. **Automatic detection**: When a cell's dependencies include `rayon`, the compiler pipeline automatically enables atomics support.
2. **COOP/COEP headers**: The server sends `Cross-Origin-Opener-Policy: same-origin` and `Cross-Origin-Embedder-Policy: require-corp` headers, enabling `SharedArrayBuffer` in the browser.
3. **Build flags**: Rayon cells are compiled with `RUSTFLAGS="-C target-feature=+atomics,+bulk-memory,+mutable-globals"` and use a shared target directory (`{cache_dir}/targets/atomics-shared/`).
4. **Thread pool**: After loading a rayon cell's WASM module, the executor calls `initThreadPool(navigator.hardwareConcurrency)` to spawn Web Workers for the rayon thread pool.

### Using Rayon in Cells

Add `rayon = "1"` to your cell's Cargo.toml, then use `rayon::prelude::*`:

```rust
// Cargo.toml:
// [dependencies]
// rayon = "1"

use rayon::prelude::*;

let sum: i64 = (0..1_000_000i64).into_par_iter().sum();
CellOutput::text(format!("Parallel sum: {sum}"))
```

### Local Atomics Sysroot Warmup

The first rayon cell compilation needs a pre-built std with atomics support. In Docker, this is pre-warmed during image build. For local development:

```bash
cargo make warmup-atomics
```

This one-time step takes 30-60 seconds and caches the result in `{cache_dir}/targets/atomics-shared/`.

### Architecture

```
Browser (crossOriginIsolated = true)
  └─ Web Worker (cell executor)
       └─ WASM module (compiled with +atomics)
            └─ rayon thread pool (sub-Workers via SharedArrayBuffer)
                 ├─ Thread 1
                 ├─ Thread N (navigator.hardwareConcurrency)
```

---

## Notebook Storage & Sharing

### Client-Side Storage (IndexedDB)

Private notebooks are stored in the browser's IndexedDB via `public/storage.js` (an IIFE that exposes `window.IronpadStorage`). The Rust `storage/client.rs` module provides wasm-bindgen bindings.

The server is **stateless** for private notebooks — no server-side CRUD.

### Canonical Notebook Format (`IronpadNotebook`)

Defined in `ironpad-common/src/types.rs`, the `IronpadNotebook` JSON format is used for IndexedDB storage, public `.ironpad` files, and shared notebook uploads:

```json
{
  "version": 1,
  "id": "uuid",
  "title": "My Notebook",
  "created_at": "2026-03-07T...",
  "updated_at": "2026-03-07T...",
  "cells": [
    {
      "id": "cell_0",
      "order": 0,
      "label": "Cell Label",
      "cell_type": "Code",
      "source": "let x = 42;\nCellOutput::new(&x)?.with_display(\"42\").into()",
      "cargo_toml": "[dependencies]\nserde = \"1\""
    }
  ]
}
```

Optional notebook fields (`description`, `tags`, `shared_source`, `shared_cargo_toml`, `reactive_mode`, `og_image`) and optional cell fields (`shared`, `collapsed`, `output_collapsed`) are omitted when unset; see `ironpad-common/src/types.rs` for the full definitions.

### Public Notebooks

Static `.ironpad` JSON files in `public/notebooks/` (bundled into `{site_root}/notebooks/` at build time). There is no index file: `list_public_notebooks()` enumerates `{site_root}/notebooks/*.ironpad` at runtime and reads each notebook's own `title`/`description`.

### Shared Notebooks

Upload notebook JSON via `share_notebook()` → blake3 content hash (first 16 hex chars) → stored at `{data_dir}/shares/{hash}.json`. Retrieve via `get_shared_notebook(hash)` at URL `/shared/{hash}`. Shares are immutable: editing and re-sharing mints a new hash, and old links stay frozen. At share time the server also snapshots compiled blobs for cache-hit cells into `{data_dir}/shares/blobs/` with a `{hash}.manifest.json` sidecar (PRD-0047), retrievable via `get_shared_manifest(hash)`, so viewers replay blobs instead of compiling.

### Mutable Shares (PRD-0049)

"Share Mutable" converts a private notebook into a server-backed one at `/mutable/{id}` (server-minted 16-hex id) and deletes the local copy: published notebooks live entirely on the server (PRD-0054). The record holds a published slot and a draft slot. The owner's edits autosave (debounced) to the draft; the toolbar Push button promotes draft to published and snapshots blobs at that moment; readers only ever see published, with "Published by @login" attribution. Ownership is the signed-in GitHub account's OWNER grant (PRD-0053). Server functions: `create_mutable_share`, `save_mutable_draft`, `get_mutable_for_edit` (owner-gated), `push_mutable` (promote), `discard_mutable_draft`, `get_mutable_notebook` (readers), `get_mutable_manifest`, `delete_mutable_share` (unpublish), `list_mutable_shares` (by session).

---

## Frontend Architecture

- **Leptos 0.8** with SSR + WASM hydration — server renders HTML, client hydrates into a reactive SPA.
- **Monaco editor** with a custom dark theme, Rust syntax highlighting, and inline diagnostic markers. Loaded from `public/monaco/` via AMD loader.
- **Cell execution** runs entirely in the browser: compiled WASM modules are loaded and invoked via the executor scripts in `public/` (`executor-bridge.js` exposes `window.IronpadExecutor` and delegates to a Web Worker built on `executor-gpu.js` + `executor-glue.js` + `executor-core.js`, loaded in that order; `executor.js` is the main-thread wrapper), with FFI-based memory management for I/O piping.

### Client-Side APIs

| Namespace                  | Purpose                                                         |
| -------------------------- | --------------------------------------------------------------- |
| `window.IronpadMonaco.*`   | Monaco editor JS bridge (create, get/set content, set markers)  |
| `window.IronpadExecutor.*` | WASM executor (load module, execute `cell_main`, manage memory) |
| `window.IronpadStorage.*`  | IndexedDB storage (notebook CRUD, from `public/storage.js`)     |

Feature flags split `ironpad-app` between server (`ssr`) and client (`hydrate`) code paths.

### Page Routes

```
/                              → HomePage (private + public notebook list)
/local/{id}                    → NotebookEditorPage (private, IndexedDB-backed)
/public/{name}                 → PublicNotebookPage (read-only, static .ironpad file)
/shared/{hash}                 → SharedNotebookPage (read-only, immutable, shared via hash)
/mutable/{id}                  → MutableNotebookPage (reader of published; owner's draft editor on hydrate; PRD-0054)
/embed/shared/{hash}           → EmbedSharedPage (chrome-less iframe variant; PRD-0039)
/embed/public/{filename}       → EmbedPublicPage (chrome-less iframe variant; PRD-0039)
```

Legacy `/notebook/{id}` and `/notebook/public/{filename}` paths redirect to the canonical routes. The three server-backed notebook routes (`/public`, `/shared`, `/mutable`) render with `SsrMode::Async` so crawlers see their metadata (see Social Previews below). Outside Leptos, the server also handles `/share-blobs/{file}`, `/og/{class}/{id}.png`, `/og/ironpad.png`, `/robots.txt`, `/sitemap.xml`, and the `/auth/*` sign-in routes (PRD-0053; `/auth/test-login` exists only under `IRONPAD_TEST_AUTH`) as plain axum routes.

### Key Components

#### Monaco Editor (`components/monaco_editor.rs`)

Thin Leptos wrapper around Monaco editor via JS FFI:

```rust
<MonacoEditor
    initial_value="fn main() {}"
    language="rust"
    on_change=callback
    handle=editor_handle
/>
```

- Loads Monaco from `public/monaco/` (copied from npm at build time)
- JS bridge: `IronpadMonaco` namespace with methods:
  - `create()` → editor ID
  - `getValue() / setValue()` → read/write content
  - `addAction()` → register keyboard shortcuts
  - `setMarkers() / clearMarkers()` → inline diagnostics
  - `dispose()` → cleanup
- Rust types: `MonacoEditorHandle` for imperative access

#### Cell Executor (`components/executor.rs`)

Bridges Rust and the WASM executor:

```rust
load_blob(cell_id, hash, bytes) -> Result<(), String>
execute_cell(cell_id, input_bytes) -> Result<(Vec<u8>, Option<String>), String>
```

- Client-side only (feature-gated as `#[cfg(feature = "hydrate")]`)
- FNV-1a hashing for WASM blob caching
- Calls into `window.IronpadExecutor` JS singleton

#### Error Panel (`components/error_panel.rs`)

Renders compiler diagnostics inline in the editor:
- Severity-based styling (red for error, yellow for warning)
- Clickable error codes linking to rust error index
- Spans displayed in tooltip/badge format

### Styling

CSS (SCSS) at `style/main.scss` with dark theme:
- CSS custom properties for colors, fonts, spacing
- Leptos-generated CSS module at `target/site/pkg/ironpad.css`
- Native UI primitives (`.ironpad-btn`, `.ironpad-tag`, `.ironpad-cell-tab`, toasts) styled in-repo; no component library

---

## Server & Deployment

### Server Functions

**`#[server]` functions** (in `server_fns.rs`) are RPC endpoints called from the browser:

```rust
#[server]
pub async fn compile_cell(request: CompileRequest) -> Result<CompileResponse, ServerFnError>

#[server]
pub async fn check_cell(request: CompileRequest) -> Result<CheckResponse, ServerFnError>

#[server]
pub async fn get_toolchain_fingerprint() -> Result<String, ServerFnError>

#[server]
pub async fn list_public_notebooks() -> Result<Vec<PublicNotebookSummary>, ServerFnError>

#[server]
pub async fn get_public_notebook(filename: String) -> Result<IronpadNotebook, ServerFnError>

#[server]
pub async fn share_notebook(notebook_json: String, cell_type_tags: Option<Vec<String>>) -> Result<String, ServerFnError>

#[server]
pub async fn get_shared_notebook(hash: String) -> Result<IronpadNotebook, ServerFnError>

#[server]
pub async fn get_shared_manifest(hash: String) -> Result<Option<ShareManifest>, ServerFnError>
```

`check_cell` backs live check-on-type (PRD-0045); `get_toolchain_fingerprint` feeds the client-side blob cache keys (PRD-0047). The mutable-share set (PRD-0054; writes are session-gated) adds `create_mutable_share`, `save_mutable_draft`, `get_mutable_for_edit`, `push_mutable` (draft promote), `discard_mutable_draft`, `get_mutable_notebook`, `get_mutable_manifest`, `delete_mutable_share`, and `list_mutable_shares`; `get_auth_info` (PRD-0053) feeds the header's sign-in surface.

They run on the server and are automatically serialized/called from the client.

### CLI Flags

```
--data-dir <PATH>                 (env: IRONPAD_DATA_DIR, default: ./data)
--cache-dir <PATH>                (env: IRONPAD_CACHE_DIR, default: ./cache)
--port <PORT>                     (env: IRONPAD_PORT, default: 3111)
--ironpad-cell-path <PATH>        (env: IRONPAD_CELL_PATH, default: ./crates/ironpad-cell)
--compilation-proxy <URL>         (env: IRONPAD_COMPILATION_PROXY, default: unset)
--public-url <ORIGIN>             (env: IRONPAD_PUBLIC_URL, default: http://localhost:{port})
--max-concurrent-builds <N>       (env: IRONPAD_MAX_CONCURRENT_BUILDS, default: 3)
--max-guests <N>                  (env: IRONPAD_MAX_GUESTS, default: 512)
--guest-idle-timeout-secs <SECS>  (env: IRONPAD_GUEST_IDLE_TIMEOUT_SECS, default: 1800)
```

### Docker Deployment

**Multi-stage Dockerfile** (`docker/Dockerfile`):

1. **Planner stage** (rust:1.93.0):
   - Generates a cargo-chef recipe so compiled dependencies cache in their own layer

2. **Builder stage** (rust:1.93.0):
   - Install `wasm32-unknown-unknown` target + binaryen
   - Install `cargo-leptos` + `wasm-bindgen-cli` (via cargo-binstall)
   - `cargo chef cook`, then `cargo leptos build --release` → compiles server + frontend WASM

3. **Runtime stage** (rust:1.93.0-slim):
   - Installs the three pinned nightly cell toolchains (`CELL_TOOLCHAIN`, `AUTODIFF_TOOLCHAIN` + enzyme, `ATOMICS_TOOLCHAIN`), then uninstalls the base image's stable toolchain (nothing uses it)
   - `wasm-opt` copied from the builder
   - Pre-warms the cargo registry and target dirs with ironpad-cell dependencies; `entrypoint.sh` seeds them into the volume
   - Copies the server binary + site assets, enables the compilation proxy, exposes port 3111

**docker-compose.yml** (`docker/docker-compose.yml`):
```yaml
services:
  ironpad:
    image: ironpad:latest
    build:
      context: ..
      dockerfile: docker/Dockerfile
    ports: ["3111:3111"]
    volumes:
      - ironpad:/ironpad
    environment:
      - IRONPAD_DATA_DIR=/ironpad/data
      - IRONPAD_CACHE_DIR=/ironpad/cache
      - IRONPAD_PORT=3111
      - IRONPAD_CELL_PATH=/app/crates/ironpad-cell
```

### Observability (OpenTelemetry)

The server always logs to stdout via `tracing` (level controlled by `RUST_LOG`, default `info`). OTLP **trace** export is **opt-in** and off by default — it turns on only when `OTEL_EXPORTER_OTLP_ENDPOINT` is set. A `tower-http` `TraceLayer` emits the root span per HTTP request, named by route template via `otel.name` (`POST /api/{*fn_name}`, `GET /og/{class}/{file}`) and carrying the request path only — never the query string, which holds the session token on `/ws/connect`.

Beneath the root span, the request paths that actually spend time are instrumented (`#[tracing::instrument]` at `info`, always `skip_all` — request payloads carry full user source and must never be Debug-dumped into span fields):

| Path | Spans (nested) |
| --- | --- |
| Compile | `compile_cell` → `compile_lock_wait`, `cache_lookup`, `build_permit_wait` (admission, PRD-0052), `scaffold`, `cargo_build`, `wasm_bindgen`, `wasm_opt`, `cache_store` |
| Live check | `check_cell` → `scaffold`, `cargo_check` |
| Share | `share_notebook` → `dir_size_scan`; `snapshot_share_blobs` → `snapshot_cell_blobs` → per-cell `cache_lookup` |
| Mutable shares | `create_mutable_share`, `push_mutable`, `list_mutable_shares`, `db_get_share` |
| Accounts (PRD-0053) | `auth_github`, `auth_callback`, `auth_logout`, `db_session_user`, `db_create_session` |
| Notebook loads | `list_public_notebooks`, `get_public_notebook`, `get_shared_notebook`, `get_shared_manifest` |
| OG cards | `render_card` → `render_lock_wait`, `rasterize` (entered inside the `spawn_blocking` closure so the resvg CPU time still lands under the request trace), `og_cache_evict` |

Outcome fields are recorded on the spans themselves (`cache = hit\|miss\|bypassed` on `compile_cell`, `status` on `check_cell`, `hit` on `cache_lookup`, `snapshotted` on `snapshot_cell_blobs`), so traces are filterable by result, not just by duration. `server_fns::tests::compile_pipeline_emits_stage_spans` guards the mechanism.

To export to Grafana Cloud (or any OTLP backend), set the standard env vars — the exporter reads them itself, so **no credentials live in code or config**. On Fly, use secrets so the token stays in Fly's encrypted store:

```bash
# Endpoint, instance ID, and token come from your Grafana Cloud OTLP page.
fly secrets set -c .hidden/fly.toml \
  OTEL_EXPORTER_OTLP_ENDPOINT='https://otlp-gateway-<zone>.grafana.net/otlp' \
  OTEL_EXPORTER_OTLP_HEADERS='Authorization=Basic <base64(instanceID:token)>'
```

For local testing, export the same two vars in your shell before `cargo make dev` (there is no `.env` auto-loading). TLS uses rustls with bundled webpki roots, so no system OpenSSL or cert store is required in the container. Metrics/logs export are easy follow-ons once traces are confirmed flowing.

---

## Social Previews (PRD-0050)

Every shareable URL serves per-page `<title>`, `og:*`, and `twitter:*` metadata plus a generated 1200x630 card image, so a pasted link unfurls with a real title, description, and picture on Reddit, X, Slack, and Discord.

### Metadata

The `SocialMeta` component (`ironpad-app/src/components/social_meta.rs`) emits the tag block on `/`, `/public/{name}`, `/shared/{hash}`, and `/mutable/{id}`.

**`SsrMode::Async` on the three server-backed notebook routes is load-bearing.** Their titles come from a `Resource`, and under Leptos's default out-of-order streaming the `<head>` is flushed before the resource resolves; `leptos_meta` then patches the tags in with a script. That is correct for a browser and invisible to every unfurler, since none of them run JavaScript. The tags would look right in devtools and not exist for crawlers, which is why `tests/e2e/social-preview.spec.ts` asserts against raw response bodies, never the hydrated DOM. Keep it that way.

`og:image` and `og:url` must be absolute URLs. `IRONPAD_PUBLIC_URL` (`AppConfig::public_url`, applied via `absolute_url`) supplies the origin server-side; the client falls back to `window.location.origin`.

### Card Generation

`GET /og/{class}/{id}.png` (plus `/og/ironpad.png` for the home page) renders a card from notebook metadata in `ironpad-server/src/og/`:

- `text.rs`: embedded fonts + advance-width measurement (SVG has no text wrapping, so every line is positioned explicitly)
- `svg.rs`: pure Card-to-SVG layout, unit-testable with no filesystem or rasterizer
- `mod.rs`: notebook extraction, disk cache, axum handlers

The SVG is rasterized with `resvg` and cached at `{data_dir}/og/{blake3-of-svg}.png`. The fonts (Inter + JetBrains Mono, under `crates/ironpad-server/assets/fonts/`) are embedded with `include_bytes!`, and `resvg` is built with `default-features = false, features = ["text"]` (no `system-fonts`): the runtime image is `rust:1.93.0-slim` and ships no fonts, so font discovery would work on a dev box and find nothing in prod.

Notebook text is attacker-controlled on `/shared` and `/mutable`. It is XML-escaped before it reaches the SVG, and a notebook's optional `og_image` override is forced root-relative by `IronpadNotebook::og_image_path()` so a share cannot point a crawler at another origin.

### Crawler Files

`/robots.txt` and `/sitemap.xml` are served by `ironpad-server/src/crawl.rs`. The sitemap enumerates public notebooks at request time by their canonical extension-less routes. Unlisted is not the same as blocked: `/shared` and `/mutable` pages carry a `noindex` robots meta tag rather than a `robots.txt` `Disallow`, because several unfurlers honour robots.txt and would then refuse to build a preview at all. `robots.txt` disallows only `/embed/` (duplicate content).

### Editing the Metadata (PRD-0051)

The fields the unfurl reads (`description`, `tags`, `og_image`, and the new `og_image_width`/`og_image_height`) are editable from a collapsible panel below the cell list, in `pages/notebook_editor/metadata_panel.rs`. It sits in its own `.ironpad-editor-metadata-appendix` wrapper rather than joining the shared-source appendix, because `shared-appendix.spec.ts` indexes that container positionally.

Both the panel and the agent protocol go through one struct. `NotebookMetaPatch` is `#[serde(flatten)]`-ed into both `Mutation::NotebookUpdateMeta` and `Event::NotebookMetaUpdated`, which keeps the two from drifting; `Mutation` is internally tagged, so the fields still sit beside `action` and the wire format is unchanged (regression-locked by `flattening_the_patch_left_the_wire_format_alone`). `NotebookMetaPatch::apply_to` is the single application of a patch to a notebook, used by the browser model and by the CLI daemon's cached copy.

Two things to know before touching this:

- **Every clearable field is `Option<Option<T>>`, and serde's default decode is wrong for it.** `None` means unchanged, `Some(None)` means clear, `Some(Some(v))` means set. Plain serde collapses an explicit `null` back to `None`, so a clear crossing the WebSocket arrived as "unchanged". `explicit_null_is_a_clear` fixes the decode; an absent key still means unchanged because `skip_serializing_if` keeps it off the wire entirely.
- **Validate at the point of use.** `og_image_dimensions()` follows `og_image_path()`: notebooks arrive from unauthenticated shares and from IndexedDB, so both axes must be present, both within `OG_IMAGE_MIN_PX..=OG_IMAGE_MAX_PX`, and an override image must actually exist. A declared size reserves a layout box in someone's feed before the image is fetched.

### oEmbed (PRD-0051)

`GET /oembed?url=…` (`ironpad-server/src/oembed.rs`) maps a canonical `/public` or `/shared` URL to its `/embed/*` route and returns a `rich` oEmbed response, so a consumer that supports discovery embeds the running notebook rather than the static card. The pages advertise it with a `<link rel="alternate" type="application/json+oembed">`.

It is locked to `public_url`: a provider that embedded arbitrary URLs would be an open redirect wearing an iframe, since the consumer trusts the returned HTML on the strength of trusting the provider. `/mutable` is excluded because no `/embed/mutable` route exists, and resolving it would hand back a frame pointing at a 404. Note that X, Reddit, and Slack use their own allowlists rather than discovery, so this changes nothing about how a link looks there; that is what the Open Graph tags are for.

### Content Security Policy

Every response carries `object-src 'none'; base-uri 'self'; form-action 'self'` (`CONTENT_SECURITY_POLICY` in `ironpad-server/src/http_policy.rs`).

Two deliberate omissions. There is no `script-src`, because Leptos hydration emits an inline module script and Monaco ships its own loader, so any policy today would need `'unsafe-inline'`, which permits exactly the kind of injection a CSP is meant to blunt; per-request nonces through `leptos_meta` and the Monaco bootstrap are the real fix. And there is no `frame-ancestors`, because `/embed/*` exists to be framed by third parties.

---

## Compilation Security

ironpad compiles arbitrary user-provided Rust code server-side, including user-specified `Cargo.toml` dependencies. Any dependency can include a `build.rs` script that executes during compilation with full network access. In a deployment environment (e.g., Fly.io), a malicious `build.rs` could probe internal networks, access metadata services, or exfiltrate data.

### The Proxy: `ironpad-proxy`

`ironpad-proxy` is a lightweight CONNECT-only proxy that filters outbound connections by domain during `cargo build`. When enabled, the compiler sets `HTTPS_PROXY` on the child cargo process, routing all HTTPS traffic through the proxy. Only connections to allowlisted domains are permitted; everything else gets a `403 Forbidden`.

```
cargo build  ──HTTPS_PROXY──►  ironpad-proxy (127.0.0.1:3112)
                                     │
                                     ├─ CONNECT crates.io:443  → ✅ tunnel
                                     ├─ CONNECT github.com:443 → ✅ tunnel
                                     └─ CONNECT evil.com:443   → ❌ 403
```

### Configuration

| Env Var | Purpose | Default |
|---|---|---|
| `IRONPAD_COMPILATION_PROXY` | Proxy URL (e.g., `http://127.0.0.1:3112`). Set to enable; unset to disable. | *unset* |
| `IRONPAD_PROXY_ALLOWLIST` | Comma-separated domains. Uses suffix matching: `crates.io` also allows `static.crates.io`. | *empty* (fail-closed: all connections denied) |

The Docker image sets `IRONPAD_PROXY_ALLOWLIST=crates.io,github.com,githubusercontent.com`, which covers the standard Cargo ecosystem:
- `crates.io` → `static.crates.io`, `index.crates.io`
- `github.com` → git dependencies
- `githubusercontent.com` → `raw.githubusercontent.com`, `objects.githubusercontent.com`

### Local Development

The proxy is **opt-in**. If `IRONPAD_COMPILATION_PROXY` is not set, compilation works exactly as before with no proxy involvement. You only need to configure it if you want to test proxy behavior locally:

```bash
# Terminal 1: start the proxy
cargo run -p ironpad-proxy

# Terminal 2: start the server with proxy enabled
IRONPAD_COMPILATION_PROXY=http://127.0.0.1:3112 cargo make dev
```

### Deployment

In Docker and Fly.io, the proxy runs alongside the server and is enabled by default. The Dockerfile starts `ironpad-proxy` in the background before exec-ing the main server, and sets `IRONPAD_COMPILATION_PROXY` and `IRONPAD_PROXY_ALLOWLIST` as environment variables.

### Limitations

- The proxy cannot inspect TLS traffic — it only sees the target hostname from the CONNECT request.
- Suffix matching means `github.com` also matches `evil-github.com`. The actual Cargo ecosystem only uses well-known domains, so this is acceptable for the threat model.
- `build.rs` scripts using raw TCP (not via cargo's HTTPS) bypass the proxy. The proxy secures cargo's own fetching, not arbitrary network code.

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
./target/release/ironpad-cli cells add --source 'CellOutput::text(format!("{}", 42))' --label "My Cell"
./target/release/ironpad-cli cells update <CELL_ID> --source 'let x = 99;'

# 6. Run a cell and read its result (PRD-0052)
./target/release/ironpad-cli cells run <CELL_ID>
# → {"status":"executed","cell_id":"…","display_text":"…","type_tag":"…","execution_time_ms":…}
```

### Agent-Triggered Execution (PRD-0052)

`cells run` closes the loop: an agent can edit a cell, run it in the hosting
browser, and read the output — no human click required.

- `Mutation::CellRun` rides the mutation envelope for the relay's
  write-permission gate, but it is **not** a state mutation: the browser
  intercepts it before `model.apply` and appends the cell to the same run
  queue Run All uses. Unexecuted prerequisites cascade first, exactly as they
  do for a human.
- Results are **events, not a response**: the browser emits `CellCompiling`,
  `CellCompiled { success }`, and `CellExecuted { success }` (session-gated —
  nothing is emitted without a live session, so no stale backlog flushes at
  the next session start). The daemon correlates by `cell_id`, because a
  message id cannot follow a cascade.
- Terminal outcomes reported by `cells run`: `executed`, `execution_error`,
  `compile_error`, and `prerequisite_failed` (a compile failure of ANY cell
  while waiting — the browser clears the run queue on compile failure, so the
  queued run is dead). `--no-wait` returns at the ack; `--timeout-secs`
  defaults to 360.

### Build Admission Control (PRD-0052)

The scarce resource is a cargo process, not an HTTP request — so admission is
consulted only after a confirmed cache miss, and cache hits stay free (warmed
notebooks, e2e, classrooms on the blog posts).

- **Global concurrency cap** (`--max-concurrent-builds`, default 3): compiles
  queue for a slot (bounded by `IRONPAD_BUILD_QUEUE_TIMEOUT_SECS`, default
  180s, surfaced as a clear "at capacity" error); live checks `try_acquire`
  a separate pool and degrade to `Skipped`, the status the client already
  retries — typing never blocks on capacity.
- **Per-client rate limit** on build *starts*: a token bucket keyed by
  `Fly-Client-IP` (then `X-Forwarded-For`, then a shared `local` bucket),
  `IRONPAD_BUILD_RATE_BURST` (default 20) refilling at
  `IRONPAD_BUILD_RATE_PER_MIN` (default 30) — sized for the error loop, since
  failed compiles are never cached and each debugging attempt is a miss. The
  Playwright suite raises both via the webServer env: its deliberate
  always-miss compiles share one "local" bucket and would trip production
  limits at suite scale.
- The queue wait is visible in traces as the `build_permit_wait` span.

### WebSocket Routes

| Route                           | Purpose                          |
| ------------------------------- | -------------------------------- |
| `GET /ws/host?notebook_id=<id>` | Browser connects as session host |
| `GET /ws/connect?token=<token>` | CLI connects as session guest    |

### CLI Environment Variables

| Variable        | Purpose                                           |
| --------------- | ------------------------------------------------- |
| `IRONPAD_HOST`  | Server WebSocket URL (e.g. `ws://localhost:3111`) |
| `IRONPAD_TOKEN` | Session token                                     |

---

## Troubleshooting

### `rust-lld` Linking Failures

Cell compilation targets `wasm32-unknown-unknown`, which uses `rust-lld` as its linker. If cells fail with `linking with rust-lld failed`, check:

1. **LLVM tools installed**: `rustup component add llvm-tools-preview`
2. **rust-lld exists**: `ls $(rustc --print sysroot)/lib/rustlib/*/bin/rust-lld`
3. **Correct toolchain**: The nightly toolchain must have `wasm32-unknown-unknown` target installed

Note: The host project builds fine with `clang`+`mold` (native target), but cell WASM compilation uses a completely different linker path.

---

## TODO / Future Ideas

- UI cleanup.
- CI cleanup.

- **Notebook tagging/filtering**: Tags on notebooks for organization, search/filter on home page
- **LSP integration**: Full rust-analyzer completions in Monaco (per-cell analysis)
- **Collaboration**: Real-time multi-user editing via WebSocket
