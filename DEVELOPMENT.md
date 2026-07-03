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
- `compiler/` — Full WASM compilation pipeline (scaffold → build → optimize → cache)
- `components/` — Leptos UI components (Monaco editor, executor, error panel, layout, view-only notebook)
- `storage/` — Client-side IndexedDB bindings (wasm-bindgen to `window.IronpadStorage`)
- `pages/` — Route pages: home, notebook editor, public notebook viewer, shared notebook viewer
- `server_fns.rs` — Leptos server functions for compilation, public notebooks, and sharing

#### ironpad-server

Minimal binary that starts the Axum + Leptos SSR server.

**Files**:
- `main.rs` — Tokio runtime, route generation, public notebook index setup
- `config.rs` — CLI argument parsing (data_dir, cache_dir, port, ironpad_cell_path)

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
- `PublicNotebookSummary` — Public notebook index entry
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
│   │       ├── compiler/           # WASM compilation pipeline
│   │       │   ├── mod.rs          # Pipeline integration + tests
│   │       │   ├── scaffold.rs     # Micro-crate generation
│   │       │   ├── cache.rs        # blake3 caching
│   │       │   ├── build.rs        # cargo build invocation
│   │       │   ├── diagnostics.rs  # rustc JSON parsing
│   │       │   └── optimize.rs     # wasm-opt
│   │       ├── components/         # UI components
│   │       │   ├── monaco_editor.rs
│   │       │   ├── executor.rs     # WASM executor bindings
│   │       │   ├── error_panel.rs
│   │       │   ├── markdown_cell.rs
│   │       │   ├── view_only_notebook.rs  # Read-only notebook viewer
│   │       │   └── app_layout.rs
│   │       ├── storage/            # Client-side storage
│   │       │   └── client.rs       # IndexedDB bindings (wasm-bindgen)
│   │       └── pages/              # Routes
│   │           ├── home_page.rs
│   │           ├── notebook_editor.rs
│   │           ├── public_notebook.rs
│   │           └── shared_notebook.rs
│   │
│   ├── ironpad-server/             # HTTP server entry
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs             # Tokio + Axum + Leptos setup
│   │       └── config.rs           # CLI args
│   │
│   ├── ironpad-frontend/           # WASM hydration
│   │   ├── Cargo.toml
│   │   └── src/lib.rs              # Hydration entry
│   │
│   ├── ironpad-common/             # Shared types
│   │   ├── Cargo.toml
│   │   ├── src/lib.rs
│   │   ├── types.rs                # IronpadNotebook, CompileRequest/Response, etc.
│   │   └── config.rs               # AppConfig
│   │
│   └── ironpad-cell/               # Cell runtime (injected as dep)
│       ├── Cargo.toml
│       └── src/lib.rs              # CellInput, CellOutput, FFI
│
├── docker/
│   ├── Dockerfile                  # Multi-stage build
│   ├── docker-compose.yml
│   └── warmup-Cargo.toml           # Cargo cache warmup
│
├── public/
│   ├── executor.js                 # WASM executor (client-side)
│   ├── storage.js                  # IndexedDB storage API (IIFE)
│   ├── notebooks/                  # Static public .ironpad files
│   │   ├── index.json
│   │   ├── welcome.ironpad
│   │   ├── tutorial.ironpad
│   │   └── async-http.ironpad
│   └── monaco/
│       ├── vs/                     # Monaco dist (copied from npm)
│       ├── init.js                 # AMD loader config
│       ├── languages.js            # Language definitions
│       └── bridge.js               # JS ↔ Rust FFI bridge
│
├── style/
│   └── main.scss                   # Dark theme styles
│
├── data/
│   ├── public_notebooks/           # Public notebook index
│   │   └── index.json
│   └── shares/                     # Shared notebook blobs
│       └── {hash}.json
│
├── tests/
│   └── e2e/
│       ├── home.spec.ts
│       ├── notebook.spec.ts
│       └── sanity.spec.ts
│
├── AGENTS.md                       # Agent guidance
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

3. **Build** (`compiler/build.rs`) — Runs `cargo build --target wasm32-unknown-unknown --release` with JSON message output and a 30-second timeout.

4. **Diagnostics** (`compiler/diagnostics.rs`) — Parses rustc JSON output and adjusts line numbers by subtracting `WRAPPER_PREAMBLE_LINES` (4) to map errors back to user source.

5. **Optimize** (`compiler/optimize.rs`) — Best-effort `wasm-opt -Oz` (binaryen). Failures are non-fatal.

### Key Details

- **Typed injection**: Types from previous cells are injected as typed variables into the scaffold, enabling inter-cell data flow.
- **Content hash inputs**: source + dependencies + previous cell types + shared deps — any change triggers recompilation.
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
- Optimized with `wasm-opt -Oz` for minimal size

### Caching Strategy

- **Cache key**: `blake3(source || cargo_toml || "wasm32-unknown-unknown")`
- **Cache path**: `{cache_dir}/blobs/{64-char-hex}.wasm`
- **Hit rate**: High for deterministic user code; misses trigger full compilation (~30s timeout)

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
loadBlob(cellId, hash, wasmBytes)           // Load + instantiate module
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

