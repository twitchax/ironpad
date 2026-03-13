# 0005: Server WebSocket Relay

## Summary

Add WebSocket support to the Axum server. The server acts as a dumb relay between browser clients (session hosts) and CLI clients (session guests), with session/token validation and permission enforcement.

## Motivation

The API server sits between the browser and the CLI. It doesn't own notebook state — it just routes messages and enforces access control. WebSocket gives us the persistent bidirectional channel needed for real-time sync.

## Design

### Routes

```
GET /ws/host?notebook_id=<id>           → Browser connects as session host
GET /ws/connect?token=<token>           → CLI connects as session guest
```

### Connection types

**Host connection (browser):**
- One per notebook session
- Authenticated implicitly (the browser is the user who opened the notebook)
- Can create/destroy sessions
- Receives mutations from guests, sends events back
- If this connection drops, all sessions for this notebook are invalidated

**Guest connection (CLI/agent):**
- Authenticated via session token (query param)
- Can send mutations (gated by permissions)
- Receives events broadcast by the host
- Multiple guests can connect to the same session

### Server state

```rust
struct WsState {
    sessions: SessionStore,                                    // from 0004
    host_connections: HashMap<String, HostConnection>,         // notebook_id → host ws sender
    guest_connections: HashMap<String, Vec<GuestConnection>>,  // session_id → guest ws senders
}

struct HostConnection {
    notebook_id: String,
    sender: SplitSink<WebSocket, WsMessage>,
}

struct GuestConnection {
    session_id: String,
    client_id: String,
    sender: SplitSink<WebSocket, WsMessage>,
}
```

### Message flow

**Guest → Host (mutation):**
```
1. Guest sends Message { kind: Mutation(..) } on its WebSocket
2. Server receives, checks permissions (0004)
3. Server forwards to the host connection for that session's notebook
4. Host (browser) processes mutation via NotebookModel (0002)
5. Host sends back EventEnvelope
6. Server broadcasts EventEnvelope to ALL guests on that session + back to host
```

**Host → Guests (event from browser UI):**
```
1. Browser user makes an edit → NotebookModel emits Event
2. Browser sends EventEnvelope on its host WebSocket
3. Server broadcasts to all guests on sessions for that notebook
```

**Guest → Server (query):**
```
1. Guest sends Message { kind: Query(..) }
2. Server forwards to host
3. Host responds with Response
4. Server forwards Response back to the requesting guest only (not broadcast)
```

### Heartbeat

- Server sends WebSocket Ping every 30 seconds
- If no Pong within 10 seconds, connection is considered dead
- Dead host connection → invalidate all sessions for that notebook
- Dead guest connection → remove from guest list

### Session management messages

These are not part of the notebook protocol — they're control messages between browser and server:

```rust
enum ControlMessage {
    CreateSession { permissions: Permissions },      // host → server
    SessionCreated { session_id: String, token: String },  // server → host
    EndSession { session_id: String },               // host → server
    SessionEnded { session_id: String },             // server → host + guests
    GuestConnected { client_id: String },            // server → host
    GuestDisconnected { client_id: String },         // server → host
}
```

### Axum integration

Add WebSocket routes alongside the existing Leptos routes:

```rust
let app = Router::new()
    .route("/ws/host", get(ws_host_handler))
    .route("/ws/connect", get(ws_connect_handler))
    .leptos_routes_with_context(...)
    .with_state(app_state);  // AppState now includes WsState
```

The existing `LeptosOptions` state needs to be combined with `WsState` into a shared `AppState`.

### Error handling

- Invalid token → WebSocket close with code 4001, reason "invalid token"
- Expired session → close 4002, "session expired"
- Permission denied → error event (don't close connection)
- Host disconnected → close 4003, "session host disconnected"

## Changes

- **`crates/ironpad-server/src/ws.rs`** (new): WebSocket handlers, message routing
- **`crates/ironpad-server/src/state.rs`** (new): `AppState` combining `WsState` + `LeptosOptions` + `AppConfig`
- **`crates/ironpad-server/src/main.rs`**: Add WS routes, initialize `AppState`
- **`crates/ironpad-server/Cargo.toml`**: Add `tokio-tungstenite` or use axum's built-in WS (axum already supports it via `axum::extract::ws`)

## Dependencies

- **0001** (message types for routing)
- **0004** (session/token validation)

## Acceptance Criteria

- Browser can connect as host via `/ws/host`
- CLI can connect as guest via `/ws/connect?token=...`
- Mutations from guest are forwarded to host
- Events from host are broadcast to all guests
- Invalid/expired tokens are rejected at connection time
- Host disconnect invalidates all associated sessions and disconnects guests
- Heartbeat detects dead connections within 40 seconds
