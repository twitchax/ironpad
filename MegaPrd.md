# PRD: ironpad — Interactive Rust Notebooks

**Project codename:** `ironpad`
**Repo:** `github.com/twitchax/ironpad`
**Author:** Aaron Roney
**Date:** 2026-03-06
**Status:** Draft

---

## 1. Problem Statement

There is no compelling, Rust-native interactive notebook experience. Existing options:

- **Rust Playground** — single-file, no cell model, no persistence, no visualization, no per-cell dependency management.
- **Jupyter + evcxr** — shoehorns Rust into a Python-centric ecosystem. Slow, fragile, poor error messages, no per-cell dependency isolation, awkward visualization story.
- **Various WASM playgrounds** — toy-scoped, no dependency management, no cell-to-cell data flow.

Rust learners and educators need a tool that feels native to the language: real `Cargo.toml` per cell, fast compilation feedback, good error rendering, and a clean path to visualization — all in a polished, approachable UI.

---

## 2. Vision

A Leptos-based interactive notebook where:

- Each cell is an isolated micro-crate with its own editable `Cargo.toml` and source.
- The server compiles cells to WASM blobs; the client caches and executes them.
- Cells pass data forward via a typed, serialized boundary (bincode).
- A curated standard library (`ironpad-std`) provides ergonomic input/output types: charts, tables, formatted text, images.
- The UI is polished and approachable for Rust learners on day one.
- Notebooks persist to the filesystem and are portable (plain files).

Long-term, ironpad becomes the default "try Rust interactively" tool for the Rust ecosystem.

---

## 3. Target Users

### 3.1 Primary: Rust learners and educators

- Students working through Rust concepts interactively.
- Educators building lesson plans with executable, shareable notebooks.
- Self-taught developers who want a faster feedback loop than `cargo new` → edit → `cargo run`.

### 3.2 Secondary: Rust power users

- Data exploration and prototyping in Rust.
- Library authors demoing crate functionality in-browser.
- Conference speakers building live-code presentations.

---

## 4. Goals & Non-Goals

### 4.1 MVP Goals

| # | Goal | Success Metric |
|---|------|---------------|
| G1 | Cold compile turnaround < 5s for a trivial cell | p95 < 5s, measured client-side end-to-end |
| G2 | Polished, approachable UI | Qualitative: "looks like a real product" from 3+ external testers |
| G3 | Per-cell dependency isolation via editable Cargo.toml | Each cell compiles independently with its own deps |
| G4 | Client-side WASM execution with bincode cell boundary | Cells execute in-browser; output passed to next cell as bytes |
| G5 | Notebook persistence to filesystem | Save/load notebooks as a directory or single file |
| G6 | Compiler error rendering that helps learners | Errors rendered inline with span highlighting, not raw `rustc` stderr dumps |
| G7 | Single-user self-hosted container deployment | `docker run` and go |

### 4.2 Non-Goals (MVP)

| # | Non-Goal | Rationale |
|---|----------|-----------|
| NG1 | Server-side execution toggle | Defer until post-MVP; client-side WASM covers most learning use cases |
| NG2 | Multi-user / collaboration | Single-user container for MVP; no auth, no sharing infra |
| NG3 | DAG-based cell ordering | Linear cell order only; DAG adds complexity with minimal learner benefit |
| NG4 | Multiple compiler versions | Ship with stable; version selector is a post-MVP feature |
| NG5 | `ironpad-std` charting/visualization library | MVP uses raw bincode `Vec<u8>` boundary; standard lib is a fast-follow |
| NG6 | Custom system dependencies (`apt-get`) | Pure-Rust / `wasm32-unknown-unknown`-compatible crates only for MVP |
| NG7 | Multi-tenant SaaS deployment | Out of scope; single-user container only |

---

## 5. Architecture

### 5.1 High-Level Overview

