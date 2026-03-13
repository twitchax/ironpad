# 0002: Notebook Model Abstraction

## Summary

Extract all notebook mutation logic into a unified `NotebookModel` that both the browser UI and WebSocket handler call. This is the architectural centerpiece — it makes the user's insight concrete: mutations from any channel go through the same codepath.

## Motivation

Today, notebook mutations are scattered across UI code:

- `add_cell_to_notebook()` in `state.rs`
- Cell source updates via Monaco `onChange` + debounce in `cell_item.rs`
- Cell deletion via button handler in `cell_item.rs`
- Cell reorder via SortableJS `onEnd` callback
- Compilation trigger via `run_trigger` signal in `cell_item.rs`

Each of these directly manipulates `NotebookState` signals. To support multiple input channels, we need a single entry point for all mutations that:

1. Validates the mutation (OCC version check, cell existence, etc.)
2. Applies it to the in-memory model
3. Emits an event that all consumers (UI signals, WebSocket, persistence) observe

## Design

### The model

```rust
/// Owns the canonical notebook state. All mutations go through here.
pub struct NotebookModel {
    notebook: RwSignal<Option<IronpadNotebook>>,
    cell_versions: RwSignal<HashMap<String, u64>>,
    event_tx: /* channel or callback for broadcasting events */,
}
```

### Mutation methods

Each method:
- Takes a `Mutation` (from 0001) + `ClientId`
- Validates (version check, cell existence)
- Applies the change to the `notebook` signal
- Emits an `Event` with the `ClientId` as `by`
- Returns `Result<Event, ProtocolError>`

```rust
impl NotebookModel {
    pub fn apply(&self, mutation: Mutation, by: ClientId) -> Result<Event, ProtocolError>;
    pub fn query(&self, query: Query) -> Result<Response, ProtocolError>;
}
```

### Event distribution

The model emits events. Consumers subscribe:

- **UI**: An effect watches events, updates the relevant Leptos signals (active cell, cell outputs, etc.) and re-renders.
- **WebSocket**: Serializes the event and sends it to the server for relay.
- **Persistence**: Debounced save to IndexedDB on any mutation event.

This can be a simple callback list, a Leptos signal, or an async channel — whatever fits Leptos idioms best. A `RwSignal<Vec<EventEnvelope>>` that consumers watch via `Effect` is the simplest approach.

### Refactoring the UI

The existing UI code changes from:

```rust
// Before: directly manipulate notebook signal
state.notebook.update(|nb| {
    nb.cells.push(new_cell);
    renumber(&mut nb.cells);
});
persist_notebook(&state);
```

To:

```rust
// After: go through the model
let result = model.apply(Mutation::CellAdd { cell, after_cell_id }, ClientId::browser());
// Event is automatically broadcast; UI updates reactively
```

The Monaco `onChange` debounce still exists, but when it fires, it calls `model.apply(Mutation::CellUpdate { ... })` instead of directly patching the signal.

### What stays in the UI

- Debounce logic (UI concern, not model concern)
- Focus management (`pending_focus_cell`, `active_cell`)
- View mode toggle
- Editor handle management
- Cell status (compiling/running/queued) — these are UI-local transient states

### What moves into the model

- Cell CRUD (add, update source/cargo_toml, delete)
- Cell reordering
- Notebook metadata updates
- Version tracking
- Persistence triggering (as an event consumer)

## Changes

- **`crates/ironpad-app/src/model.rs`** (new): `NotebookModel` implementation
- **`crates/ironpad-app/src/pages/notebook_editor/state.rs`**: `NotebookState` holds a `NotebookModel` reference; remove direct mutation logic
- **`crates/ironpad-app/src/pages/notebook_editor/cell_item.rs`**: Refactor mutation calls to go through model
- **`crates/ironpad-app/src/pages/notebook_editor/mod.rs`**: Wire up model creation, event subscription

## Dependencies

- **0001** (message protocol types)

## Acceptance Criteria

- All cell mutations (add, update, delete, reorder) go through `NotebookModel::apply()`
- Existing UI behavior is unchanged (same user experience, same debounce, same persistence)
- Events are emitted for every mutation
- Model can be called from non-UI code (no dependency on DOM or browser APIs in the model itself)
- Existing tests and E2E suite pass without modification
