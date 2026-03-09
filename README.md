# ironpad — Interactive Rust Notebooks

**ironpad** is a full-stack web application for writing, compiling, and executing Rust code in an interactive notebook environment. Users write Rust cells that compile to WebAssembly, execute in the browser, and communicate through a bincode-serialized data pipeline.

## Table of Contents

1. [Architecture Overview](#architecture-overview)
2. [Workspace Structure](#workspace-structure)
3. [Core Components](#core-components)
4. [Compilation Pipeline](#compilation-pipeline)
5. [Frontend UI](#frontend-ui)
6. [Server & Deployment](#server--deployment)
7. [Development Guide](#development-guide)
8. [Project Layout](#project-layout)

---

## Architecture Overview

ironpad follows a **full-stack Rust + WASM** architecture:

- **Frontend**: Leptos (Rust-to-WASM framework) with Monaco editor and UI components
- **Server**: Axum web framework with Leptos SSR integration
- **Compilation**: Multi-stage pipeline: scaffolding → `cargo build` (to WASM) → diagnostics → optimization
- **Execution**: Client-side WASM execution with FFI-based I/O piping between cells
- **Storage**: Private notebooks in IndexedDB (browser); public notebooks as static `.ironpad` files; shared notebooks via content-addressed hash

## Workspace Structure

The project is a Cargo workspace with 5 member crates under `crates/`:

### 1. **ironpad-app** (Core business logic - 7,643 LoC)
The heart of the application, split between SSR server code and hydrate (client) code.

**Key modules**:
- `compiler/` — Full WASM compilation pipeline (scaffold → build → optimize → cache)
- `components/` — Leptos UI components (Monaco editor, executor, error panel, layout, view-only notebook)
- `storage/` — Client-side IndexedDB bindings (wasm-bindgen to `window.IronpadStorage`)
- `pages/` — Route pages: home, notebook editor, public notebook viewer, shared notebook viewer
- `server_fns.rs` — Leptos server functions for compilation, public notebooks, and sharing

### 2. **ironpad-server** (HTTP server entry point)
Minimal binary that starts the Axum + Leptos SSR server.

**Files**:
- `main.rs` — Tokio runtime, route generation, public notebook index setup
- `config.rs` — CLI argument parsing (data_dir, cache_dir, port, ironpad_cell_path)

**CLI Flags**:
```
--data-dir <PATH>           Data directory for notebooks (default: ./data)
--cache-dir <PATH>          Cache directory for compiled blobs (default: ./cache)
--port <PORT>               HTTP port (default: 3111)
--ironpad-cell-path <PATH>  Path to ironpad-cell crate (default: ./crates/ironpad-cell)
```

Environment variable overrides: `IRONPAD_DATA_DIR`, `IRONPAD_CACHE_DIR`, `IRONPAD_PORT`, `IRONPAD_CELL_PATH`

### 3. **ironpad-frontend** (WASM hydration layer)
Minimal crate that hydrates the Leptos app into the browser.

**Files**:
- `lib.rs` — Hydration entry point with panic hook setup

### 4. **ironpad-common** (Shared types)
Types used by both server and client (compile requests/responses, notebook format, diagnostics, etc.).

**Key types**:
- `CompileRequest` / `CompileResponse` — RPC contract
- `Diagnostic` / `Severity` / `Span` — Compiler diagnostics with source mapping
- `IronpadNotebook` / `IronpadCell` / `IronpadMarkdownCell` — Canonical notebook JSON format
- `PublicNotebookSummary` — Public notebook index entry
- `ExecutionResult` — Execution output with timing
- `AppConfig` — Server configuration

### 5. **ironpad-cell** (User cell runtime)
Injected into every compiled cell as a path dependency. Provides the FFI layer for I/O.

**Key types**:
- `CellInput` — Read-only view over previous cell's output (deserialize via bincode)
- `CellOutput` — Output builder with optional display text and binary payload
- `CellResult` — FFI-compatible struct returned from `cell_main` (`#[repr(C)]`)
- `ironpad_alloc` / `ironpad_dealloc` — WASM memory management exports

---

## Core Components

### Compiler Module (`crates/ironpad-app/src/compiler/`)

The compilation pipeline is event-driven and fully cached:

#### **1. Scaffolding** (`scaffold.rs` - 353 LoC)
Takes user source + Cargo.toml and generates a complete micro-crate:
- Directory: `{cache_dir}/workspaces/{session_id}/{cell_id}/`
- **Cargo.toml**: Package metadata + `cdylib` crate type + ironpad-cell path dependency + user deps
- **src/lib.rs**: Wraps user code in `cell_main` FFI function with ironpad_cell prelude

**Key constant**: `WRAPPER_PREAMBLE_LINES = 4` — used to map compiler diagnostics back to user code

#### **2. Caching** (`cache.rs` - 167 LoC)
Uses **blake3 hashing** for deterministic caching:
```
hash = blake3(source || cargo_toml || "wasm32-unknown-unknown")
cache_path = {cache_dir}/blobs/{hash}.wasm
```
Cache misses are transparent; hits skip compilation.

#### **3. Build** (`build.rs` - 209 LoC)
Invokes `cargo build --target wasm32-unknown-unknown --release --message-format=json`:
- Timeout: 30 seconds
- Shared `CARGO_HOME` for registry cache (warmup in Docker)
- Per-session `CARGO_TARGET_DIR` for incremental reuse
- Returns either `BuildResult::Success { wasm_path }` or `BuildResult::Failure { stdout, stderr }`

#### **4. Diagnostics** (`diagnostics.rs` - 617 LoC)
Parses rustc JSON output into `Diagnostic` types:
- Extracts line/column spans from `src/lib.rs`
- **Adjusts line numbers** by subtracting `WRAPPER_PREAMBLE_LINES` so errors point to user code
- Maps error levels: error → Error, warning → Warning, note/help → Note
- Preserves error codes (e.g., "E0308") for linking to rust error index

#### **5. Optimization** (`optimize.rs` - 80 LoC)
Best-effort WASM optimization via `wasm-opt` (from binaryen). Failures are non-fatal.

#### **Full Pipeline** (`mod.rs` - 403 LoC)
Integrated in `server_fns.rs`:
```
compile_cell() {
  1. Hash input (content_hash)
  2. Cache check (try_cache_hit)
  3. Scaffold micro-crate (scaffold_micro_crate)
  4. Build to WASM (build_micro_crate)
  5. Parse diagnostics (parse_diagnostics)
  6. Optimize WASM (optimize_wasm)
  7. Cache result (store_blob)
  8. Return CompileResponse
}
```

### Notebook Storage & Sharing

#### **Client-Side Storage (IndexedDB)**

Private notebooks are stored in the browser's IndexedDB via `public/storage.js` (an IIFE that exposes `window.IronpadStorage`). The Rust `storage/client.rs` module provides wasm-bindgen bindings.

The server is **stateless** for private notebooks — no server-side CRUD.

#### **Canonical Notebook Format** (`IronpadNotebook`)

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

#### **Public Notebooks**

Static `.ironpad` JSON files in `public/notebooks/` (e.g., `welcome.ironpad`, `tutorial.ironpad`). An index at `{data_dir}/public_notebooks/index.json` is read by `list_public_notebooks()`.

#### **Shared Notebooks**

Upload notebook JSON via `share_notebook()` → blake3 content hash (first 16 hex chars) → stored at `{data_dir}/shares/{hash}.json`. Retrieve via `get_shared_notebook(hash)` at URL `/shared/{hash}`.

---

## Compilation Pipeline

### End-to-End Cell Compilation

**User Input**:
```rust
// Cell source
let fibs: Vec<u64> = vec![0, 1, 1, 2, 3, 5];
CellOutput::new(&fibs)?.with_display(format!("{:?}", fibs)).into()

// Cell Cargo.toml
[dependencies]
serde = "1"
```

**Generated Micro-Crate**:
```rust
// src/lib.rs (generated wrapper)
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

## Frontend UI

### Framework: Leptos 0.8 + Thaw Components

**Leptos** is a full-stack Rust framework for building SPAs with SSR. ironpad uses:
- **SSR mode** (server feature): HTML rendered on the server
- **Hydrate mode** (client feature): WASM takes over on the browser

**Thaw** provides pre-built UI components (dark theme by default).

### Page Routes

```
/                              → HomePage (private + public notebook list)
/notebook/{id}                 → NotebookEditorPage (private, IndexedDB-backed)
/notebook/public/{filename}    → PublicNotebookPage (read-only, static .ironpad file)
/shared/{hash}                 → SharedNotebookPage (read-only, shared via hash)
```

### Key Components

#### **Monaco Editor** (`components/monaco_editor.rs` - 254 LoC)
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

#### **Cell Executor** (`components/executor.rs` - 122 LoC)
Bridges Rust and the WASM executor:

```rust
load_blob(cell_id, hash, bytes) -> Result<(), String>
execute_cell(cell_id, input_bytes) -> Result<(Vec<u8>, Option<String>), String>
```

- Client-side only (feature-gated as `#[cfg(feature = "hydrate")]`)
- FNV-1a hashing for WASM blob caching
- Calls into `window.IronpadExecutor` JS singleton

#### **Error Panel** (`components/error_panel.rs` - 222 LoC)
Renders compiler diagnostics inline in the editor:
- Severity-based styling (red for error, yellow for warning)
- Clickable error codes linking to rust error index
- Spans displayed in tooltip/badge format

#### **App Layout** (`components/app_layout.rs` - 279 LoC)
Top-level layout with header, content area, and status bar.

#### **Pages**
- **HomePage** (`pages/home_page.rs`): List of private (IndexedDB) + public notebooks, search/filter
- **NotebookEditorPage** (`pages/notebook_editor.rs`): Full notebook editor
  - Cell list (draggable ordering via T-015)
  - Monaco editors (one per cell)
  - Compile/execute buttons
  - Output display
  - Status indicators (compiling, success, error)

- **PublicNotebookPage** (`pages/public_notebook.rs`): Read-only viewer for public `.ironpad` notebooks
- **SharedNotebookPage** (`pages/shared_notebook.rs`): Read-only viewer for shared notebooks

### Styling

CSS (SCSS) at `style/main.scss` with dark theme:
- CSS custom properties for colors, fonts, spacing
- Leptos-generated CSS module at `target/site/pkg/ironpad.css`
- Thaw components provide pre-styled UI

---

## Server & Deployment

### Axum + Leptos SSR

**main.rs** starts a Tokio async runtime with:
1. **Route generation**: `generate_route_list(App)` from Leptos
2. **Context setup**: Provides `AppConfig` to `#[server]` functions
3. **Fallback handler**: 404 page rendering
4. **TCP bind**: Listen on 0.0.0.0:{port}

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

---

## Development Guide

### Quick Start

```bash
# Install tools
cargo make install-tools

# Development server (hot reload)
cargo make dev
# Serves at http://localhost:3111
# Recompiles on code changes

# Build for release
cargo make build

# Run tests
cargo make test

# Full CI (fmt-check + clippy + test)
cargo make ci

# UAT (CI + integration tests + Playwright)
cargo make uat
```

### Makefile.toml Tasks

- `dev` — cargo-leptos watch with live reload
- `build` — Release build
- `fmt` / `fmt-check` — Rust formatting
- `clippy` — Lints
- `test` — Unit/integration tests via cargo-nextest
- `test-integration` — Slow integration tests (requires wasm target)
- `ci` — fmt-check + clippy + test
- `playwright` — Playwright e2e tests
- `uat` — ci + test-integration + playwright
- `docker-build` / `docker-up` / `docker-down` / `docker-uat` — Docker commands

### Testing

**Unit tests**: In-crate `#[test]` (compiler logic, etc.)

**Integration tests**: Slow tests in `#[test]` marked `#[ignore]` (full compilation pipeline)
```bash
cargo make test-integration
```

**E2E tests**: Playwright in `tests/e2e/*.spec.ts`
- Sanity checks (page loads)
- Notebook editing
- Cell compilation + execution
```bash
cargo make playwright
```

---

## Project Layout

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
├── .mr/                            # microralph (agent guidance)
│   ├── prds/
│   ├── templates/
│   ├── prompts/
│   └── PRDS.md
│
├── AGENTS.md                       # Agent guidance
└── MegaPrd.md                      # Full product requirements
```

---

## Key Technical Details

### WASM Compilation Target
- **Target**: `wasm32-unknown-unknown` (no WASI)
- **Optimized with**: `wasm-opt -Oz` (binaryen)
- **Exports**: `memory`, `ironpad_alloc`, `ironpad_dealloc`, `cell_main`

### Caching Strategy
- **Cache key**: blake3(source || cargo_toml || "wasm32-unknown-unknown")
- **Cache path**: `{cache_dir}/blobs/{64-char-hex}.wasm`
- **Hit rate**: High for deterministic user code; misses trigger full compilation (~30s timeout)

### Diagnostic Mapping
- Compiler reports spans in wrapper lines (src/lib.rs generated code)
- `WRAPPER_PREAMBLE_LINES = 4` hardcoded offset
- Diagnostic parser adjusts all line numbers: `user_line = rustc_line - 4`
- Error codes extracted for rust error index linking

### Cell I/O Serialization
- **Format**: bincode 2.0 with standard config
- **Flow**: Struct → bincode bytes → WASM memory → next cell's CellInput
- **Display**: Separate string field in CellResult for human-readable output

### Monaco Editor Integration
- Loaded from `public/monaco/` (AMD loader + worker setup)
- JS bridge: `window.IronpadMonaco` namespace
- Rust wrapper: `MonacoEditorHandle` for get/set operations
- Markers for inline error/warning decorations

### CLI Flags
```
--data-dir <PATH>           (env: IRONPAD_DATA_DIR, default: ./data)
--cache-dir <PATH>          (env: IRONPAD_CACHE_DIR, default: ./cache)
--port <PORT>               (env: IRONPAD_PORT, default: 3111)
--ironpad-cell-path <PATH>  (env: IRONPAD_CELL_PATH, default: ./crates/ironpad-cell)
```

### Troubleshooting: `rust-lld` Linking Failures

Cell compilation targets `wasm32-unknown-unknown`, which uses `rust-lld` as its linker. If cells fail with `linking with rust-lld failed`, check:

1. **LLVM tools installed**: `rustup component add llvm-tools-preview`
2. **rust-lld exists**: `ls $(rustc --print sysroot)/lib/rustlib/*/bin/rust-lld`
3. **Correct toolchain**: The nightly toolchain must have `wasm32-unknown-unknown` target installed

Note: The host project builds fine with `clang`+`mold` (native target), but cell WASM compilation uses a completely different linker path.

---

## Conventions for Agents

- Keep changes minimal and focused
- Follow existing style; match indentation/formatting
- Use `anyhow::Result` for fallible functions
- Prefer `tracing` over `println!` for diagnostics
- All dev commands route through `cargo make`
- Write tests for compiler logic; integration tests for full pipeline

---

**Last updated**: 2026-03-07