```
┌──────────────────────────────────────────────────┐
│                   Browser (Client)                │
│                                                   │
│  ┌─────────────┐  ┌─────────────┐  ┌──────────┐  │
│  │ Monaco       │  │ Monaco       │  │ Output   │  │
│  │ (cell code)  │  │ (Cargo.toml) │  │ Panel    │  │
│  └──────┬──────┘  └──────┬──────┘  └────▲─────┘  │
│         │                │               │        │
│         ▼                ▼               │        │
│  ┌─────────────────────────────┐         │        │
│  │  Leptos Client (Hydrated)   │         │        │
│  │  - Cell state management    │         │        │
│  │  - WASM blob cache (Map)    │         │        │
│  │  - WASM executor            │─────────┘        │
│  │  - Bincode cell I/O wiring  │                  │
│  └──────────┬─────────────────┘                   │
│             │ compile request                     │
└─────────────┼─────────────────────────────────────┘
              │ Leptos #[server] fn
              ▼
┌──────────────────────────────────────────────────┐
│              Server (Container)                   │
│                                                   │
│  ┌─────────────────────────────────────────────┐  │
│  │  Compilation Service                        │  │
│  │  - Receives (source, Cargo.toml, target)    │  │
│  │  - Content-hash check → cache hit/miss      │  │
│  │  - On miss: write micro-crate, cargo build  │  │
│  │  - Returns .wasm blob                       │  │
│  └─────────────────────────────────────────────┘  │
│                                                   │
│  ┌──────────────┐  ┌───────────────────────────┐  │
│  │ Blob Cache   │  │ Cargo Registry Cache      │  │
│  │ (content-    │  │ (shared across sessions)  │  │
│  │  hash → wasm)│  │                           │  │
│  └──────────────┘  └───────────────────────────┘  │
│                                                   │
│  ┌──────────────────────────────────────────────┐ │
│  │ Notebook Storage (filesystem)                │ │
│  │ /data/notebooks/{id}/notebook.json           │ │
│  └──────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────┘
```

### 5.2 Cell Model

Each cell is an isolated micro-crate. On the server, a cell compile looks like:

```
/tmp/ironpad-workspaces/{session}/{cell_id}/
  Cargo.toml       # user-editable
  src/
    lib.rs         # auto-wrapped cell code
```

The user writes the cell body. The server wraps it in a function with the standard signature:

```rust
// Auto-generated wrapper (not visible to user in simple mode)
use ironpad_cell::prelude::*;

#[no_mangle]
pub extern "C" fn cell_main(input_ptr: *const u8, input_len: usize) -> CellResult {
    // --- user code begins ---
    {user_code}
    // --- user code ends ---
}
```

For the MVP (before `ironpad-std` exists), the raw interface is:

```rust
#[no_mangle]
pub extern "C" fn cell_main(input_ptr: *const u8, input_len: usize) -> *mut u8 {
    // User code that reads input bytes and returns output bytes
}
```

A thin `ironpad-cell` crate (always injected as a dependency) provides the memory allocation FFI, bincode helpers, and the `CellResult` type.

### 5.3 Cell Data Flow

```
Cell 0           Cell 1           Cell 2
  │                 │                │
  │ output: Vec<u8> │                │
  │ (bincode)       │                │
  ├────────────────►│                │
  │                 │ output: Vec<u8>│
  │                 │ (bincode)      │
  │                 ├───────────────►│
  │                 │                │
```

- Each cell receives the previous cell's output as `&[u8]` (bincode-serialized).
- Cell 0 receives empty input (`&[]`).
- The client-side WASM executor manages this pipeline: instantiate blob → call `cell_main` with previous output → capture output → pass to next cell.
- Output is also rendered in the cell's output panel (raw bytes shown as hex/text for MVP; rich rendering post-MVP with `ironpad-std`).

### 5.4 Compilation Pipeline

1. Client sends `(cell_source, cargo_toml_contents, compiler_version)` to server via `#[server]` fn.
2. Server computes content hash: `blake3(cell_source || cargo_toml || compiler_version || target)`.
3. Cache lookup (filesystem: `/cache/blobs/{hash}.wasm`).
4. **Cache hit:** return blob immediately.
5. **Cache miss:**
   a. Write micro-crate to `/tmp/ironpad-workspaces/{session}/{cell_id}/`.
   b. Inject `ironpad-cell` as a dependency (path or registry dep).
   c. Run `cargo build --target wasm32-unknown-unknown --release` with:
      - `CARGO_HOME` pointing to shared registry cache.
      - `CARGO_TARGET_DIR` per-session (allows incremental reuse within session).
      - Timeout: 30s hard kill.
   d. Strip the `.wasm` with `wasm-opt` or `wasm-strip` (if available) for size.
   e. Cache the blob at `/cache/blobs/{hash}.wasm`.
   f. Return blob + compile diagnostics.
