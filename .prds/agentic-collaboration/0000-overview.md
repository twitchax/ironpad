# 0000: Overview — Collaborative Notebooks & Agent CLI

## Vision

A user running a private notebook in their browser constitutes a **session host**. The host can vend a short-lived token to an LLM agent, which connects via a CLI daemon. The agent reads, writes, and observes cells through the same protocol the browser uses — mutations from the user and the agent are not fundamentally different channels, just different sources of messages into the same notebook model.

The browser remains the authoritative model server. The ironpad API server is a dumb relay. The CLI daemon keeps a warm WebSocket connection for fast agent interactions.

Execution stays human-triggered by default. The agent writes code; the human reviews and runs it.

## Architecture

```
                   ┌──────────────────────┐
                   │       Browser        │
                   │  ┌────────────────┐  │
                   │  │ Notebook Model │  │  ← source of truth
                   │  │  (IndexedDB +  │  │
                   │  │   in-memory)   │  │
                   │  └───────┬────────┘  │
                   │          │           │
                   │  Unified Message     │
                   │  Protocol (0001)     │
                   └─────┬────────────────┘
                         │ WebSocket
                   ┌─────┴─────┐
                   │ API Server│  ← relay + compilation + session/token mgmt
                   └─────┬─────┘
                         │ WebSocket
                   ┌─────┴─────┐
                   │ CLI Daemon│  ← warm connection, local state cache
                   └─────┬─────┘
                         │ Unix socket IPC
                   ┌─────┴─────┐
                   │   Agent   │
                   └───────────┘
```

## Dependency Tree

```
0001 Message Protocol
 │
 ├──► 0002 Notebook Model Abstraction
 │     │
 │     └──► 0003 Cell Versioning (OCC)
 │           │
 │           └──► 0006 Browser WebSocket Integration ◄── 0005
 │                 │
 │                 └──► 0007 Browser Session UI ◄── 0004
 │
 ├──► 0004 Session & Token Management
 │     │
 │     └──► 0005 Server WebSocket Relay ◄── 0001
 │           │
 │           ├──► 0006 (see above)
 │           │
 │           └──► 0008 CLI Daemon ◄── 0001
 │                 │
 │                 └──► 0009 CLI Client
 │
 └──► 0010 Integration Testing (depends on all)
```

## Critical Path

```
0001 → 0002 → 0003 → 0006 → 0007
```

The browser-side model refactor (0002) is the longest chain and the biggest architectural bet. Everything else follows from it.

## Parallelism

Once **0001** lands:
- **0002** (model abstraction) and **0004** (session/token) can proceed in parallel
- **0005** (server WS) can start once 0004 is done, independent of 0002/0003

Once **0005** lands:
- **0008** (CLI daemon) can proceed independent of browser-side work (0006/0007)

## Task Index

| Task | Title | Status | Key Change |
|------|-------|--------|------------|
| [0001](./0001-message-protocol.md) | Message Protocol | **done** | Shared types in `ironpad-common` for all mutations, queries, events |
| [0002](./0002-notebook-model-abstraction.md) | Notebook Model Abstraction | **done** | Extract mutations into unified `NotebookModel` — the architectural centerpiece |
| [0003](./0003-cell-versioning.md) | Cell Versioning (OCC) | **done** | Monotonic version counters on cells for conflict detection |
| [0004](./0004-session-and-token-management.md) | Session & Token Management | **done** | Server-side session store, crypto-random tokens, permission scoping |
| [0005](./0005-server-websocket-relay.md) | Server WebSocket Relay | **done** | WebSocket upgrade routes in Axum, message routing between host and guests |
| [0006](./0006-browser-websocket-integration.md) | Browser WebSocket Integration | not started | Browser connects to server WS, bridges messages to/from NotebookModel |
| [0007](./0007-browser-session-ui.md) | Browser Session UI | not started | Start/stop session, token display, agent activity indicators |
| [0008](./0008-cli-daemon.md) | CLI Daemon | not started | Long-lived process with WS connection and Unix socket IPC |
| [0009](./0009-cli-client.md) | CLI Client | not started | Agent-facing subcommands with JSON output |
| [0010](./0010-integration-testing.md) | Integration Testing | not started | End-to-end Playwright + CLI subprocess tests |
| [0011](./0011-repo-alignment.md) | Repo Alignment | not started | Final sweep: workspace manifest, CI, Docker, cargo-make, dependency hygiene, dead code audit |
