# 0011: Repo Alignment

## Summary

Ensure all changes from 0001–0010 are cleanly integrated with the repo's build system, CI pipeline, Docker image, and development tooling. This is the final sweep that turns a feature branch into a shippable, maintainable whole.

## Motivation

Each preceding task focused on a single slice of functionality. None of them own the cross-cutting concerns: workspace manifest consistency, CI coverage of the new binary, Docker image updates, cargo-make task additions, and dependency hygiene. Without a dedicated pass, these gaps accumulate and the feature merges in a state where `cargo make uat` doesn't exercise the new code, CI doesn't build the CLI, and the Docker image ships without the daemon.

## Design

### 1. Workspace manifest (`Cargo.toml`)

- **Add `ironpad-cli` to workspace members.** 0008 creates the crate; this task verifies the root manifest lists it.
- **Audit workspace dependencies.** Any new deps introduced by 0004–0009 (e.g., `tokio-tungstenite`, `sha2`, `futures-util`) should be lifted to `[workspace.dependencies]` and referenced via `workspace = true` in crate manifests. No version pinning in leaf crates unless there's a concrete reason.
- **Verify feature flags.** `ironpad-common` is used by both the browser (hydrate) and server (ssr). Ensure protocol types and session types compile cleanly under both feature sets and under `--all-targets`.

### 2. CI pipeline (`.github/workflows/build.yml`)