6. Client receives blob, caches in a `Map<cell_id, {hash, Module}>`.
7. On re-run without source changes, client skips server round-trip entirely.

### 5.5 Client-Side WASM Execution

The client executor:

1. Receives compiled `.wasm` blob from server (or client cache).
2. Instantiates via `WebAssembly.instantiate(bytes, imports)`.
3. Provides imports: memory allocator, `ironpad-cell` FFI functions.
4. Calls `cell_main(input_ptr, input_len)`.
5. Reads output from WASM linear memory.
6. Renders output in the cell's output panel.
7. Stores output bytes for the next cell's input.

### 5.6 Notebook Persistence

Notebooks are stored as a directory on the server filesystem:

```
/data/notebooks/{notebook_id}/
  notebook.json       # metadata, cell ordering, cell configs
  cells/
    cell_0/
      source.rs
      Cargo.toml
    cell_1/
      source.rs
      Cargo.toml
    ...
```

`notebook.json` schema (MVP):

```json
{
  "id": "uuid",
  "title": "My Notebook",
  "created_at": "2026-03-06T00:00:00Z",
  "updated_at": "2026-03-06T00:00:00Z",
  "compiler_version": "stable",
  "cells": [
    {
      "id": "cell_0",
      "order": 0,
      "label": "Setup"
    },
    {
      "id": "cell_1",
      "order": 1,
      "label": "Process data"
    }
  ]
}
```

Save is triggered explicitly (Ctrl+S / save button). Auto-save is a post-MVP feature.

---

## 6. UI / UX Design

### 6.1 Layout

```
┌─────────────────────────────────────────────────────────────┐
│  ironpad                          [Notebook Title]  [Save]  │
├─────────────────────────────────────────────────────────────┤
│                                                             │
│  ┌─ Cell 0 ──────────────────────────────────────────────┐  │
│  │ [Code ▾] [Cargo.toml ▾]              [▶ Run] [⋯]     │  │
│  │ ┌──────────────────────────────────────────────────┐  │  │
│  │ │  // Monaco editor: cell source                   │  │  │
│  │ │  let data = vec![1.0, 2.0, 3.0];                │  │  │
│  │ │  bincode::serialize(&data).unwrap()              │  │  │
│  │ └──────────────────────────────────────────────────┘  │  │
│  │ ┌──────────────────────────────────────────────────┐  │  │
│  │ │  Output:                                         │  │  │
│  │ │  [1.0, 2.0, 3.0]  (12 bytes)       ✓ 0.3ms     │  │  │
│  │ └──────────────────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────────────────┘  │
│                           ┊                                 │
│                     [+ Add Cell]                            │
│                           ┊                                 │
│  ┌─ Cell 1 ──────────────────────────────────────────────┐  │
│  │ [Code ▾] [Cargo.toml ▾]              [▶ Run] [⋯]     │  │
│  │ ┌──────────────────────────────────────────────────┐  │  │
│  │ │  let data: Vec<f64> = bincode::deserialize(input)│  │  │
│  │ │      .unwrap();                                  │  │  │
│  │ │  let sum: f64 = data.iter().sum();               │  │  │
│  │ │  println!("Sum: {sum}");                         │  │  │
│  │ └──────────────────────────────────────────────────┘  │  │
│  │ ┌──────────────────────────────────────────────────┐  │  │
│  │ │  Output:                                         │  │  │
│  │ │  Sum: 6.0                           ✓ 0.1ms     │  │  │
│  │ └──────────────────────────────────────────────────┘  │  │
│  └───────────────────────────────────────────────────────┘  │
│                                                             │
│                     [+ Add Cell]                            │
│                                                             │
├─────────────────────────────────────────────────────────────┤
│  Status: Ready  │  Compiler: stable  │  Cells: 2           │
└─────────────────────────────────────────────────────────────┘
```

### 6.2 UI Components (Thaw UI)

- **Notebook header:** title (editable), save button, notebook-level settings.
- **Cell card:** collapsible card component containing:
  - **Tab bar:** "Code" and "Cargo.toml" tabs (Monaco instances).
  - **Run button:** compile + execute this cell. Shift+Enter keyboard shortcut.
  - **Run All Below:** execute this cell and all cells below it sequentially.
  - **Cell menu (⋯):** delete, move up/down, rename label, duplicate.
  - **Output panel:** rendered output below the editor. Collapsible.
  - **Error panel:** rendered compiler errors with inline span highlights.
  - **Status indicator:** idle / compiling / running / error / success + timing.
