# 0008: CLI Daemon

## Summary

Create a new `ironpad-cli` crate with a long-lived daemon process that maintains a WebSocket connection to the ironpad server and exposes a local IPC interface for fast CLI commands.

## Motivation

An LLM agent will make many rapid CLI calls. Each call shouldn't pay the cost of establishing a WebSocket connection, authenticating, and syncing state. A daemon (like `sccache` or `buckd`) keeps the connection warm and responds to CLI commands over a local Unix socket.

## Design

### Architecture

```
ironpad-cli cells list --notebook <id>
     │
     ├─ Is daemon running? (check pidfile / connect to socket)
     │   ├─ No  → spawn daemon in background, wait for socket ready
     │   └─ Yes → connect to socket
     │
     ├─ Send request over Unix socket
     ├─ Receive response
     └─ Print to stdout (JSON)

ironpad-cli daemon
     │
     ├─ Parse --host, --token from args or env
     ├─ Connect WebSocket to server: ws(s)://{host}/ws/connect?token={token}
     ├─ Listen on Unix socket: ~/.ironpad/daemon.sock
     ├─ Event loop:
     │   ├─ WebSocket message → update local state cache, notify waiting CLI requests
     │   └─ Unix socket request → translate to protocol message → send on WebSocket → await response
     └─ Shutdown on: explicit stop, WebSocket close (session ended), SIGTERM
```

### Daemon state

The daemon maintains a local cache of the notebook state for fast reads:

```rust
struct DaemonState {
    notebook: Option<IronpadNotebook>,  // cached from initial sync + events
    ws: WebSocketStream,
    pending_requests: HashMap<String, oneshot::Sender<Response>>,
}
```

On connect, the daemon sends `Query::NotebookGet` to populate its cache. Subsequent events keep it updated. Read queries (`cells list`, `cells get`) are served from cache without hitting the WebSocket.

### IPC protocol

Simple JSON-over-Unix-socket, newline-delimited:

```
Client sends: { "command": "cells.list" }\n
Daemon responds: { "ok": true, "data": [...] }\n

Client sends: { "command": "cells.update", "cell_id": "abc", "source": "...", "version": 3 }\n
Daemon responds: { "ok": true, "data": { "version": 4 } }\n
```

### Pidfile and socket path

```
~/.ironpad/daemon.pid     — PID of running daemon
~/.ironpad/daemon.sock    — Unix domain socket
~/.ironpad/daemon.log     — Daemon log output
```

Multiple daemon instances (for different sessions) could be supported later by namespacing:
```
~/.ironpad/sessions/<session_id>/daemon.sock
```

For MVP, single daemon instance is fine.

### Auto-start

When the CLI runs a command and no daemon is running:
1. Spawn `ironpad-cli daemon --host <host> --token <token>` as a background process
2. Wait for `daemon.sock` to appear (poll with short timeout)
3. Connect and send the command

The `--host` and `--token` can come from:
- CLI args: `ironpad-cli --host ... --token ... cells list`
- Environment: `IRONPAD_HOST`, `IRONPAD_TOKEN`
- Config file: `~/.ironpad/config.toml`

### Shutdown

The daemon shuts down when:
- The WebSocket closes (server ended the session, or host browser closed)
- Explicit: `ironpad-cli daemon stop`
- SIGTERM/SIGINT
- Idle timeout (no CLI commands for 1 hour)

On shutdown, remove pidfile and socket.

### Logging

The daemon logs to `~/.ironpad/daemon.log`:
- Connection events
- Incoming/outgoing messages (at debug level)
- Errors

## Changes

- **`crates/ironpad-cli/`** (new crate):
  - `Cargo.toml`
  - `src/main.rs` — CLI entry point, subcommand dispatch
  - `src/daemon.rs` — Daemon process: WS connection, Unix socket listener, state cache
  - `src/ipc.rs` — IPC protocol types, client-side connection logic
  - `src/auto_start.rs` — Daemon auto-start logic
- **`Cargo.toml` (workspace)**: Add `ironpad-cli` to members
- **Dependencies**: `tokio`, `clap`, `serde_json`, `tokio-tungstenite`, `ironpad-common`

## Dependencies

- **0001** (message protocol for WebSocket communication)
- **0005** (server WebSocket endpoint to connect to)

## Acceptance Criteria

- `ironpad-cli daemon --host <host> --token <token>` starts and connects
- Daemon maintains WebSocket connection with heartbeat
- Unix socket accepts connections and processes commands
- Local state cache serves read queries without WebSocket round-trip
- WebSocket events update the local cache
- Daemon auto-starts when CLI commands are run without a running daemon
- Clean shutdown on session end, SIGTERM, or explicit stop
- Pidfile and socket are cleaned up on shutdown