- **`cargo make ci` already covers clippy + fmt + test**, so adding `ironpad-cli` to the workspace members is sufficient for test and lint coverage — no workflow changes needed *unless* the CLI binary needs a separate build step (it shouldn't; `cargo nextest run` covers workspace members automatically).
- **Add a `cargo build --release -p ironpad-cli` step** (or verify that `cargo leptos build --release` doesn't skip non-Leptos binary crates). The CLI is a standalone binary, not a Leptos artifact, so it may need an explicit build in CI.
- **Verify `cargo llvm-cov` includes `ironpad-cli`.** Coverage should pick it up automatically via `--workspace`, but confirm.

### 3. Docker image (`docker/Dockerfile`)

The Docker image currently copies only the `ironpad-server` binary. After this feature, it should also include the CLI daemon so that the container can serve as a self-contained deployment target.

```dockerfile
# In the builder stage — build the CLI alongside the server
RUN cargo build --release -p ironpad-cli

# In the runtime stage — copy the CLI binary
COPY --from=builder /build/target/release/ironpad-cli /app/ironpad-cli
```

Whether the CLI *should* be in the same image is a judgment call. If the CLI is only used by external agents (connecting to the container from outside), it doesn't need to be in the image. But including it costs almost nothing and enables in-container usage (e.g., health checks, scripted interactions).

### 4. cargo-make tasks (`Makefile.toml`)

Add or verify the following tasks:

| Task | Purpose |
|------|---------|
| `build-cli` | `cargo build --release -p ironpad-cli` — explicit CLI binary build |
| `test-ws` | Run WebSocket-specific integration tests (may need a running server) |
| `ci` | Verify it already covers the CLI crate (it should via `--all-targets`) |
| `uat` | Verify the full gate still passes with the new crate in the workspace |

Update `install-tools` if any new tooling is required (unlikely — the CLI uses existing deps).

### 5. Dependency hygiene

Audit every crate's `Cargo.toml` for:

- **Redundant direct dependencies** — if a crate depends on `ironpad-common` which re-exports `serde`, does the crate also need `serde` directly? (Often yes for derive macros, but verify.)
- **Feature bloat** — `tokio = { features = ["full"] }` in the workspace is fine for the server, but the CLI may only need `rt-multi-thread`, `macros`, `net`, `io-util`. If binary size matters, consider per-crate feature scoping. For MVP, `full` is acceptable.
- **Unused workspace deps** — `blake3` and `rand` were added speculatively in the workspace manifest. Verify they're actually used by at least one crate after 0004 lands. Remove if not.

### 6. Server binary alignment (`ironpad-server`)

- **`sessions.rs`** was pre-created during planning but may not be wired into `main.rs`. Verify it's imported and the `SessionStore` is injected into Axum state.
- **`ws.rs` and `state.rs`** (from 0005) should be integrated into the router. Confirm the existing Leptos routes and the new WebSocket routes coexist without conflicts (path collisions, middleware ordering).
- **`config.rs`** — verify any new CLI flags (e.g., `--session-ttl`, `--ws-heartbeat-interval`) are plumbed through and have sensible defaults.

### 7. Frontend alignment (`ironpad-app`)

- **`web-sys` features** — 0006 requires `WebSocket`, `MessageEvent`, `CloseEvent`, `ErrorEvent` features on `web-sys`. Verify these are declared in `ironpad-app/Cargo.toml` under `[target.'cfg(target_arch = "wasm32")'.dependencies]` or equivalent.
- **New components and modules** (`session.rs`, `session_panel.rs`) should be gated behind the `hydrate` feature (they're browser-only). Verify they don't break `ssr` compilation.
- **`model.rs` changes** — the event subscription mechanism added in 0006 must not regress existing notebook behavior. The existing Playwright tests (notebook, keyboard, execution) serve as the regression suite.

### 8. Integration test infrastructure

- **CLI test helpers** (`tests/e2e/helpers/cli.ts`) need the `ironpad-cli` binary to be built and accessible. Add a `build-cli` dependency to the `playwright` task or document the prerequisite.
- **Test server management** — new e2e tests from 0010 start their own server instance. Verify this doesn't conflict with the Playwright config's `webServer` directive (which also starts a server on port 3111). Either use a different port for agent tests or coordinate.
- **Timeouts** — WebSocket-based tests are inherently timing-sensitive. Verify Playwright's `expect.timeout` and test-level `timeout` values are sufficient for daemon startup + WS connection + message round-trip.

### 9. README and documentation (`README.md`)

The README is the repo's front door. It currently documents 5 crates, 4 routes, and zero mention of WebSockets, sessions, or a CLI. After this feature lands, update:

- **Architecture Overview** — add the WebSocket relay and CLI daemon to the diagram. The current description ("Frontend: Leptos … Server: Axum … Compilation … Execution … Storage") needs a new bullet for real-time collaboration.
- **Workspace Structure** — add a section for **ironpad-cli** (crate 6). Update the **ironpad-server** section to mention `sessions.rs`, `ws.rs`, and `state.rs`. Update **ironpad-common** to mention `protocol.rs`.
- **Project Layout tree** — add `crates/ironpad-cli/` and its source tree. Add new files in `ironpad-server/src/` (`sessions.rs`, `ws.rs`, `state.rs`) and `ironpad-app/src/` (`session.rs`, `components/session_panel.rs`). Add the agent e2e test files under `tests/e2e/`.
- **Development Guide** — document new cargo-make tasks (`build-cli`, etc.). Add a subsection for running the CLI daemon locally during development. Document env vars `IRONPAD_HOST` and `IRONPAD_TOKEN`.
- **CLI Flags section** — add `ironpad-cli` usage alongside the existing `ironpad-server` flags.
- **Quick Start** — add an "Agent Access" subsection showing how to start a session in the browser and connect via CLI.
- **Key Technical Details** — add a section on the WebSocket protocol, session lifecycle, and OCC versioning.
- **Last updated date** — bump to the date the feature merges.

The README should remain a single comprehensive file. Don't split it into multiple docs.

### 10. Agent guide (`AGENTS.md`)

`AGENTS.md` is a detailed guide for AI coding agents. It currently references 5 crates, the existing cargo-make task table, file organization, and architecture. It needs parallel updates to the README:

- **Workspace Overview** — add `ironpad-cli/` to the crate tree. Update the "Key Statistics" (crate count, LoC).
- **cargo-make Tasks table** — add new tasks (`build-cli`, `test-ws`, etc.).
- **Key Architecture** — add a section on the WebSocket relay, session management, and the CLI daemon architecture. The "Notebook Storage & Sharing" section should mention that the browser is now also the model server for real-time collaboration.
- **Routes** — add `/ws/host` and `/ws/connect` to the route listing.
- **File Organization / Hot-Edit Files** — add new hot-edit files: `sessions.rs`, `ws.rs`, `state.rs`, `session.rs`, `session_panel.rs`, CLI daemon modules.
- **Common Tasks** — add sections for "Modifying WebSocket Handlers", "Working with the CLI Daemon", and "Adding New Protocol Messages".
- **Key Dependencies** — add `tokio-tungstenite` (if used) or note that Axum's built-in WS support is used.
- **Common Pitfalls** — add pitfalls for WebSocket message ordering, session token handling, and OCC version conflicts.

### 11. Code quality pass

- **`cargo make fmt`** — run across the entire workspace. New files from 0004–0009 should match the project's formatting.
- **`cargo make clippy`** — fix any new warnings. Pay attention to:
  - Unused imports from speculative code
  - Missing `#[allow(dead_code)]` on types that are defined but not yet wired
  - `async` functions that could be sync
- **Dead code audit** — if any tasks landed types or functions that are ultimately unused (e.g., speculative helpers that a later task didn't need), remove them.

## Changes

- **`Cargo.toml`** (workspace root): Add `ironpad-cli` to members, audit `[workspace.dependencies]`
- **`Makefile.toml`**: Add `build-cli` task, verify `ci`/`uat` coverage
- **`.github/workflows/build.yml`**: Add explicit CLI build step if needed
- **`docker/Dockerfile`**: Copy `ironpad-cli` binary into runtime image
- **`crates/ironpad-server/src/main.rs`**: Verify session + WS integration
- **`crates/ironpad-app/Cargo.toml`**: Verify `web-sys` feature flags
- **`tests/e2e/`**: Verify test infrastructure prerequisites
- **`README.md`**: Add CLI, WebSocket, session, and collaboration documentation
- **`AGENTS.md`**: Update crate count, task table, architecture, routes, file organization, common tasks

## Dependencies

- **All previous tasks** (0001–0010)

## Acceptance Criteria

- `cargo make ci` passes with zero warnings across all crates including `ironpad-cli`
- `cargo make uat` passes (existing tests don't regress, new tests pass)
- `cargo build --release -p ironpad-cli` produces a working binary
- Docker image builds successfully and includes both `ironpad-server` and `ironpad-cli`
- No unused workspace dependencies
- No dead code warnings from clippy
- All new modules compile under both `ssr` and `hydrate` feature sets (as appropriate)
- README documents the CLI, WebSocket protocol, session lifecycle, and agent workflow
- AGENTS.md reflects the new crate, routes, architecture, and development workflows
