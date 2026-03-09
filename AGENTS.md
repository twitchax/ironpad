# ironpad Agents Guide

This document provides guidance for AI coding agents working in the ironpad repository.

## Quick Overview

**ironpad** is an interactive Rust notebook environment that compiles cells to WebAssembly and executes them in the browser. The codebase is ~7.6k LoC across 5 Rust crates, with a full-stack Leptos (SSR/WASM) frontend, Axum server, and a sophisticated multi-stage compiler pipeline.

### Key Statistics
- **Total LoC**: ~7.6k (ironpad-app) + supporting crates
- **Crates**: 5 (app, server, frontend, common, cell)
- **Compiler modules**: 6 (scaffold, cache, build, diagnostics, optimize, mod)
- **Framework**: Leptos 0.8 + Axum + Monaco editor
- **Build tool**: cargo-make (all dev commands)

---

## Workspace Overview

```
crates/
  ironpad-app/          # Core: compiler, UI, storage, pages
  ironpad-server/       # HTTP server entry (minimal)
  ironpad-frontend/     # WASM hydration (minimal)
  ironpad-common/       # Shared types (IronpadNotebook, CompileRequest, etc.)
  ironpad-cell/         # Cell runtime (injected into every cell)

docker/                 # Multi-stage build + docker-compose
tests/e2e/              # Playwright e2e tests
public/                 # executor.js, storage.js, Monaco editor, public notebooks
style/                  # SCSS styles
data/                   # Server-side shares + public notebook index
```

---

## Quick Start

### Prerequisites
- **Rust**: 1.93+
- **Node.js**: 18+ (for npm/Playwright)
- **wasm32-unknown-unknown**: `rustup target add wasm32-unknown-unknown`

### Build & Run

```bash
# Install development tools
cargo make install-tools

# Development server (with hot reload)
cargo make dev
# Opens http://localhost:3111

# Run all tests
cargo make test

# Full CI (fmt-check + clippy + test)
cargo make ci

# UAT (the one true gate: CI + integration tests + Playwright)
cargo make uat
```

### All cargo-make Tasks

| Task               | Purpose                                            |
| ------------------ | -------------------------------------------------- |
| `dev`              | Start cargo-leptos watch (dev server, live reload) |
| `build`            | Release build via cargo-leptos                     |
| `fmt`              | Auto-format all Rust code                          |
| `fmt-check`        | Check formatting (no changes)                      |
| `clippy`           | Run clippy lints                                   |
| `test`             | Unit/integration tests via cargo-nextest           |
| `test-integration` | Slow tests (requires wasm32 target)                |
| `ci`               | fmt-check + clippy + test                          |
| `playwright`       | Run Playwright e2e tests                           |
| `uat`              | ci + test-integration + playwright                 |
| `docker-build`     | Build Docker image                                 |
| `docker-up`        | Start container via docker-compose                 |
| `docker-down`      | Stop container                                     |

---

## Constitutional Rules

These are inviolable project principles. Agents **must** follow these at all times.

1. **Clippy cleanliness**: All code must pass `cargo make clippy` (which runs with `-D warnings`). Clippy is enforced in both `ci` and `uat`. When making changes, fix any clippy warnings you introduce — and fix pre-existing warnings in files you touch when reasonable.

---

## Code Conventions

### Style & Patterns

- **Formatting**: Run `cargo make fmt` before committing
- **Imports**: Group by workspace, std, then external (alphanumeric)
- **Error handling**: `anyhow::Result<T>` for fallible functions; use `.context()` for error messages
- **Logging**: `tracing` (info, warn, debug) — no `println!`
- **Comments**: Use `//` and separate sections with `// ── Section ──────`
- **Modules**: Keep modules focused; use descriptive names

### Rust Idioms

- Prefer guard clauses over nested conditionals
- Use `?` operator for error propagation
- Leverage functional programming (map, filter, fold)
- Avoid `unwrap()` in library code; use `?` or `.context()`

### Testing

All compiler logic should be well-tested:
- **Unit tests**: In-crate `#[test]` functions
- **Integration tests**: Marked `#[ignore]` if slow, for `cargo make test-integration`
- **E2E tests**: Playwright in `tests/e2e/` (browser automation)

---

## Key Architecture

### Compilation Pipeline

The core of ironpad is a 5-stage WASM compiler:

1. **Scaffold** (`compiler/scaffold.rs`):
   - Generates a micro-crate from user source + Cargo.toml
   - Wraps user code in `cell_main` FFI function
   - Injects ironpad-cell dependency