- **Add Cell button:** between cells and at the bottom.
- **Status bar:** compiler version, cell count, last save time.

### 6.3 Monaco Configuration

- **Language:** Rust (syntax highlighting, basic completions via `monarch` grammar).
- **Theme:** match ironpad's overall theme (dark default, light option).
- **Cargo.toml tab:** TOML language mode.
- **Keybindings:**
  - `Shift+Enter` — run current cell.
  - `Ctrl+Shift+Enter` — run all cells from top.
  - `Ctrl+S` — save notebook.
  - `Ctrl+Shift+N` — new cell below.

### 6.4 Error Rendering

Compiler errors are the #1 pain point for learners. ironpad must render them well:

- Parse `rustc` JSON diagnostic output (`--error-format=json`).
- Map error spans back to the user's code (offset by the auto-generated wrapper lines).
- Render inline in Monaco via `monaco.editor.setModelMarkers`.
- Show the full error message in the output panel with the same formatting as `cargo` terminal output (colors via ANSI-to-HTML or similar).
- Link to Rust error index pages (e.g., `https://doc.rust-lang.org/error_codes/E0308.html`) where applicable.

---

## 7. Technical Design Details

### 7.1 Tech Stack

| Layer | Technology | Rationale |
|-------|-----------|-----------|
| Framework | Leptos | Rust-native, SSR + hydration, `#[server]` fns for compile RPC |
| UI Components | Thaw UI | Leptos-native component library; tabs, cards, buttons, modals |
| Code Editor | Monaco | Industry standard; JS-based, integrated via `wasm-bindgen` / JS interop |
| Serialization (cell boundary) | bincode | Fast, compact, Rust-native; zero-copy deserialization possible |
| Compilation | `cargo` + `rustc` (wasm32-unknown-unknown) | Standard Rust toolchain |
| WASM execution | `WebAssembly.instantiate` (browser native) | No additional runtime needed |
| Content hashing | blake3 | Fast, Rust-native |
| Notebook storage | Filesystem (JSON + source files) | Simple, portable, git-friendly |
| Container runtime | Docker | Standard; single `Dockerfile` |
| CSS | Tailwind (via Thaw defaults or custom) | Utility-first, works well with Leptos |

### 7.2 Crate Structure

```
ironpad/
  Cargo.toml                  # workspace root
  crates/
    ironpad-server/           # Leptos server binary
      src/
        main.rs
        compile.rs            # compilation pipeline
        cache.rs              # blob cache
        notebook.rs           # persistence
    ironpad-client/           # Leptos client (hydrated)
      src/
        app.rs                # root component
        cell.rs               # cell component
        executor.rs           # WASM executor (JS interop)
        editor.rs             # Monaco wrapper
    ironpad-cell/             # injected into every cell as a dependency
      src/
        lib.rs                # CellResult, memory FFI, bincode helpers
    ironpad-common/           # shared types (server ↔ client)
      src/
        types.rs              # CellSource, CompileRequest, CompileResponse, NotebookManifest
  docker/
    Dockerfile
    docker-compose.yml
  notebooks/                  # default notebook storage mount point
```

### 7.3 ironpad-cell Crate (Injected Dependency)

This crate is automatically added to every cell's `Cargo.toml`. It provides:

```rust
// ironpad-cell/src/lib.rs

pub mod prelude {
    pub use bincode;
    pub use serde::{Serialize, Deserialize};
    pub use crate::{CellInput, CellOutput, CellResult};
}

/// Wrapper around input bytes from the previous cell.
pub struct CellInput<'a> {
    bytes: &'a [u8],
}

impl<'a> CellInput<'a> {
    pub fn deserialize<T: serde::de::DeserializeOwned>(&self) -> Result<T, bincode::Error> {
        bincode::deserialize(self.bytes)
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn raw(&self) -> &[u8] {
        self.bytes
    }
}

/// Wrapper around output bytes to pass to the next cell.
pub struct CellOutput {
    bytes: Vec<u8>,
    display: Option<String>,
}

impl CellOutput {
    pub fn new<T: serde::Serialize>(value: &T) -> Result<Self, bincode::Error> {
        Ok(Self {
            bytes: bincode::serialize(value)?,
            display: None,
        })
    }

    pub fn with_display(mut self, text: String) -> Self {
        self.display = Some(text);
        self
    }

    pub fn empty() -> Self {
        Self { bytes: vec![], display: None }
    }

    pub fn text(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        Self { bytes: vec![], display: Some(msg) }
    }
}

/// FFI result type returned from cell_main.
#[repr(C)]
pub struct CellResult {
    pub output_ptr: *mut u8,
    pub output_len: usize,
    pub display_ptr: *mut u8,
    pub display_len: usize,
}

// Memory allocation functions exported for the host to call.
#[no_mangle]
pub extern "C" fn ironpad_alloc(len: usize) -> *mut u8 {
    let mut buf = Vec::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

#[no_mangle]
pub extern "C" fn ironpad_dealloc(ptr: *mut u8, len: usize) {
    unsafe { drop(Vec::from_raw_parts(ptr, len, len)); }
}
```

### 7.4 Server-Side Compilation Service

```rust
// Pseudocode for the compilation pipeline

#[server]
async fn compile_cell(request: CompileRequest) -> Result<CompileResponse, ServerFnError> {
    let hash = blake3::hash(&[
        request.source.as_bytes(),
        request.cargo_toml.as_bytes(),
        request.compiler_version.as_bytes(),
        b"wasm32-unknown-unknown",
    ].concat());

    let cache_path = format!("/cache/blobs/{}.wasm", hash.to_hex());

    // Cache hit
    if Path::new(&cache_path).exists() {
        let blob = fs::read(&cache_path).await?;
        return Ok(CompileResponse { blob, diagnostics: vec![], cached: true });
    }

    // Cache miss: write micro-crate
    let workspace = format!("/tmp/ironpad/{}/{}", session_id, request.cell_id);
    fs::create_dir_all(format!("{workspace}/src")).await?;

    // Inject ironpad-cell dependency into Cargo.toml
    let cargo_toml = inject_ironpad_cell_dep(&request.cargo_toml);
    fs::write(format!("{workspace}/Cargo.toml"), cargo_toml).await?;

    // Wrap user code in cell_main function
    let wrapped_source = wrap_cell_source(&request.source);
    fs::write(format!("{workspace}/src/lib.rs"), wrapped_source).await?;

    // Compile
    let output = Command::new("cargo")
        .args(["build", "--target", "wasm32-unknown-unknown", "--release",
               "--message-format=json"])
        .current_dir(&workspace)
        .env("CARGO_HOME", "/cargo-cache")
        .timeout(Duration::from_secs(30))
        .output()
        .await?;

    // Parse diagnostics
    let diagnostics = parse_rustc_diagnostics(&output.stderr, &request.source)?;

    if !output.status.success() {
        return Ok(CompileResponse { blob: vec![], diagnostics, cached: false });
    }

    // Read and optimize the wasm blob
    let wasm_path = format!(
        "{workspace}/target/wasm32-unknown-unknown/release/{}.wasm",
        crate_name_from_toml(&request.cargo_toml)
    );
    let blob = fs::read(&wasm_path).await?;
    let blob = optimize_wasm(blob)?; // wasm-opt -Oz if available

    // Cache it
    fs::write(&cache_path, &blob).await?;

    Ok(CompileResponse { blob, diagnostics, cached: false })
}
```

### 7.5 Client-Side WASM Executor (JS Interop)

The executor runs in the browser via `wasm-bindgen` JS interop:

```javascript
// Simplified executor logic (called from Leptos via wasm-bindgen)

class CellExecutor {
    constructor() {
        this.modules = new Map();  // cell_id -> { hash, instance }
    }

    async loadBlob(cellId, hash, wasmBytes) {
        if (this.modules.has(cellId) && this.modules.get(cellId).hash === hash) {
            return; // Already loaded, same version
        }

        const imports = {
            env: {
                ironpad_alloc: (len) => { /* allocate in wasm memory */ },
                ironpad_dealloc: (ptr, len) => { /* free in wasm memory */ },
            }
        };

        const { instance } = await WebAssembly.instantiate(wasmBytes, imports);
        this.modules.set(cellId, { hash, instance });
    }

    execute(cellId, inputBytes) {
        const mod = this.modules.get(cellId);
        if (!mod) throw new Error(`Cell ${cellId} not loaded`);

        const { instance } = mod;
        const memory = instance.exports.memory;

        // Write input bytes to WASM memory
        const inputPtr = instance.exports.ironpad_alloc(inputBytes.length);
        new Uint8Array(memory.buffer, inputPtr, inputBytes.length).set(inputBytes);

        // Call cell_main
        const resultPtr = instance.exports.cell_main(inputPtr, inputBytes.length);

        // Read CellResult struct from memory
        const view = new DataView(memory.buffer);
        const outputPtr = view.getUint32(resultPtr, true);
        const outputLen = view.getUint32(resultPtr + 4, true);
        const displayPtr = view.getUint32(resultPtr + 8, true);
        const displayLen = view.getUint32(resultPtr + 12, true);

        const outputBytes = new Uint8Array(memory.buffer, outputPtr, outputLen).slice();
        const displayText = displayLen > 0
            ? new TextDecoder().decode(new Uint8Array(memory.buffer, displayPtr, displayLen))
            : null;

        // Clean up
        instance.exports.ironpad_dealloc(inputPtr, inputBytes.length);
        instance.exports.ironpad_dealloc(outputPtr, outputLen);
        if (displayLen > 0) instance.exports.ironpad_dealloc(displayPtr, displayLen);

        return { outputBytes, displayText };
    }
}
```

---

## 8. Notebook File Format

### 8.1 Directory Structure

```
my-notebook/
  ironpad.json          # manifest
  cells/
    cell_0/
      source.rs
      Cargo.toml
    cell_1/
      source.rs
      Cargo.toml
```

### 8.2 Manifest Schema (`ironpad.json`)

```json
{
  "version": 1,
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "title": "My First Notebook",
  "created_at": "2026-03-06T12:00:00Z",
  "updated_at": "2026-03-06T14:30:00Z",
  "compiler": {
    "version": "stable",
    "target": "wasm32-unknown-unknown"
  },
  "cells": [
    { "id": "cell_0", "order": 0, "label": "Generate data" },
    { "id": "cell_1", "order": 1, "label": "Analyze" }
  ]
}
```

### 8.3 Default Cell Cargo.toml

```toml
[package]
name = "cell_0"
version = "0.1.0"
edition = "2021"

[lib]
crate-type = ["cdylib"]

[dependencies]
ironpad-cell = "0.1"

# User adds their deps below this line:
```

### 8.4 Portability

- Notebooks are plain directories → git-friendly, can be zipped and shared.
- Post-MVP: support single-file `.ironpad` format (tar.gz or similar) for easy sharing.

---

## 9. Deployment

### 9.1 Container Image

```dockerfile
FROM rust:latest

# Install wasm target
RUN rustup target add wasm32-unknown-unknown

# Install wasm-opt for blob optimization
RUN apt-get update && apt-get install -y binaryen

# Install sccache for compilation caching
RUN cargo install sccache
ENV RUSTC_WRAPPER=sccache
ENV SCCACHE_DIR=/cache/sccache

# Pre-download ironpad-cell and common deps to warm the cargo cache
COPY crates/ironpad-cell /tmp/ironpad-cell-warmup/ironpad-cell
COPY docker/warmup-Cargo.toml /tmp/ironpad-cell-warmup/Cargo.toml
RUN cd /tmp/ironpad-cell-warmup && cargo build --target wasm32-unknown-unknown --release
RUN rm -rf /tmp/ironpad-cell-warmup

# Build ironpad server
COPY . /app
WORKDIR /app
RUN cargo build --release --bin ironpad-server

EXPOSE 3000

VOLUME /data/notebooks
VOLUME /cache

CMD ["./target/release/ironpad-server"]
```

### 9.2 Docker Compose

```yaml
version: "3.8"
services:
  ironpad:
    build: .
    ports:
      - "3000:3000"
    volumes:
      - notebooks:/data/notebooks
      - cache:/cache
    environment:
      - IRONPAD_NOTEBOOK_DIR=/data/notebooks
      - IRONPAD_CACHE_DIR=/cache
      - RUST_LOG=info

volumes:
  notebooks:
  cache:
```

### 9.3 First-Run Experience

