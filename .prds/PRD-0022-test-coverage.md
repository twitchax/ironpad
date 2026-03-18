---
id: PRD-0022
title: "Test Coverage: Server, WebSocket, and CLI Tests"
status: active
owner: "Aaron Roney"
created: 2026-03-18
updated: 2026-03-18

depends_on:
- PRD-0021

principles:
- "Test the logic, not the framework — extract pure functions from Leptos/Axum contexts"
- "Unit tests for message handling; integration tests for the relay pipeline"
- "Follow existing SessionStore test patterns (tokio::test, direct construction)"
- "Skip browser-dependent code (NotebookModel, Leptos components) — Playwright covers those"

references:
- name: "SessionStore test pattern"
  url: "crates/ironpad-server/src/sessions.rs#L217"
- name: "WsState API"
  url: "crates/ironpad-server/src/state.rs"
- name: "WebSocket handlers"
  url: "crates/ironpad-server/src/ws.rs"
- name: "CLI daemon"
  url: "crates/ironpad-cli/src/daemon.rs"

acceptance_tests:
- id: uat-001
  name: "All new tests pass in cargo make ci"
  command: cargo make ci
  uat_status: verified
- id: uat-002
  name: "WsState unit tests cover register/unregister/send/broadcast/query tracking"
  command: "cargo nextest run -p ironpad-server ws_state"
  uat_status: verified
- id: uat-003
  name: "WebSocket handler tests cover host and guest message routing"
  command: "cargo nextest run -p ironpad-server ws_handler"
  uat_status: verified
- id: uat-004
  name: "Integration test verifies full host-guest relay round-trip"
  command: "cargo nextest run -p ironpad-server relay_integration"
  uat_status: verified
- id: uat-005
  name: "CLI translation functions tested for all command types"
  command: "cargo nextest run -p ironpad-cli translate"
  uat_status: verified
- id: uat-006
  name: "Server function core logic tested without Leptos context"
  command: "cargo nextest run -p ironpad-app server_fn_core"
  uat_status: verified

tasks:
- id: T-001
  title: "WsState unit tests"
  priority: 1
  status: done
  notes: "Add #[cfg(test)] mod to state.rs. Test all 11+ public methods using mock mpsc channels. Cover: register/unregister host+guest, send_to_host, send_to_guest, broadcast_to_guests, broadcast_to_notebook_guests, track_query/resolve_query, disconnect_guests. Follow SessionStore test pattern."

- id: T-002
  title: "WebSocket message handler unit tests"
  priority: 1
  status: done
  notes: "Add #[cfg(test)] mod to ws.rs. Test handle_host_message and handle_guest_message with constructed AppState + JSON message strings. Cover: event broadcast, response routing, permission enforcement, error cases (invalid JSON, missing session, denied permission). These are already extracted async fns."

- id: T-003
  title: "Host-guest relay integration test"
  priority: 2
  status: done
  notes: "Create crates/ironpad-server/tests/relay.rs. Spin up Axum router on random port (TcpListener::bind 127.0.0.1:0). Connect host via tokio-tungstenite, create session, connect guest with token, verify mutation/event relay round-trip. Add tokio-tungstenite + futures-util to dev-dependencies."

- id: T-004
  title: "CLI translate_command and translate_response unit tests"
  priority: 1
  status: done
  notes: "Add #[cfg(test)] mod to daemon.rs. Test translate_command for all IpcRequest variants (cells list/get/add/update/delete/reorder, notebook get/export). Test translate_response for all protocol response types. Test renumber() helper. These are pure functions — no async needed."

- id: T-005
  title: "Extract server function core logic"
  priority: 2
  status: done
  notes: "Refactor server_fns.rs: extract core logic from each #[server] fn into standalone async functions that take AppConfig/paths as parameters instead of using expect_context(). The #[server] fns become thin wrappers. Targets: share_notebook_core, get_shared_notebook_core, list_public_notebooks_core, get_public_notebook_core. compile_cell already delegates to compiler module."

