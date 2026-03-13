# 0003: Cell Versioning (OCC)

## Summary

Add optimistic concurrency control to cells via monotonic version counters. This prevents the agent and user from silently overwriting each other's edits.

## Motivation

Without versioning, last-write-wins. If the user is editing a cell in the browser and the agent sends an update at the same time, one edit is silently lost. OCC makes this explicit: the loser gets a `VersionConflict` error and can re-read and retry.

## Design

### Version field

Add `version: u64` to `IronpadCell`:

```rust
pub struct IronpadCell {
    pub id: String,
    pub order: u32,
    pub label: String,
    pub cell_type: CellType,
    pub source: String,
    pub cargo_toml: Option<String>,
    #[serde(default)]
    pub version: u64,           // new
}
```

`#[serde(default)]` ensures backward compatibility with existing `.ironpad` files and IndexedDB records that don't have a version field (they'll deserialize as 0).

### Behavior

1. Every cell starts at version 0
2. `Mutation::CellUpdate { version: expected }` — the model checks `cell.version == expected`. If not, returns `Err(VersionConflict { expected, actual })`
3. On successful update, `cell.version += 1`
4. The `CellUpdated` event carries the new version
5. `Mutation::CellDelete { version }` — same check. Prevents deleting a cell that was just modified.
6. `Mutation::CellAdd` — new cell starts at version 0, no check needed
7. `Mutation::CellReorder` — no per-cell version check (ordering is a notebook-level concern, not a cell-content concern)

### Browser-side handling

When the browser's own update fails with `VersionConflict` (because an agent mutation arrived first), the model should:

1. Re-read the current cell content
2. Signal the UI to refresh the Monaco editor content
3. The user sees the agent's edit appear and can continue from there

When the agent's update fails, the CLI returns the conflict error with the current version and source, so the agent can retry.

### Version in the UI

The UI doesn't display versions to the user. Versions are internal bookkeeping. The UI just needs to track the current version for each cell so it can include it in mutations.

## Changes

- **`crates/ironpad-common/src/types.rs`**: Add `version: u64` to `IronpadCell` with `#[serde(default)]`
- **`crates/ironpad-app/src/model.rs`**: Version check + increment logic in `apply()`
- **`crates/ironpad-common/src/protocol.rs`**: Version fields already present in 0001 mutations/events

## Dependencies

- **0001** (protocol types reference version fields)
- **0002** (model abstraction is where version checks live)

## Acceptance Criteria

- `CellUpdate` with correct version succeeds and increments
- `CellUpdate` with stale version returns `VersionConflict` with actual version
- `CellDelete` with stale version returns `VersionConflict`
- Existing notebooks without version fields load correctly (default to 0)
- Round-trip serialization preserves version
