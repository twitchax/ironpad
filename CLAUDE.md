# ironpad Agents Guide

This document provides guidance for AI coding agents working in the ironpad repository.

## Quick Overview

**ironpad** is an interactive Rust notebook environment that compiles cells to WebAssembly and executes them in the browser. It supports real-time collaboration between humans and AI agents via WebSocket. The codebase spans 7 Rust crates, with a full-stack Leptos (SSR/WASM) frontend, Axum server with WebSocket relay, and a CLI daemon for agent interaction.

### Key Statistics
- **Crates**: 7 (app, cli, server, frontend, common, cell, proxy)
- **Compiler modules**: 7 (scaffold, cache, build, diagnostics, optimize, toolchain, mod)
- **Framework**: Leptos 0.8 + Axum + Monaco editor
- **Build tool**: cargo-make (all dev commands)
- **Collaboration**: WebSocket relay + CLI daemon + session/token management

---

## Workspace Overview

```
crates/
  ironpad-app/          # Core: compiler, UI, storage, pages, model, session
  ironpad-cli/          # CLI daemon + agent commands
  ironpad-server/       # HTTP server + WebSocket relay + session management
  ironpad-proxy/        # Domain-filtering forward proxy (cell-compile network sandbox)
  ironpad-frontend/     # WASM hydration (minimal)
  ironpad-common/       # Shared types + collaboration protocol
  ironpad-cell/         # Cell runtime (injected into every cell)

docker/                 # Multi-stage build + docker-compose
tests/e2e/              # Playwright e2e tests (including agent session tests)
public/                 # executor-bridge.js (+ worker/core), storage.js, Monaco editor, public notebooks
style/                  # SCSS styles
data/                   # Server-side shares + private notebook storage
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

# Full CI (fmt-check + gen-completions-check + clippy + test)
cargo make ci

# UAT (the one true gate: CI + integration tests + Playwright)
cargo make uat
```

### All cargo-make Tasks

| Task                 | Purpose                                            |
| -------------------- | -------------------------------------------------- |
| `install-tools`      | Install all dev tools + wasm target                |
| `setup-monaco`       | Install monaco-editor from npm, copy dist to public/monaco/ |
| `dev`                | Start cargo-leptos watch (dev server, live reload) |
| `build`              | Release build via cargo-leptos                     |
| `build-cli`          | Build ironpad-cli binary (release)                 |
| `fmt`                | Auto-format all Rust code                          |
| `fmt-check`          | Check formatting (no changes)                      |
| `clippy`             | Run clippy lints                                   |
| `test`               | Unit/integration tests via cargo-nextest           |
| `test-integration`   | Slow tests (requires wasm32 target)                |
| `gen-completions`    | Regenerate Monaco completions index from ironpad-cell source |
| `gen-completions-check` | Fail if the committed completions index is stale |
| `warm-prod`          | Converge a deployed instance's compile/check caches (post-deploy) |
| `ci`                 | fmt-check + gen-completions-check + clippy + test  |
| `warmup-atomics`     | Pre-build std with atomics for rayon cells (one-time) |
| `playwright-install` | Install Playwright browsers                        |
| `playwright`         | Build CLI + run Playwright e2e tests               |
| `uat`                | ci + test-integration + playwright                 |
| `coverage`           | Coverage report via cargo-llvm-cov                 |
| `docker-build`       | Build Docker image                                 |
| `docker-up`          | Start container via docker-compose                 |
| `docker-down`        | Stop container                                     |
| `docker-uat`         | Build image, start container, run playwright, tear down |

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
   - blake3 hash of (source, cargo_toml, previous cell types, shared cargo_toml/source, atomics flag) plus a toolchain fingerprint, so toolchain upgrades invalidate stale blobs
   - Lookup at `{cache_dir}/blobs/{hash}.wasm`

3. **Build** (`compiler/build.rs`):
   - `cargo build --target wasm32-unknown-unknown --release --message-format=json`
   - 300-second timeout (override with `IRONPAD_BUILD_TIMEOUT_SECS`)
   - Returns WASM blob path or stdout with diagnostics

4. **Diagnostics** (`compiler/diagnostics.rs`):
   - Parses rustc JSON output
   - **Critical**: Adjusts line numbers by subtracting `WRAPPER_PREAMBLE_LINES` (4)
   - Maps back to user source for inline error display

5. **Optimize** (`compiler/optimize.rs`):
   - Best-effort `wasm-opt -O3` (binaryen; runtime speed over size)
   - Non-fatal failures

**Important**: All stages are tested with unit tests; full pipeline tested with integration tests in `compiler/mod.rs`.

### Notebook Storage & Sharing

**Private notebooks** are stored client-side in **IndexedDB** (browser-local):
- `storage/client.rs` — wasm-bindgen bindings to `window.IronpadStorage` (from `public/storage.js`)
- No server-side notebook CRUD — the server is stateless for private notebooks
- Canonical format: `IronpadNotebook` (defined in `ironpad-common/src/types.rs`)