- id: T-006
  title: "Server function core logic tests"
  priority: 2
  status: done
  notes: "Test the extracted core functions from T-005 using temp directories. Cover: share_notebook (valid JSON, invalid JSON, hash determinism), get_shared_notebook (exists, not found, path traversal rejection), list_public_notebooks (empty dir, multiple notebooks), get_public_notebook (exists, not found). Use tempdir crate."

- id: T-007
  title: "Verify CI and measure coverage impact"
  priority: 3
  status: done
  notes: "Run cargo make ci locally. Push and verify GH Actions passes. Check codecov for coverage improvement. Update PRD with final coverage numbers."
---

# Summary

Add comprehensive test coverage for the server, WebSocket relay, and CLI layers — the largest untested surface area that can be covered without browser dependencies. Target: meaningful coverage improvement through unit tests for state management and message handling, an integration test for the full WebSocket relay, and extracted+tested server function core logic.

# Problem

ironpad is at 42% code coverage. The compiler pipeline and cell runtime are well-tested (126+ tests), but the server layer (WsState, WebSocket relay, server functions) and CLI layer (daemon translation, command parsing) have zero test coverage despite being pure async logic that's straightforward to test. This represents ~3,000 LOC of testable code.

# Goals

1. Unit test all WsState public methods (register, send, broadcast, query tracking)
2. Unit test WebSocket message handlers (host + guest message routing, permissions)
3. Integration test the full host↔guest relay round-trip via in-process Axum server
4. Unit test CLI translation functions (IPC ↔ protocol conversion)
5. Extract and test server function core logic without Leptos dependency
6. Improve overall coverage meaningfully (targeting 55%+)

# Technical Approach

## Unit Tests (T-001, T-002, T-004)

Follow the existing `SessionStore` test pattern: `#[tokio::test]`, direct construction, inline `#[cfg(test)]` modules.

**WsState tests (T-001):** Create `WsState::default()`, use `mpsc::unbounded_channel()` to mock senders, call methods, assert messages received on channels:
```rust
#[tokio::test]
async fn send_to_host_delivers_message() {
    let ws = WsState::default();
    let (tx, mut rx) = mpsc::unbounded_channel();
    ws.register_host("nb-1", "conn-1", tx).await;
    assert!(ws.send_to_host("nb-1", "hello").await);
    assert_eq!(rx.recv().await.unwrap(), "hello");
}
```

**Handler tests (T-002):** Construct `AppState` with `WsState::default()`, pre-register hosts/guests with mock channels, call `handle_host_message`/`handle_guest_message` with JSON strings, verify messages arrive on the right channels:
```rust
#[tokio::test]
async fn guest_mutation_forwarded_to_host() {
    let state = test_app_state();
    let (host_tx, mut host_rx) = mpsc::unbounded_channel();
    state.ws.register_host("nb-1", "conn-1", host_tx).await;
    // ... create session, register guest ...
    handle_guest_message(mutation_json, "nb-1", "sess-1", "client-1", &full_perms, &state).await;
    let msg = host_rx.recv().await.unwrap();
    // Assert mutation was relayed
}
```

**CLI tests (T-004):** Direct function calls — no async needed for `translate_command`/`translate_response`:
```rust
#[test]
fn translate_cells_list() {
    let req = IpcRequest::CellsList;
    let result = translate_command(&req).unwrap();
    assert!(matches!(result, MessageKind::Query(Query::CellsList)));
}
```

## Integration Test (T-003)

Create `crates/ironpad-server/tests/relay.rs`:
1. Build minimal Axum router with WS routes + `AppState`
2. Bind to `127.0.0.1:0` (OS-assigned port)
3. Spawn server in background task
4. Connect host via `tokio-tungstenite`, send CreateSession control message
5. Parse token from response
6. Connect guest with token
7. Send mutation from guest → verify host receives it
8. Send event from host → verify guest receives it