```bash
docker compose up -d
# Open http://localhost:3000
# Greeted with a "Welcome to ironpad" page with:
#   - "New Notebook" button
#   - List of existing notebooks (empty on first run)
#   - A sample notebook pre-loaded that demonstrates basic cell usage
```

---

## 10. Compile Performance Budget

Target: **< 5s cold compile for a trivial cell** (p95, end-to-end including network).

| Phase | Budget | Notes |
|-------|--------|-------|
| Client → Server RPC | 50ms | Leptos server fn, local container |
| Content hash + cache check | 5ms | blake3 is ~6GB/s; filesystem stat |
| Write micro-crate to disk | 10ms | Small files |
| `cargo build` (cold, trivial cell) | 3-4s | Dominated by linking; `wasm32` link is fast |
| `cargo build` (warm, same deps) | 0.5-1s | Incremental; only cell source changed |
| `wasm-opt` | 200ms | `-Oz` on small blobs |
| Cache write | 10ms | Small blob |
| Server → Client transfer | 50ms | Small blob, local |
| WASM instantiation | 10ms | Browser-native, fast for small modules |
| **Total (cold)** | **~4s** | |
| **Total (warm/cached)** | **~100ms** | Hash hit → return blob → instantiate |

### 10.1 Performance Levers

- **sccache:** caches `rustc` invocations across cells with shared deps.
- **Shared `CARGO_HOME`:** registry/index downloaded once.
- **Incremental compilation:** within a session, `cargo` can reuse the target dir.
- **Pre-warmed sysroot:** the container image pre-compiles `std` for `wasm32-unknown-unknown`.
- **Client-side blob cache:** skip server round-trip entirely if source hasn't changed.

---

## 11. Post-MVP Roadmap

### 11.1 Phase 1: Standard Library (`ironpad-std`)

- `ironpad-std` crate with ergonomic output types:
  - `Chart` (line, bar, scatter, histogram) → renders to SVG/Canvas in output panel.
  - `Table` → renders as an HTML table in output panel.
  - `Text` → formatted text (markdown rendering).
  - `Image` → PNG/SVG display.
- Pre-compiled WASM module for `ironpad-std` shipped with the container to avoid compile-time cost for standard vis.

### 11.2 Phase 2: Server-Side Execution

- Cell-level toggle: `Run: Client (WASM)` vs `Run: Server (native)`.
- Server compiles to native, executes in a sandbox, returns serialized output.
- Enables cells using crates that don't compile to `wasm32-unknown-unknown` (tokio, filesystem, network).
- Sandboxing via `nsjail`, `bubblewrap`, or WASI runtime.

### 11.3 Phase 3: Multi-Compiler & Toolchain

- Compiler version selector in notebook settings.
- Support `stable`, `beta`, `nightly`, and pinned versions.
- Managed via `rustup` in the container.

### 11.4 Phase 4: Sharing & Collaboration

- Export notebook as `.ironpad` archive (single file).
- Import/export to/from git repositories.
- Read-only sharing via URL (static HTML export of executed notebook).
- Multi-user editing (CRDT-based, e.g., `yrs` / Yjs).

### 11.5 Phase 5: Ecosystem

- Public notebook gallery.
- Custom system dependency installation (`apt-get` via notebook config).
- Plugin system for custom output renderers.
- LSP integration in Monaco (full `rust-analyzer` completions per cell).

---

## 12. Open Questions

| # | Question | Options | Decision |
|---|----------|---------|----------|
| OQ1 | Should the user see the auto-generated wrapper, or just write the cell body? | (a) Hide it, magic; (b) Show it, educational; (c) Toggle | Leaning (c): hide by default, "Show wrapper" toggle for learners |
| OQ2 | How to handle `println!` / stdout in WASM? | (a) Custom `print` macro that writes to a display buffer; (b) WASI stdout capture | Leaning (a) for MVP — simpler, no WASI dep |
| OQ3 | Should cells have explicit names/types for their output? | (a) Untyped `Vec<u8>`; (b) Declared output type in cell metadata | (a) for MVP; evolve toward (b) with `ironpad-std` |
| OQ4 | Notebook format: directory vs single file? | (a) Directory (git-friendly); (b) Single file (portable) | (a) for MVP; add (b) export in Phase 4 |
| OQ5 | Monaco loading strategy? | (a) CDN; (b) Bundled in container; (c) Lazy-load from server | Leaning (b) — no external CDN dep for self-hosted |
| OQ6 | How to manage `ironpad-cell` versioning? | (a) Path dep (from container FS); (b) Published to crates.io; (c) Git dep | Leaning (a) for MVP — simplest, no external publishing |

