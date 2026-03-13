# 0004: Session & Token Management

## Summary

Add server-side session and token infrastructure so the browser can vend short-lived, scoped tokens that authorize CLI/agent access to a specific notebook.

## Motivation

The browser is the model server. But the CLI can't talk directly to the browser — it goes through the ironpad API server. The API server needs to know: "is this CLI client allowed to talk to this notebook?" Sessions and tokens answer that question.

## Design

### Session lifecycle

```
1. User clicks "Start Agent Session" in browser
2. Browser sends CreateSession request to API server (via WebSocket, see 0005)
3. Server generates:
   - session_id: UUID
   - token: crypto-random 32-byte hex string
   - Stores session in memory: { session_id, notebook_id, token, browser_ws_id, created_at, permissions }
4. Server returns { session_id, token } to browser
5. Browser displays token to user (copy-to-clipboard)
6. User gives token to agent: `ironpad connect --token <token>`
7. CLI daemon connects to server WebSocket with token in handshake
8. Server validates token, links CLI connection to session
9. Messages flow: CLI ↔ Server ↔ Browser

Teardown:
- User clicks "End Session" → browser sends EndSession → server invalidates token, drops CLI connections
- Browser WebSocket disconnects (tab close) → server invalidates all sessions for that browser connection
- Token expiry (configurable, default 24h) → server invalidates
```

### Types

```rust
pub struct Session {
    pub id: String,
    pub notebook_id: String,
    pub token_hash: String,         // store hash, not plaintext
    pub permissions: Permissions,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub browser_connection_id: String,
}

pub struct Permissions {
    pub read: bool,     // can query cells, notebook state
    pub write: bool,    // can mutate cells
    pub execute: bool,  // can trigger compile/execute
}

// Default agent permissions: read + write, no execute
impl Default for Permissions {
    fn default() -> Self {
        Self { read: true, write: true, execute: false }
    }
}
```

### Token properties

- 32 bytes of `rand::OsRng`, hex-encoded → 64 character string
- Stored as blake3 hash server-side (token is only shown once, to the user)
- Scoped to a single notebook
- Scoped to a single browser session (if browser disconnects, token dies)
- Default TTL: 24 hours (configurable)
- Permissions set at creation time

### Server-side session store

In-memory `HashMap<String, Session>` behind `Arc<RwLock<>>`. Shared via Axum state.

No persistence needed — sessions are ephemeral by design. Server restart = all sessions invalidated = agents must reconnect. This is correct: the browser (model server) also lost its WebSocket connection on restart.

### Token validation

When a CLI client connects:

1. Extract token from WebSocket handshake (query parameter or first message)
2. Hash it with blake3
3. Look up session by token hash
4. Check: not expired, browser still connected, permissions sufficient for requested operation
5. If valid: associate this WebSocket connection with the session
6. If invalid: close with appropriate error code

### Permission enforcement

The API server checks permissions before relaying mutations to the browser:

- `read` → can send `Query` messages
- `write` → can send `Mutation` messages (except `CellCompile`, `CellExecute`)
- `execute` → can send `CellCompile` and `CellExecute`

This is enforced at the server relay layer, not in the browser. Defense in depth: the browser can also check, but the server is the gatekeeper.

## Changes

- **`crates/ironpad-common/src/session.rs`** (new): `Session`, `Permissions`, `CreateSessionRequest`, `CreateSessionResponse` types
- **`crates/ironpad-server/src/sessions.rs`** (new): `SessionStore` (in-memory), token generation, validation
- **`crates/ironpad-server/src/main.rs`**: Add `SessionStore` to Axum shared state
- **`Cargo.toml` (workspace)**: Add `rand` dependency

## Dependencies

- **0001** (protocol types — `Mutation` variants determine permission checks)

## Acceptance Criteria

- Token generation produces cryptographically random 64-char hex strings
- Tokens are stored as blake3 hashes (plaintext never persisted)
- Session lookup by token hash works
- Expired sessions are rejected
- Sessions tied to a browser connection are invalidated when that connection drops
- Permission checks correctly gate mutation types