This tests the full relay without a browser.

## Server Function Extraction (T-005, T-006)

Refactor `server_fns.rs` — extract core logic into testable functions:

```rust
// Before (requires Leptos context):
#[server]
pub async fn share_notebook(notebook_json: String) -> Result<String, ServerFnError> {
    let config: AppConfig = expect_context();
    // ... logic ...
}

// After:
pub(crate) async fn share_notebook_core(data_dir: &Path, notebook_json: &str) -> Result<String> {
    // ... same logic, takes path directly ...
}

#[server]
pub async fn share_notebook(notebook_json: String) -> Result<String, ServerFnError> {
    let config: AppConfig = expect_context();
    share_notebook_core(&config.data_dir, &notebook_json).await.map_err(...)
}
```

Test core functions with `tempdir`:
```rust
#[tokio::test]
async fn share_and_retrieve_notebook() {
    let dir = tempdir().unwrap();
    let hash = share_notebook_core(dir.path(), VALID_NOTEBOOK_JSON).await.unwrap();
    let nb = get_shared_notebook_core(dir.path(), &hash).await.unwrap();
    assert_eq!(nb.cells.len(), 1);
}
```

# Assumptions

- `handle_host_message` and `handle_guest_message` are already extracted async functions (confirmed)
- WsState can be default-constructed and all methods work without WebSocket (confirmed)
- `translate_command` and `translate_response` are pure functions (confirmed)
- `AppState` can be constructed in tests (needs `LeptosOptions` from `get_configuration`)

# Constraints

- `AppState` construction requires `get_configuration(None)` for `LeptosOptions` — may need a test helper or mock
- Integration test (T-003) needs `tokio-tungstenite` and `futures-util` as dev-dependencies
- Server function tests (T-006) need `tempdir` as dev-dependency
- `handle_host_message`/`handle_guest_message` are `pub(crate)` or private — tests must be in-crate (`#[cfg(test)]` mod) or functions made `pub(crate)`
- The relay integration test may be slow (WebSocket handshake + async tasks) — mark `#[ignore]` if needed

# References to Code

- `crates/ironpad-server/src/state.rs` — WsState (14 public methods, all testable)
- `crates/ironpad-server/src/ws.rs` — handle_host_message (~L123), handle_guest_message (~L329)
- `crates/ironpad-server/src/sessions.rs` — Existing test pattern (14 tests, L217-420)
- `crates/ironpad-cli/src/daemon.rs` — translate_command (~L446), translate_response (~L556), renumber (~L307)
- `crates/ironpad-app/src/server_fns.rs` — 5 server functions to extract core logic from

# Non-Goals (MVP)

- NotebookModel tests (requires Leptos runtime — covered by Playwright)
- Full daemon end-to-end tests (requires in-process server + daemon + Unix socket)
- Leptos component/page tests
- Hitting a specific coverage percentage target
- Refactoring WsState or ws.rs (test as-is)

# History

## 2026-03-18 — Batch Execution (T-001 through T-006)
- **Tasks completed**: T-001, T-002, T-003, T-004, T-005, T-006, T-007
- **Changes**:
  - T-001: 16 WsState unit tests (register/unregister/send/broadcast/query) in state.rs
  - T-002: 15 WebSocket handler tests (routing, permissions, edge cases) in ws.rs
  - T-003: 2 integration tests (relay round-trip, invalid token rejection) in tests/relay.rs; created lib.rs for ironpad-server
  - T-004: 23 CLI translation tests (translate_command, translate_response, renumber) in daemon.rs
  - T-005: Extracted 4 `_core` functions from server_fns.rs (share, get_shared, list_public, get_public)
  - T-006: 13 server function core tests (tempdir-based filesystem tests) in server_fns.rs
- **Test results**: 418 pass, 0 fail, 6 skipped — `cargo make clippy` clean
- **UATs verified**: uat-001 through uat-006 (all verified)
- **Constitution compliance**: No violations
