# 0006: Browser WebSocket Integration

## Summary

Connect the browser to the API server via WebSocket when an agent session is active. Bridge incoming protocol messages to the `NotebookModel` and outgoing model events to the WebSocket.

## Motivation

The browser is the model server. When a session is active, it needs to:
1. Listen for mutations from the agent (via the server relay)
2. Apply them to the NotebookModel (same codepath as user edits)
3. Send events back so the agent sees the result

## Design

### When to connect

The browser does NOT maintain a WebSocket at all times. It only connects when the user starts an agent session (0007). The WebSocket lifecycle is tied to the session lifecycle.

### Connection setup

```rust
// In browser (hydrate feature)
pub struct SessionConnection {
    ws: WebSocket,                      // web_sys::WebSocket
    session_id: String,
    on_message: Closure<dyn FnMut(MessageEvent)>,
}
```

On session start:
1. Open WebSocket to `ws(s)://{host}/ws/host?notebook_id={id}`
2. Send `ControlMessage::CreateSession { permissions }`
3. Receive `SessionCreated { session_id, token }`
4. Display token to user (0007)
5. Start listening for incoming messages

### Incoming message handling

```
WebSocket message received
  ↓
Deserialize as Message
  ↓
Match on kind:
  Mutation → model.apply(mutation, ClientId::agent(client_id))
             → Event emitted → broadcast back via WS
  Query   → model.query(query) → send Response back via WS
  Control → handle session lifecycle (GuestConnected, etc.)
```

The key insight: `model.apply()` is the same call the UI makes. The browser doesn't know or care that this mutation came from a WebSocket — it just processes it like any other edit.

### Outgoing event handling

When the user makes an edit in the browser:
1. UI calls `model.apply(mutation, ClientId::browser())`
2. Model emits `EventEnvelope { event, by: browser }`
3. The WebSocket bridge picks up the event and sends it to the server
4. Server relays to all connected guests

This means the browser needs to subscribe to model events and forward them over the WebSocket. An `Effect` watching the model's event signal works:

```rust
Effect::new(move || {
    let events = model.pending_events.get();
    for event in events {
        ws.send_with_str(&serde_json::to_string(&event).unwrap());
    }
});
```

### Reconnection

If the WebSocket drops unexpectedly:
- Show a "disconnected" indicator in the UI
- Attempt reconnect with exponential backoff (1s, 2s, 4s, max 30s)
- On reconnect, re-send `CreateSession` (server will have invalidated the old session)
- New token is generated — but existing agents need the user to provide the new token
- Alternative: store a reconnect secret so the server can restore the session silently

For MVP, simple reconnect with new token is fine. The user just copies the new token.

### Connection lifecycle

```
Session start → open WS → listen for messages + forward events
Session end   → send EndSession → close WS → stop forwarding
Tab close     → WS drops → server invalidates session automatically
```

## Changes

- **`crates/ironpad-app/src/session.rs`** (new, hydrate-only): `SessionConnection`, connect/disconnect, message handling
- **`crates/ironpad-app/src/model.rs`**: Add event subscription mechanism (signal or callback)
- **`crates/ironpad-app/Cargo.toml`**: Ensure `web-sys` features include `WebSocket`, `MessageEvent`, `CloseEvent`

## Dependencies

- **0001** (message types)
- **0002** (NotebookModel — mutations and event subscription)
- **0005** (server WebSocket endpoint to connect to)

## Acceptance Criteria

- Browser connects to `/ws/host` when session starts
- Incoming mutations are applied via `NotebookModel::apply()`
- Model events from browser edits are sent over WebSocket
- Guest connect/disconnect control messages update session state
- WebSocket close triggers session cleanup
- Reconnection works with exponential backoff