**Public notebooks** are static `.ironpad` JSON files served from `public/notebooks/` (bundled into `{site_root}/notebooks/` at build time):
- **No index file**: `list_public_notebooks()` enumerates `{site_root}/notebooks/*.ironpad` at runtime and reads each notebook's own `title`/`description`
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
- `/embed/shared/{hash}` — EmbedSharedPage (chrome-less iframe variant; PRD-0039)
- `/embed/public/{filename}` — EmbedPublicPage (chrome-less iframe variant; PRD-0039)
- `/ws/host?notebook_id=<id>` — WebSocket: browser connects as session host
- `/ws/connect?token=<token>` — WebSocket: CLI connects as session guest

Cell I/O uses **bincode 2.0** serialization for piping output between cells.

**Shared cells (PRD-0044)**: a cell with `shared: true` renders amber inline, never executes, and its source is appended to the notebook's `shared.rs` after the notebook-level shared source. The assembly is `ironpad_common::effective_shared_source` — used by the editor, the view-only runner, and the notebook gate; never assemble it by hand. Shared cells hold empty piping slots (`cellN` indices stay positional), and editing one stales ALL code cells. Caveat: shared-cell text feeds feature detection (simd/autodiff/rayon), so a shared cell mentioning `std::simd` opts every cell in the notebook into simd128.

### Agent Collaboration Architecture

The browser is the authoritative model server. The API server is a dumb relay. The CLI daemon keeps a warm WebSocket connection for fast agent interactions.

```
Browser (model) ←→ WS ←→ API Server (relay) ←→ WS ←→ CLI Daemon ←→ Agent
```

Key modules:
- **`ironpad-common/src/protocol.rs`** — unified message protocol (mutations, queries, events, control messages)
- **`ironpad-server/src/sessions.rs`** — session store, token generation/validation, permission checking
- **`ironpad-server/src/ws.rs`** — WebSocket relay handlers
- **`ironpad-server/src/state.rs`** — shared server state (`AppState`, `WsState`)
- **`ironpad-app/src/model.rs`** — `NotebookModel` — all mutations go through here (same codepath for UI and agent)
- **`ironpad-app/src/session/`** — browser-side WebSocket session management
- **`ironpad-app/src/components/session_panel.rs`** — session UI (start/stop, token display, agent list)
- **`ironpad-cli/src/daemon.rs`** — CLI daemon (WS connection, Unix socket IPC, state cache)
- **`ironpad-cli/src/main.rs`** — CLI subcommands (cells list/get/add/update/delete/reorder, notebook, status)

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

Current server functions: `compile_cell`, `check_cell` (live check-on-type, PRD-0045), `list_public_notebooks`, `get_public_notebook`, `share_notebook`, `get_shared_notebook`.

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
- `IronpadExecutor` not found → executor-bridge.js not loaded
- WASM trap errors → Cell runtime panic

---

## File Organization

### Hot-Edit Files

Files you'll frequently modify:

- **Compiler logic**: `crates/ironpad-app/src/compiler/*.rs`
- **Notebook model**: `crates/ironpad-app/src/model.rs`
- **UI components**: `crates/ironpad-app/src/components/*.rs`
- **Session management**: `crates/ironpad-app/src/session/`
- **Client storage**: `crates/ironpad-app/src/storage/*.rs` (IndexedDB bindings)
- **Server functions**: `crates/ironpad-app/src/server_fns.rs`
- **Pages**: `crates/ironpad-app/src/pages/*.rs`
- **WebSocket relay**: `crates/ironpad-server/src/ws.rs`, `state.rs`, `sessions.rs`
- **Protocol types**: `crates/ironpad-common/src/protocol.rs`
- **CLI daemon/commands**: `crates/ironpad-cli/src/`
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

### 6. **Browser-Cache Hygiene for Static Assets**
The pkg bundle is content-hashed (cargo-leptos `hash-files`; `hash.txt` must sit next to the server binary or leptos panics — the Dockerfile keeps it in lockstep with `LEPTOS_HASH_FILES=true`). Every URL-stable `<script>`/`<link>` in the shell (`lib.rs`) must be wrapped in `versioned()` so it carries a `?v={release}` cache-buster, and a middleware in `ironpad-server/src/main.rs` serves `/pkg/` as immutable and everything else `no-cache`. Without all three, browsers heuristically cache stale JS/WASM across releases, and an old client silently drops newer notebook fields (this shipped a live bug where shared cells compiled as normal cells).

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
- Verify `public/executor-bridge.js` (plus its worker/core siblings) and `public/monaco/` exist

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

- **Dev guide**: `DEVELOPMENT.md` (setup, architecture deep-dive, contribution workflow)
- **Full PRD**: `MegaPrd.md` (historical product vision; current behavior is tracked in `.prds/`)
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