2. **Cache Check** (`compiler/cache.rs`):
   - blake3 hash of (source || cargo_toml || "wasm32-unknown-unknown")
   - Lookup at `{cache_dir}/blobs/{hash}.wasm`

3. **Build** (`compiler/build.rs`):
   - `cargo build --target wasm32-unknown-unknown --release --message-format=json`
   - 30-second timeout
   - Returns WASM blob path or stdout with diagnostics

4. **Diagnostics** (`compiler/diagnostics.rs`):
   - Parses rustc JSON output
   - **Critical**: Adjusts line numbers by subtracting `WRAPPER_PREAMBLE_LINES` (4)
   - Maps back to user source for inline error display

5. **Optimize** (`compiler/optimize.rs`):
   - Best-effort `wasm-opt -Oz` (binaryen)
   - Non-fatal failures

**Important**: All stages are tested with unit tests; full pipeline tested with integration tests in `compiler/mod.rs`.

### Notebook Storage & Sharing

**Private notebooks** are stored client-side in **IndexedDB** (browser-local):
- `storage/client.rs` — wasm-bindgen bindings to `window.IronpadStorage` (from `public/storage.js`)
- No server-side notebook CRUD — the server is stateless for private notebooks
- Canonical format: `IronpadNotebook` (defined in `ironpad-common/src/types.rs`)

**Public notebooks** are static `.ironpad` JSON files served from `public/notebooks/`:
- Index at `{data_dir}/public_notebooks/index.json`
- Server functions: `list_public_notebooks()`, `get_public_notebook(filename)`

**Shared notebooks** use content-addressed storage:
- Upload notebook JSON → blake3 hash (16 hex chars) → stored at `{data_dir}/shares/{hash}.json`
- Server functions: `share_notebook(notebook_json)`, `get_shared_notebook(hash)`
- Share URL: `/shared/{hash}`

**Routes**:
- `/` — HomePage (lists private IndexedDB notebooks + public notebooks)
- `/notebook/{id}` — NotebookEditorPage (private, IndexedDB-backed)
- `/notebook/public/{filename}` — PublicNotebookPage (read-only, static `.ironpad` file)
- `/shared/{hash}` — SharedNotebookPage (read-only, shared via hash)

Cell I/O uses **bincode 2.0** serialization for piping output between cells.

### Frontend Architecture

**Leptos** with SSR + hydration:
- Server renders HTML; client hydrates into WASM SPA
- `ironpad-app` split by feature flags: `ssr` (server), `hydrate` (client)
- Components: Monaco editor, executor bindings, error panel, layout, view-only notebook
- Pages: home, notebook editor, public notebook viewer, shared notebook viewer

**Key client-side APIs**:
- `window.IronpadMonaco.*` — Monaco editor JS bridge
- `window.IronpadExecutor.*` — WASM executor (cell loading/execution)
- `window.IronpadStorage.*` — IndexedDB storage (notebook CRUD, from `public/storage.js`)

---

## Common Tasks

### Adding a New Server Function

Current server functions: `compile_cell`, `list_public_notebooks`, `get_public_notebook`, `share_notebook`, `get_shared_notebook`.

1. Add to `server_fns.rs` with `#[server]` attribute:
   ```rust
   #[server]
   pub async fn my_operation(param: Type) -> Result<Response, ServerFnError> {
       let config = expect_context::<AppConfig>();
       // ... implementation
   }
   ```

2. Call from client component:
   ```rust
   let result = my_operation(param).await?;
   ```

3. Test with Playwright if it needs e2e validation.

### Modifying Compiler Logic

1. Identify which stage: scaffold, cache, build, diagnostics, or optimize
2. Update the module and add/update unit tests
3. Run `cargo make test` to verify
4. If modifying diagnostics line mapping, test in `compiler/mod.rs::pipeline_tests`

### Adding UI Components

1. Create new file in `components/`
2. Define Leptos component with `#[component]` macro
3. Export from `components/mod.rs`
4. Use in pages (home_page or notebook_editor)
5. Add styling to `style/main.scss` (CSS custom properties + dark theme)

### Updating CLI Configuration

1. Modify `server/src/config.rs` (clap parser)
2. Update default values, env var names, help text
3. Add tests in config.rs for arg parsing
4. Update Docker environment in `docker/Dockerfile` if needed

---

## Debugging Tips

### Compilation Errors

