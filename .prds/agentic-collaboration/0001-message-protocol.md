# 0001: Message Protocol

## Summary

Define a unified message protocol in `ironpad-common` that all clients (browser, CLI, future collaborators) use to interact with the notebook model. This is the foundation everything else builds on.

## Motivation

The user's browser and an agent's CLI are not fundamentally different channels — they're both sources of mutations and consumers of events on the same notebook model. A shared protocol makes this concrete in the type system.

## Design

### Envelope

Every message has a common envelope:

```rust
struct Message {
    id: String,         // correlation ID (for request/response pairing)
    kind: MessageKind,  // the payload
}
```

### Mutations (client → model)

These are requests to change state. Any client can send them.

```rust
enum Mutation {
    CellAdd {
        cell: NewCell,          // source, cell_type, label, cargo_toml
        after_cell_id: Option<String>,  // insert position (None = end)
    },
    CellUpdate {
        cell_id: String,
        source: String,
        cargo_toml: Option<String>,
        version: u64,           // OCC: expected current version
    },
    CellDelete {
        cell_id: String,
        version: u64,
    },
    CellReorder {
        cell_ids: Vec<String>,  // full ordered list
    },
    CellCompile {
        cell_id: String,
    },
    CellExecute {
        cell_id: String,
    },
    NotebookUpdateMeta {
        title: Option<String>,
        shared_cargo_toml: Option<String>,
        shared_source: Option<String>,
    },
}
```

### Queries (client → model)

Read-only requests:

```rust
enum Query {
    NotebookGet,
    CellGet { cell_id: String },
    CellsList,
    SessionStatus,
}
```

### Events (model → all clients)

Broadcast to every connected client (including the originator, for confirmation):

```rust
enum Event {
    CellAdded {
        cell: IronpadCell,
        after_cell_id: Option<String>,
        version: u64,
    },
    CellUpdated {
        cell_id: String,
        source: String,
        cargo_toml: Option<String>,
        version: u64,
    },
    CellDeleted {
        cell_id: String,
    },
    CellReordered {
        cell_ids: Vec<String>,
    },
    CellCompiling {
        cell_id: String,
    },
    CellCompiled {
        cell_id: String,
        diagnostics: Vec<Diagnostic>,
        success: bool,
    },
    CellExecuted {
        cell_id: String,
        display_text: Option<String>,
        type_tag: Option<String>,
        execution_time_ms: f64,
    },
    NotebookMetaUpdated {
        title: Option<String>,
        shared_cargo_toml: Option<String>,
        shared_source: Option<String>,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}
```

### Responses

Queries get direct responses:

```rust
enum Response {
    Notebook(IronpadNotebook),
    Cell(IronpadCell),
    CellsList(Vec<CellManifest>),
    SessionStatus {
        session_id: String,
        connected_clients: u32,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
}
```

### Error Codes

```rust
enum ErrorCode {
    VersionConflict,    // OCC: stale version
    CellNotFound,
    NotebookNotFound,
    PermissionDenied,   // token doesn't allow this operation
    InvalidMessage,
}
```

### Wire format

JSON over WebSocket. Each frame is a `Message` serialized with serde_json. The `MessageKind` enum covers `Mutation`, `Query`, `Event`, and `Response` variants.

### `by` field

Events carry an origin marker so clients can distinguish their own mutations from others':

```rust
struct EventEnvelope {
    id: String,             // correlation ID (matches the mutation that caused it)
    by: ClientId,           // who caused this
    event: Event,
}
```

`ClientId` is an opaque string — could be `"browser"`, `"agent:<token_prefix>"`, etc. The protocol doesn't ascribe meaning to it; the UI can use it for display purposes.

## Changes

- **`crates/ironpad-common/src/protocol.rs`** (new): All types above
- **`crates/ironpad-common/src/lib.rs`**: Add `pub mod protocol;`

## Dependencies

None — this is the base layer.

## Acceptance Criteria

- All protocol types compile and derive `Serialize`, `Deserialize`, `Clone`, `Debug`
- Types are usable from both `ssr` and `hydrate` feature contexts
- Round-trip JSON serialization tests pass for each message variant