ironpad supports multi-core parallelism in cells via [rayon](https://docs.rs/rayon) and [wasm-bindgen-rayon](https://github.com/nickhobbs94/nickhobbs94.github.io).

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
  "id": "uuid",
  "title": "My Notebook",
  "created_at": "2026-03-07T...",
  "updated_at": "2026-03-07T...",
  "cells": [
    {
      "id": "cell_0",
      "order": 0,
      "label": "Cell Label",
      "source": "let x = 42;\nCellOutput::new(&x)?.with_display(\"42\").into()",
      "cargo_toml": "[dependencies]\nserde = \"1\""
    }
  ]
}
```

### Public Notebooks

Static `.ironpad` JSON files in `public/notebooks/` (e.g., `welcome.ironpad`, `tutorial.ironpad`). An index at `{data_dir}/public_notebooks/index.json` is read by `list_public_notebooks()`.

### Shared Notebooks

Upload notebook JSON via `share_notebook()` → blake3 content hash (first 16 hex chars) → stored at `{data_dir}/shares/{hash}.json`. Retrieve via `get_shared_notebook(hash)` at URL `/shared/{hash}`.

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

### Page Routes

```
/                              → HomePage (private + public notebook list)
/notebook/{id}                 → NotebookEditorPage (private, IndexedDB-backed)
/notebook/public/{filename}    → PublicNotebookPage (read-only, static .ironpad file)
/shared/{hash}                 → SharedNotebookPage (read-only, shared via hash)
```

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
- Thaw components provide pre-styled UI

---

## Server & Deployment

### Server Functions

**`#[server]` functions** (in `server_fns.rs`) are RPC endpoints called from the browser:

```rust
#[server]
pub async fn compile_cell(request: CompileRequest) -> Result<CompileResponse, ServerFnError>

#[server]
pub async fn list_public_notebooks() -> Result<Vec<PublicNotebookSummary>, ServerFnError>

#[server]
pub async fn get_public_notebook(filename: String) -> Result<IronpadNotebook, ServerFnError>

#[server]
pub async fn share_notebook(notebook_json: String) -> Result<String, ServerFnError>

#[server]
pub async fn get_shared_notebook(hash: String) -> Result<IronpadNotebook, ServerFnError>
```

They run on the server and are automatically serialized/called from the client.

### CLI Flags

```
--data-dir <PATH>           (env: IRONPAD_DATA_DIR, default: ./data)
--cache-dir <PATH>          (env: IRONPAD_CACHE_DIR, default: ./cache)
--port <PORT>               (env: IRONPAD_PORT, default: 3111)
--ironpad-cell-path <PATH>  (env: IRONPAD_CELL_PATH, default: ./crates/ironpad-cell)
```

### Docker Deployment

**Multi-stage Dockerfile** (`docker/Dockerfile`):

1. **Builder stage** (rust:1.93.0):
   - Install `wasm32-unknown-unknown` target + binaryen
   - Install `cargo-leptos`
   - `cargo leptos build --release` → compiles server + frontend WASM

2. **Runtime stage** (rust:1.93.0):
   - Rust toolchain (needed for compiling user cells)
   - `wasm32-unknown-unknown` target
   - Binaryen (`wasm-opt`)
   - Pre-warm cargo registry with ironpad-cell dependencies
   - Copy built server binary + site assets
   - Expose port 3111

**docker-compose.yml**:
```yaml
services:
  ironpad:
    build: .
    ports: ["3111:3111"]
    volumes:
      - notebooks:/data
      - cache:/cache
    environment:
      - IRONPAD_DATA_DIR=/data
      - IRONPAD_CACHE_DIR=/cache
      - IRONPAD_PORT=3111
      - IRONPAD_CELL_PATH=/app/crates/ironpad-cell
```

### Observability (OpenTelemetry)

The server always logs to stdout via `tracing` (level controlled by `RUST_LOG`, default `info`). OTLP **trace** export is **opt-in** and off by default — it turns on only when `OTEL_EXPORTER_OTLP_ENDPOINT` is set. A `tower-http` `TraceLayer` emits one span per HTTP request, which is what gets exported.

To export to Grafana Cloud (or any OTLP backend), set the standard env vars — the exporter reads them itself, so **no credentials live in code or config**. On Fly, use secrets so the token stays in Fly's encrypted store:

```bash
# Endpoint, instance ID, and token come from your Grafana Cloud OTLP page.
fly secrets set -c .hidden/fly.toml \
  OTEL_EXPORTER_OTLP_ENDPOINT='https://otlp-gateway-<zone>.grafana.net/otlp' \
  OTEL_EXPORTER_OTLP_HEADERS='Authorization=Basic <base64(instanceID:token)>'
```

For local testing, export the same two vars in your shell before `cargo make dev` (there is no `.env` auto-loading). TLS uses rustls with bundled webpki roots, so no system OpenSSL or cert store is required in the container. Metrics/logs export are easy follow-ons once traces are confirmed flowing.

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
| `IRONPAD_PROXY_ALLOWLIST` | Comma-separated domains. Uses suffix matching: `crates.io` also allows `static.crates.io`. | `crates.io,github.com,githubusercontent.com` |

The default allowlist covers the standard Cargo ecosystem:
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
./target/release/ironpad-cli cells add --source 'let x = 42;' --label "My Cell"
./target/release/ironpad-cli cells update <CELL_ID> --source 'let x = 99;'
```

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


```
- UI cleanup.
- CI cleanup.

- **Notebook tagging/filtering**: Tags on notebooks for organization, search/filter on home page
- **LSP integration**: Full rust-analyzer completions in Monaco (per-cell analysis)
- **Collaboration**: Real-time multi-user editing via WebSocket