Most compilation issues are caught by `cargo make ci`. For diagnostic details:

```bash
# Check a specific cell's output
RUST_LOG=debug cargo make dev
# Look for "cache hit", "cache miss", "compilation succeeded/failed"

# See full cargo output
cd /tmp/ironpad-e2e-test-{uuid}
cargo build --target wasm32-unknown-unknown --release 2>&1 | less
```

### Test Failures

```bash
# Run a specific test with output
cargo test --lib compiler::pipeline_tests::pipeline_hash_scaffold_diagnostics_round_trip -- --nocapture

# Run Playwright tests with visible browser
HEADED=1 cargo make playwright
```

### Runtime Issues

Enable detailed logging:
```bash
RUST_LOG=ironpad=debug cargo make dev
```

Check browser console (F12) for JS errors, especially:
- `IronpadMonaco` not found → Monaco setup failed
- `IronpadExecutor` not found → executor.js not loaded
- WASM trap errors → Cell runtime panic

---

## File Organization

### Hot-Edit Files

Files you'll frequently modify:

- **Compiler logic**: `crates/ironpad-app/src/compiler/*.rs`
- **UI components**: `crates/ironpad-app/src/components/*.rs`
- **Client storage**: `crates/ironpad-app/src/storage/*.rs` (IndexedDB bindings)
- **Server functions**: `crates/ironpad-app/src/server_fns.rs`
- **Pages**: `crates/ironpad-app/src/pages/*.rs`
- **Styles**: `style/main.scss`
- **IndexedDB JS**: `public/storage.js`
- **Public notebooks**: `public/notebooks/*.ironpad`

### Configuration Files

- **Workspace**: `Cargo.toml` (dependencies, profiles, workspace metadata)
- **Build tasks**: `Makefile.toml`
- **Frontend config**: `playwright.config.ts`, `package.json`
- **Docker**: `docker/Dockerfile`, `docker/docker-compose.yml`

### Generated Files (Do Not Edit)

- `target/site/` — Built frontend bundle
- `public/monaco/vs/` — Monaco dist (copied from npm)
- `Cargo.lock` — Lock file (commit if workspace changes)

---

## Testing Strategy

### Unit Tests
Most modules have in-crate `#[test]` functions:
- Compiler scaffolding, caching, diagnostics
- FFI memory management
- Input/output serialization

Run with: `cargo make test`

### Integration Tests
Full pipeline tests in `compiler/mod.rs::e2e_tests`:
- Compile trivial cell → valid WASM
- Compilation failure → correct diagnostics
- Cache round-trip

Marked `#[ignore]` (slow). Run with: `cargo make test-integration`

### E2E Tests
Playwright tests in `tests/e2e/`:
- Page loads (sanity)
- Notebook editing
- Cell compilation + execution

Run with: `cargo make playwright`

---

## Common Pitfalls

### 1. **Forgetting WRAPPER_PREAMBLE_LINES**
When working with diagnostics, always subtract the preamble offset:
```rust
let user_line = diagnostic.spans[0].line_start - WRAPPER_PREAMBLE_LINES;
```

### 2. **Not Running cargo make before committing**
Always run `cargo make ci` (or at least `cargo make fmt && cargo make clippy`).

### 3. **Modifying ironpad-cell without version bump**
If you change the cell runtime API, the injected dependency version must match. Update in `scaffold.rs`.

### 4. **Blocking Operations on Async Runtime**
Server functions are async. Avoid sync blocking calls; use `tokio::task::spawn_blocking` if needed.

### 5. **Cache Invalidation**
If you change the compilation pipeline, the cache key (blake3 hash input) may need updating. Test with `cargo make test-integration`.

---

## Development Workflow

### Typical Feature Branch

```bash
# Create branch
git checkout -b feature/my-feature

# Make changes
# ... edit files ...

# Run CI locally
cargo make ci
cargo make test-integration  # If touching compiler
cargo make playwright         # If touching UI

# Commit
git add .
git commit -m "feat: description"

# Push and open PR
git push origin feature/my-feature
```

### Code Review Checklist

- [ ] `cargo make ci` passes
- [ ] New tests added for new logic
- [ ] No `unwrap()` or `println!()` without justification
- [ ] Error messages are clear (use `.context()`)
- [ ] Code style follows conventions (spacing, naming, comments)
- [ ] If compiler changes, `cargo make test-integration` passes
- [ ] If UI changes, Playwright tests updated

---