---

## 13. Success Criteria (MVP Launch)

| # | Criterion | How to Measure |
|---|-----------|---------------|
| S1 | A user can `docker compose up`, open the browser, create a notebook, and run a cell within 2 minutes | Manual test with a fresh user |
| S2 | Cold compile of a trivial cell completes in < 5s | Automated benchmark in CI |
| S3 | Cells can pass data via bincode and the next cell can deserialize it | Integration test |
| S4 | Compiler errors are rendered inline in Monaco with span highlighting | Manual test with intentional errors |
| S5 | Notebooks persist across container restarts (via volume mount) | Restart container, verify notebooks load |
| S6 | The UI looks polished and is usable by someone unfamiliar with the project | Qualitative feedback from 3+ testers |
| S7 | Sample notebook is pre-loaded demonstrating basic multi-cell data flow | Verify on first launch |

---

## 14. Risks

| # | Risk | Likelihood | Impact | Mitigation |
|---|------|-----------|--------|-----------|
| R1 | `cargo build` cold compile > 5s for cells with non-trivial deps | High | High | sccache, pre-warmed target dirs, dep caching |
| R2 | WASM memory management bugs (leaks, corruption) between cells | Medium | High | Thorough integration tests; consider fresh instance per execution |
| R3 | Monaco integration with Leptos is janky (hydration issues, event conflicts) | Medium | Medium | Isolate Monaco in an iframe or web component if needed |
| R4 | Thaw UI SSR hydration edge cases | Medium | Low | Pin version; fallback to custom components if needed |
| R5 | `wasm32-unknown-unknown` crate compatibility is worse than expected | Medium | Medium | Document compatible crates; Phase 2 server-side execution as escape hatch |
| R6 | Blob size bloat with per-cell allocators/panic handlers | Low | Medium | `wee_alloc`, `#[panic = "abort"]`, `wasm-opt -Oz` |

---

## Appendix A: Example User Session

1. User runs `docker compose up -d` and opens `http://localhost:3000`.
2. Clicks "New Notebook," names it "Fibonacci Explorer."
3. In Cell 0, writes:

```rust
let fibs: Vec<u64> = {
    let mut v = vec![0, 1];
    for i in 2..20 {
        let next = v[i-1] + v[i-2];
        v.push(next);
    }
    v
};
CellOutput::new(&fibs)?
```

4. Cell 0's Cargo.toml already has `ironpad-cell` as a dependency. No changes needed.
5. Presses Shift+Enter. Sees "Compiling..." for ~3s. Output panel shows: `[0, 1, 1, 2, 3, 5, 8, 13, 21, ...]  (160 bytes)  ✓ 3.2s`.
6. Adds Cell 1. Writes:

```rust
let fibs: Vec<u64> = input.deserialize()?;
let sum: u64 = fibs.iter().sum();
CellOutput::text(format!("Sum of first {} Fibonacci numbers: {}", fibs.len(), sum))
```

7. Presses Shift+Enter. Output: `Sum of first 20 Fibonacci numbers: 17710  ✓ 0.2ms`.
8. Edits Cell 0 to generate 30 numbers instead of 20. Presses Shift+Enter.
9. Cell 0 recompiles (~1s warm). Cell 1 re-runs with new input automatically.
10. Saves notebook (Ctrl+S). Closes browser. Reopens later — notebook is there.

---

## Appendix B: Reference Projects

| Project | Relevance |
|---------|-----------|
| [Rust Playground](https://play.rust-lang.org/) | Single-file Rust compilation to WASM; no cell model |
| [Jupyter + evcxr](https://github.com/evcxr/evcxr) | Rust kernel for Jupyter; inspiration for cell model, anti-pattern for perf |
| [Observable](https://observablehq.com/) | JavaScript notebooks with reactive cells; UI/UX inspiration |
| [Marimo](https://marimo.io/) | Python notebooks with reactive DAG; architecture inspiration |
| [Leptos](https://leptos.dev/) | Framework |
| [Thaw UI](https://thawui.vercel.app/) | Component library |
| [Monaco Editor](https://microsoft.github.io/monaco-editor/) | Code editor |