Cell compilation targets `wasm32-unknown-unknown`, which uses `rust-lld` as the default linker. On current nightly toolchains, `rust-lld` for this target no longer defaults to `--allow-undefined`, so any bare `extern "C"` block in `ironpad-cell` declaring a host-provided import (e.g. `ironpad_sim_read`, `ironpad_host_message`, the `ironpad_gpu_*` functions) fails to link with an error like `rust-lld: error: undefined symbol: ironpad_sim_read` — even though `cargo build` of the ironpad project itself succeeds (the host target uses `clang`+`mold` and isn't affected).

**Root cause**: the browser executor supplies these host functions under the WASM import module `env` (see `public/executor-core.js`), but the `extern "C"` blocks in `ironpad-cell` (`sim.rs`, `lib.rs`, `gpu.rs`) didn't tell rustc which import module to target, so the linker couldn't resolve them once `--allow-undefined` stopped being the default.

**Fix**: annotate each host-import `extern "C"` block with `#[link(wasm_import_module = "env")]` (PRD-0031 T-001). This is a compile-time hint, not a linker flag — no `.cargo/config.toml` or `RUSTFLAGS` changes are needed. Regression coverage lives in `crates/ironpad-app/src/compiler/mod.rs::e2e_tests::compile_cell_with_host_imports_links_successfully`.

### Cell Special Cases (opt-in by usage)

Cells opt into heavier build modes just by using a feature; detection is substring-based over (source, shared_source):

| Feature | Trigger substrings | Build effect | Runtime effect |
| --- | --- | --- | --- |
| rayon (atomics) | `rayon` in merged deps | `ATOMICS_TOOLCHAIN` (`nightly-2025-12-22`), `-Zbuild-std`, atomics target features + shared-memory link flags | wasm-bindgen-rayon worker pool |
| autodiff (Enzyme) | `autodiff_forward` / `autodiff_reverse` / `std::autodiff` | `AUTODIFF_TOOLCHAIN` (`nightly-2026-06-01`) + `enzyme` component, `-Zautodiff=Enable`, forced fat-LTO profile, crate-root `#![feature(autodiff)]` | none |
| SIMD (PRD-0042) | `std::simd` / `core::simd` / `std::arch::wasm32` (comments count) | `-C target-feature=+simd128`, crate-root `#![feature(portable_simd)]`; no std rebuild | none (all modern browsers) |
| blocking/JSPI (PRD-0043) | imports `ironpad_blocking_*` (via `ironpad_cell::blocking`) | none (plain imports) | executor wraps imports in `WebAssembly.Suspending`, enters raw `cell_main` via `WebAssembly.promising`; Chrome/Edge 137+ only, friendly gate elsewhere |

**Toolchains**: cell builds pin their toolchain explicitly via `cell_toolchain` (in `compiler/build.rs`), never the host default (dev boxes, CI, and the deploy image all differ, and that divergence once shipped cells that compiled locally and failed on prod). Three pins:
- **`CELL_TOOLCHAIN`** (`nightly-2026-07-14`) — normal + SIMD cells, the common case, tracked fresh. Defined ungated in `crates/ironpad-app/src/lib.rs` and displayed in the footer.
- **`AUTODIFF_TOOLCHAIN`** (`nightly-2026-06-01` + `enzyme`) — `std::autodiff` cells. Held back because July 2026 nightlies ICE on autodiff typetrees for slices; also carries `rust-src` for the autodiff+rayon `-Zbuild-std` combo.
- **`ATOMICS_TOOLCHAIN`** (`nightly-2025-12-22`) — rayon cells (wasm-bindgen-rayon's atomics guard breaks on newer nightlies).

`AUTODIFF_TOOLCHAIN` wins over `ATOMICS_TOOLCHAIN` when both apply. The cache fingerprint queries only `CELL_TOOLCHAIN`'s rustc, so bumping `CELL_TOOLCHAIN` invalidates all blobs automatically — but bumping `AUTODIFF_TOOLCHAIN`/`ATOMICS_TOOLCHAIN` needs a `CACHE_EPOCH` bump. `docker/Dockerfile` and `.github/workflows/build.yml` must install all three pins (the image drops stable entirely). Autodiff cells carry distinct RUSTFLAGS + a fat-LTO profile, so they never shared the Docker default-target warmup; they're warmed post-deploy by the runtime cache warmer.

Sharp edges: target features from independent concerns must merge into ONE `-C target-feature=` flag (rustc keeps only the last; see `compose_rustflags` in `build.rs`); each injected crate-root gate bumps `preamble_lines` by one for diagnostics mapping; all detection booleans are part of the blake3 cache key; a cell whose text contains the literal `.await` substring anywhere (even in a string) compiles to an async wrapper, which the JSPI promising path cannot enter — keep `blocking::*` cells free of it.

### PRDs

- **PRD files live in `.prds/`** with YAML frontmatter (currently `PRD-0001` through `PRD-0038`)
- **`MegaPrd.md` (repo root)** holds the original product vision; treat it as historical
- **Review notes live in `.prds/reviews/`**; agentic-collaboration specs live in `.prds/agentic-collaboration/`

---

**Last Updated**: 2026-07-21 — view mode renders the public notebook renderer + code-wide papercut sweep (v0.12.11); per-cell collapse defaults (v0.12.10); live check-on-type + completions (PRD-0045); shared cells; unified toolchains
**Target Audience**: AI agents, developers contributing to ironpad