## Troubleshooting

### Build Issues

**"cannot find `ironpad_cell` in path dependency"**
- Check that `crates/ironpad-cell` exists
- Verify path in `scaffold.rs` matches actual location
- Run `cargo clean` and retry

**"WASM module instantiation failed"**
- Executor.js couldn't load Monaco or cell WASM
- Check browser console (F12) for 404s
- Verify `public/executor.js` and `public/monaco/` exist

### Runtime Issues

**"cache miss — compiling" takes 30+ seconds**
- First compile is slow (full cargo build + link)
- Subsequent identical cells hit cache instantly
- Check cargo registry cache setup (Docker `CARGO_HOME`)

**Diagnostics point to wrong line**
- Verify `WRAPPER_PREAMBLE_LINES` constant in `scaffold.rs` (must be 4)
- Check `diagnostics.rs::adjust_span()` is subtracting correctly

---

## Key Dependencies

### Workspace

- **Leptos 0.8**: Full-stack web framework (SSR + WASM)
- **Axum 0.8**: Web framework (routes, middleware)
- **Tokio 1**: Async runtime
- **Blake3 1**: Content hashing (cache keys)
- **Bincode 2**: Binary serialization (cell I/O)
- **Serde 1**: Serialization framework
- **Clap 4**: CLI argument parsing
- **Thaw 0.5-beta**: UI component library

### Frontend-Only

- **wasm-bindgen 0.2**: Rust ↔ JS FFI
- **web-sys 0.3**: Browser APIs
- **js-sys 0.3**: JavaScript utilities

### Build Tools

- **cargo-leptos**: Leptos SSR builder
- **cargo-nextest**: Parallel test runner
- **cargo-make**: Task runner
- **Playwright**: E2E testing

---

## Further Reading

- **Full PRD**: `MegaPrd.md` (comprehensive product requirements)
- **Per-module docs**: Each compiler module has inline `//!` documentation
- **Test examples**: Look at test cases for API usage patterns
- **Leptos docs**: https://leptos.dev
- **Thaw docs**: Component library with dark theme

---

## System & Toolchain Notes

### Active Rust Toolchain

The project uses **nightly** (`nightly-x86_64-unknown-linux-gnu`) as the default toolchain. The `wasm32-unknown-unknown` target is installed on nightly.

### Global Cargo Config (`~/.cargo/config.toml`)

The user's global cargo config sets:

```toml
[build]
jobs = 128
rustc-wrapper = "/home/twitchax/.cargo/bin/sccache"

[target.x86_64-unknown-linux-gnu]
linker = "clang"
rustflags = ["-C", "link-arg=-fuse-ld=mold"]
```

**Impact on cell compilation**: The `build_micro_crate()` function in `compiler/build.rs` sets `CARGO_HOME` and `CARGO_TARGET_DIR` but does **not** override `RUSTFLAGS` or the global cargo config. This means:
- `sccache` wraps `rustc` for cell builds (if available)
- The `mold` linker config applies only to the native `x86_64-unknown-linux-gnu` target and does **not** affect `wasm32-unknown-unknown`
- The `wasm32-unknown-unknown` target spec uses `rust-lld` as its default linker (`"linker": "rust-lld"`, `"linker-flavor": "wasm-lld"`)

### Known Issue: `rust-lld` Linking Failures During Cell Evaluation

Cell compilation targets `wasm32-unknown-unknown`, which uses `rust-lld` as the default linker. If `rust-lld` is not properly installed or accessible in the Rust toolchain, cells will fail to link even though `cargo build` of the ironpad project itself succeeds (since the host target uses `clang`+`mold`).

**Diagnosis**: Run `rustup component list --installed | grep llvm` to check LLVM tools availability. The `rust-lld` binary lives inside the toolchain sysroot at `$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld`.

**Potential fixes**:
- Ensure `llvm-tools` or `llvm-tools-preview` component is installed: `rustup component add llvm-tools-preview`
- Or set a `.cargo/config.toml` in the project root to explicitly configure the WASM linker
- Or pass explicit `RUSTFLAGS` in the build command to specify the linker path

### PRD / microralph

- PRD files live in `.mr/prds/` with YAML frontmatter
- `MegaPrd.md` contains the comprehensive product requirements
- `.mr/PRDS.md` is an auto-generated index

---

**Last Updated**: 2026-03-07 — enriched with system/toolchain details and known issues
**Target Audience**: AI agents, developers contributing to ironpad
