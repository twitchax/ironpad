//! Unified notebook model.
//!
//! All mutations to notebook state — whether from the browser UI or a remote
//! agent — go through [`NotebookModel::apply`]. This guarantees a single
//! codepath for state changes, version tracking, stale marking, and event
//! generation regardless of the mutation source.

use std::collections::HashMap;

use ironpad_common::notebook_ops;
use ironpad_common::protocol::*;
use ironpad_common::{CellManifest, CellType, IronpadCell, IronpadNotebook};
use leptos::prelude::*;

// ── Constants ───────────────────────────────────────────────────────────────

/// Maximum number of pending events buffered for the WebSocket bridge.
const MAX_EVENT_BUFFER: usize = 64;

// ── Error type ──────────────────────────────────────────────────────────────

/// Error returned by model operations.
// Fields accessed in session/connection.rs (hydrate-only); appear dead during SSR.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct ModelError {
    pub(crate) code: ErrorCode,
    pub(crate) message: String,
}

// ── Notebook model ──────────────────────────────────────────────────────────

/// Owns the canonical mutation interface to notebook state.
///
/// Created from the same [`RwSignal`]s that the UI reads, so reactive
/// rendering works unchanged. Provided as a separate Leptos context alongside
/// `NotebookState`.
#[derive(Clone, Copy)]
pub(crate) struct NotebookModel {
    notebook: RwSignal<Option<IronpadNotebook>>,
    cells: RwSignal<Vec<CellManifest>>,
    cell_stale: RwSignal<HashMap<String, bool>>,
    cell_versions: RwSignal<HashMap<String, u64>>,
    /// Events waiting to be sent over WebSocket. Subscribers watch
    /// `event_generation` (tracked) and drain this via `drain_events`.
    pending_events: RwSignal<Vec<EventEnvelope>>,
    /// Bumped each time events are pushed. The WebSocket bridge watches this.
    event_generation: RwSignal<u64>,
    /// Bumped when a remote (agent) edit changes a cell's content, so the host's
    /// editors can pull the new source into Monaco. Shared with `NotebookState`.
    external_content_generation: RwSignal<u64>,
}

/// Field bundle for [`NotebookModel::cell_update`] — mirrors
/// [`Mutation::CellUpdate`] so the handler stays readable as per-cell
/// attributes grow instead of accreting positional arguments.
#[allow(clippy::option_option)] // None = unchanged, Some(None) = clear, Some(Some(v)) = set.
struct CellUpdateFields {
    cell_id: String,
    source: Option<String>,
    cargo_toml: Option<Option<String>>,
    label: Option<String>,
    shared: Option<bool>,
    collapsed: Option<bool>,
    output_collapsed: Option<bool>,
    version: u64,
}

impl NotebookModel {
    /// Create a model backed by the given reactive signals.
    pub(crate) fn new(
        notebook: RwSignal<Option<IronpadNotebook>>,
        cells: RwSignal<Vec<CellManifest>>,
        cell_stale: RwSignal<HashMap<String, bool>>,
        external_content_generation: RwSignal<u64>,
    ) -> Self {
        Self {
            notebook,
            cells,
            cell_stale,
            cell_versions: RwSignal::new(HashMap::new()),
            pending_events: RwSignal::new(Vec::new()),
            event_generation: RwSignal::new(0),
            external_content_generation,
        }
    }

    // ── Public API ──────────────────────────────────────────────────────

    /// Apply a mutation to the notebook.
    ///
    /// Returns the result and an event envelope suitable for broadcasting.
    /// **Callers are responsible for persistence and UI-specific cleanup.**
    pub(crate) fn apply(
        &self,
        mutation: Mutation,
        by: ClientId,
    ) -> Result<(MutationResult, EventEnvelope), ModelError> {
        // A remote (non-browser) edit that changes a cell's content must refresh
        // the host's Monaco editor — the browser's own edits already live in
        // Monaco, so they don't. Detect it before `mutation` is moved below.
        let remote_content_edit = is_remote_content_edit(&by, &mutation);

        let (result, event) = match mutation {
            Mutation::CellAdd {
                cell,
                after_cell_id,
            } => self.cell_add(cell, after_cell_id)?,
            Mutation::CellUpdate {
                cell_id,
                source,
                cargo_toml,
                label,
                shared,
                collapsed,
                output_collapsed,
                version,
            } => self.cell_update(CellUpdateFields {
                cell_id,
                source,
                cargo_toml,
                label,
                shared,
                collapsed,
                output_collapsed,
                version,
            })?,
            Mutation::CellDelete { cell_id, version } => self.cell_delete(cell_id, version)?,
            Mutation::CellReorder { cell_ids } => self.cell_reorder(cell_ids)?,
            Mutation::NotebookUpdateMeta { meta } => self.notebook_update_meta(meta)?,
            // Not a state mutation: the session layer intercepts CellRun and
            // dispatches it to the run queue (PRD-0052). Reaching the model
            // is a routing bug, so refuse rather than silently no-op.
            Mutation::CellRun { .. } => {
                return Err(ModelError {
                    code: ErrorCode::InvalidMessage,
                    message: "CellRun is dispatched by the session layer, not the model".into(),
                });
            }
            // A mutation from a newer peer this build doesn't recognise. It
            // can't be applied — surface an error rather than silently acting.
            Mutation::Unknown => {
                return Err(ModelError {
                    code: ErrorCode::InvalidMessage,
                    message: "unrecognized mutation from a newer peer".into(),
                });
            }
        };
        let envelope = EventEnvelope { by, event };

        // Push event for the WebSocket bridge to pick up.
        //
        // Cap the buffer to prevent unbounded growth. Dropping past the cap is
        // safe by design: it only happens when nothing is draining — either no
        // session is active (a newly-connected guest fetches full state, so it
        // can't miss a pre-connection event), or the bridge has stalled. During
        // a healthy active session the bridge drains every generation, keeping
        // the buffer far below 64, so a connected guest never loses an event.
        self.pending_events.update(|events| {
            if events.len() < MAX_EVENT_BUFFER {
                events.push(envelope.clone());
            }
        });
        self.event_generation.update(|g| *g += 1);

        // Signal host editors to pull a remote content edit into Monaco.
        if remote_content_edit {
            self.external_content_generation.update(|g| *g += 1);
        }

        Ok((result, envelope))
    }

    /// Read-only queries against the notebook model.
    pub(crate) fn query(&self, query: Query) -> Result<Response, ModelError> {
        match query {
            Query::NotebookGet => {
                let nb = self.notebook.get_untracked().ok_or(ModelError {
                    code: ErrorCode::NotebookNotFound,
                    message: "No notebook loaded".into(),
                })?;
                Ok(Response::Notebook { notebook: nb })
            }
            Query::CellGet { cell_id } => {
                let nb = self.notebook.get_untracked().ok_or(ModelError {
                    code: ErrorCode::NotebookNotFound,
                    message: "No notebook loaded".into(),
                })?;
                let cell = nb
                    .cells
                    .iter()
                    .find(|c| c.id == cell_id)
                    .ok_or(ModelError {
                        code: ErrorCode::CellNotFound,
                        message: format!("Cell {cell_id} not found"),
                    })?;
                Ok(Response::Cell { cell: cell.clone() })
            }
            Query::CellsList => {
                let cells = self.cells.get_untracked();
                Ok(Response::CellsList { cells })
            }
            Query::Unknown => Err(ModelError {
                code: ErrorCode::InvalidMessage,
                message: "unrecognized query from a newer peer".into(),
            }),
        }
    }

    /// Get the current OCC version for a cell.
    pub(crate) fn cell_version(&self, cell_id: &str) -> u64 {
        self.cell_versions
            .get_untracked()
            .get(cell_id)
            .copied()
            .unwrap_or(0)
    }

    /// Reactive signal that bumps when new events are pushed.
    /// The WebSocket bridge watches this to know when to drain.
    pub(crate) fn event_generation(&self) -> Signal<u64> {
        self.event_generation.into()
    }

    /// Drain all pending events (untracked — won't re-trigger watchers).
    pub(crate) fn drain_events(&self) -> Vec<EventEnvelope> {
        let mut events = Vec::new();
        self.pending_events.update(|e| {
            events = std::mem::take(e);
        });
        events
    }

    /// Push a browser-originated event that did not come from a mutation —
    /// execution status (PRD-0052). Same buffer cap and generation bump as
    /// [`apply`](Self::apply), so the WebSocket bridge forwards it like any
    /// other event.
    pub(crate) fn emit_event(&self, event: Event) {
        let envelope = EventEnvelope {
            by: ClientId::browser(),
            event,
        };
        self.pending_events.update(|events| {
            if events.len() < MAX_EVENT_BUFFER {
                events.push(envelope);
            }
        });
        self.event_generation.update(|g| *g += 1);
    }

    // ── Internal helpers ────────────────────────────────────────────────

    /// Rebuild the `cells` signal (ordered `CellManifest` list) from the
    /// canonical `notebook` signal, and sync version tracking.
    pub(crate) fn sync_from_notebook(&self) {
        if let Some(nb) = self.notebook.get_untracked() {
            self.cells.set(
                nb.cells
                    .iter()
                    .map(|c| CellManifest {
                        id: c.id.clone(),
                        order: c.order,
                        label: c.label.clone(),
                        cell_type: c.cell_type.clone(),
                        shared: c.shared,
                        collapsed: c.collapsed,
                        output_collapsed: c.output_collapsed,
                    })
                    .collect(),
            );

            // Sync version map from persisted cell versions (e.g. after loading
            // a notebook from IndexedDB). Only populate entries that aren't
            // already tracked, so in-flight versions aren't overwritten.
            self.cell_versions.update(|versions| {
                for c in &nb.cells {
                    versions.entry(c.id.clone()).or_insert(c.version);
                }
            });
        }
    }

    /// Mark this cell and all downstream Code cells as stale.
    fn mark_downstream_stale(&self, from_cell_id: &str) {
        let cells = self.cells.get_untracked();
        // Policy: if the origin cell isn't in the derived list (e.g. it was
        // already removed), mark nothing. The alternative — treating "not found"
        // as index 0 via `unwrap_or(0)` — would silently mark *every* Code cell
        // stale on a lookup miss, over-invalidating downstream compiles.
        let Some(my_idx) = cells.iter().position(|c| c.id == from_cell_id) else {
            return;
        };
        self.cell_stale.update(|stale| {
            for cell in &cells[my_idx..] {
                // Shared cells never execute, so they are never stale.
                if cell.cell_type == CellType::Code && !cell.shared {
                    stale.insert(cell.id.clone(), true);
                }
            }
        });
    }

    /// Mark all Code cells as stale (e.g. shared deps changed).
    fn mark_all_code_cells_stale(&self) {
        self.cell_stale.update(|stale| {
            let cells = self.cells.get_untracked();
            for cell in &cells {
                // Shared cells never execute, so they are never stale.
                if cell.cell_type == CellType::Code && !cell.shared {
                    stale.insert(cell.id.clone(), true);
                }
            }
        });
    }

    // ── Mutation implementations ────────────────────────────────────────

    #[allow(clippy::unnecessary_wraps)] // Consistent with other mutation methods.
    fn cell_add(
        &self,
        new_cell: NewCell,
        after_cell_id: Option<String>,
    ) -> Result<(MutationResult, Event), ModelError> {
        let new_id = generate_cell_id(self.cells.get_untracked().len());

        let cargo_toml = if new_cell.cell_type == CellType::Code {
            new_cell
                .cargo_toml
                .or_else(|| Some(default_cell_cargo_toml(&new_id)))
        } else {
            None
        };

        let is_shared = new_cell.shared;
        let cell = IronpadCell {
            id: new_id.clone(),
            order: 0,
            label: new_cell.label,
            cell_type: new_cell.cell_type,
            source: new_cell.source,
            cargo_toml,
            shared: is_shared,
            collapsed: false,
            output_collapsed: false,
            version: 0,
            // Saved outputs are embedded at serialize time (PRD-0056),
            // never minted by the model.
            saved_output: None,
        };

        self.notebook.update(|nb_opt| {
            let Some(nb) = nb_opt else { return };
            // Shared insert semantics (after / append-on-missing / top for
            // None) — one definition with the daemon's event applier.
            notebook_ops::insert_cell(&mut nb.cells, cell.clone(), after_cell_id.as_deref());
        });

        self.cell_versions.update(|v| {
            v.insert(new_id.clone(), 0);
        });
        self.sync_from_notebook();

        Ok((
            MutationResult::CellAdded {
                cell_id: new_id,
                version: 0,
            },
            Event::CellAdded {
                cell,
                after_cell_id,
            },
        ))
    }

    fn cell_update(&self, fields: CellUpdateFields) -> Result<(MutationResult, Event), ModelError> {
        let CellUpdateFields {
            cell_id,
            source,
            cargo_toml,
            label,
            shared,
            collapsed,
            output_collapsed,
            version,
        } = fields;
        // OCC check.
        let current = self.cell_version(&cell_id);
        if version != current {
            return Err(ModelError {
                code: ErrorCode::VersionConflict,
                message: format!("Expected version {version}, actual {current}"),
            });
        }

        // Verify cell exists.
        let exists = self.notebook.with_untracked(|nb_opt| {
            nb_opt
                .as_ref()
                .is_some_and(|nb| nb.cells.iter().any(|c| c.id == cell_id))
        });
        if !exists {
            return Err(ModelError {
                code: ErrorCode::CellNotFound,
                message: format!("Cell {cell_id} not found"),
            });
        }

        let new_version = current + 1;
        let content_changed = source.is_some() || cargo_toml.is_some();
        // Shared-cell edits change EVERY cell's compilation input, not just
        // downstream: shared source is global. Capture the flag before the
        // update so both "was shared" (source edited) and "became (un)shared"
        // (flag toggled) widen the staling below.
        let was_shared = self.notebook.with_untracked(|nb_opt| {
            nb_opt
                .as_ref()
                .is_some_and(|nb| nb.cells.iter().any(|c| c.id == cell_id && c.shared))
        });
        let affects_shared =
            shared.is_some_and(|s| s != was_shared) || (was_shared && content_changed);

        // Use update_untracked for content-only changes (performance: avoids
        // triggering the notebook-level Effect that syncs layout title, etc.).
        self.notebook.update_untracked(|nb_opt| {
            let Some(nb) = nb_opt else { return };
            let Some(cell) = nb.cells.iter_mut().find(|c| c.id == cell_id) else {
                return;
            };
            // Shared per-field application — one definition with the
            // daemon's event applier.
            notebook_ops::CellPatch {
                source: source.as_ref(),
                cargo_toml: cargo_toml.as_ref(),
                label: label.as_ref(),
                shared,
                collapsed,
                output_collapsed,
            }
            .apply_to(cell, new_version);
        });

        self.cell_versions.update(|v| {
            v.insert(cell_id.clone(), new_version);
        });

        if affects_shared {
            self.mark_all_code_cells_stale();
        } else if content_changed {
            self.mark_downstream_stale(&cell_id);
        }

        // Label, the shared flag, and the collapse defaults are part of
        // CellManifest, so sync the derived list. (The cell list rebuilds
        // its rows from these manifests on remount — e.g. returning from
        // view mode — so a stale manifest resurrects old header state.)
        if label.is_some() || shared.is_some() || collapsed.is_some() || output_collapsed.is_some()
        {
            self.sync_from_notebook();
        }

        Ok((
            MutationResult::CellUpdated {
                cell_id: cell_id.clone(),
                version: new_version,
            },
            Event::CellUpdated {
                cell_id,
                source,
                cargo_toml,
                label,
                shared,
                collapsed,
                output_collapsed,
                version: new_version,
            },
        ))
    }

    fn cell_delete(
        &self,
        cell_id: String,
        version: u64,
    ) -> Result<(MutationResult, Event), ModelError> {
        // Existence check BEFORE the OCC check (cell_update does the same):
        // an unknown id reads version 0, so `CellDelete { version: 0 }` would
        // otherwise report success and broadcast a phantom CellDeleted for a
        // cell that never existed — masking a typo and terminal-izing any
        // matching `cells.run` wait as "cell_deleted".
        let exists = self.notebook.with_untracked(|nb_opt| {
            nb_opt
                .as_ref()
                .is_some_and(|nb| nb.cells.iter().any(|c| c.id == cell_id))
        });
        if !exists {
            return Err(ModelError {
                code: ErrorCode::CellNotFound,
                message: format!("Cell {cell_id} not found"),
            });
        }

        let current = self.cell_version(&cell_id);
        if version != current {
            return Err(ModelError {
                code: ErrorCode::VersionConflict,
                message: format!("Expected version {version}, actual {current}"),
            });
        }

        self.notebook.update(|nb_opt| {
            let Some(nb) = nb_opt else { return };
            notebook_ops::delete_cell(&mut nb.cells, &cell_id);
        });

        self.cell_versions.update(|v| {
            v.remove(&cell_id);
        });
        self.cell_stale.update(|s| {
            s.remove(&cell_id);
        });
        self.sync_from_notebook();

        Ok((
            MutationResult::CellDeleted {
                cell_id: cell_id.clone(),
            },
            Event::CellDeleted { cell_id },
        ))
    }

    #[allow(clippy::unnecessary_wraps)] // Consistent with other mutation methods.
    fn cell_reorder(&self, cell_ids: Vec<String>) -> Result<(MutationResult, Event), ModelError> {
        self.notebook.update(|nb_opt| {
            let Some(nb) = nb_opt else { return };
            notebook_ops::reorder_cells(&mut nb.cells, &cell_ids);
        });

        self.sync_from_notebook();

        Ok((
            MutationResult::CellReordered,
            Event::CellReordered { cell_ids },
        ))
    }

    #[allow(clippy::unnecessary_wraps)] // Consistent with other mutation methods.
    fn notebook_update_meta(
        &self,
        meta: NotebookMetaPatch,
    ) -> Result<(MutationResult, Event), ModelError> {
        self.notebook.update(|nb_opt| {
            let Some(nb) = nb_opt else { return };
            meta.apply_to(nb);
        });

        // Only the compilation inputs invalidate built cells. The presentation
        // fields are read by crawlers and the home page, never by the compiler.
        if meta.shared_cargo_toml.is_some() || meta.shared_source.is_some() {
            self.mark_all_code_cells_stale();
        }

        Ok((
            MutationResult::NotebookMetaUpdated,
            Event::NotebookMetaUpdated { meta },
        ))
    }
}

// ── Free helpers ────────────────────────────────────────────────────────────

/// Generate a new cell ID. Uses UUID v4 in the browser, a sequential
/// placeholder during SSR.
#[allow(clippy::used_underscore_binding)] // Underscore needed: only used in non-hydrate cfg.
fn generate_cell_id(_cell_count: usize) -> String {
    #[cfg(feature = "hydrate")]
    {
        uuid::Uuid::new_v4().to_string()
    }
    #[cfg(not(feature = "hydrate"))]
    {
        format!("cell_{_cell_count}")
    }
}

/// Default `Cargo.toml` template for a new code cell.
/// Dependencies (e.g. `ironpad-cell`) come from the notebook's shared Cargo.toml.
fn default_cell_cargo_toml(cell_id: &str) -> String {
    format!(
        "[package]\nname = \"{cell_id}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n"
    )
}

/// Whether a mutation from `by` is a remote (non-browser) edit that changes a
/// cell's content. This is the case that must push the new source into the
/// host's imperative Monaco editor; browser-originated edits already live there,
/// and non-content edits (reorder, label-only, delete) don't touch editor text.
fn is_remote_content_edit(by: &ClientId, mutation: &Mutation) -> bool {
    by != &ClientId::browser()
        && matches!(
            mutation,
            Mutation::CellUpdate {
                source: Some(_),
                ..
            } | Mutation::CellUpdate {
                cargo_toml: Some(_),
                ..
            }
        )
}

#[cfg(test)]
mod tests {
    use super::is_remote_content_edit;
    use ironpad_common::protocol::{ClientId, Mutation};

    fn source_update(source: Option<&str>, label: Option<&str>) -> Mutation {
        Mutation::CellUpdate {
            cell_id: "c1".into(),
            source: source.map(Into::into),
            cargo_toml: None,
            label: label.map(Into::into),
            shared: None,
            collapsed: None,
            output_collapsed: None,
            version: 0,
        }
    }

    #[test]
    fn agent_source_edit_is_remote_content_edit() {
        assert!(is_remote_content_edit(
            &ClientId::agent("a"),
            &source_update(Some("fn main() {}"), None)
        ));
    }

    #[test]
    fn agent_cargo_toml_edit_is_remote_content_edit() {
        let m = Mutation::CellUpdate {
            cell_id: "c1".into(),
            source: None,
            cargo_toml: Some(Some("[package]".into())),
            label: None,
            shared: None,
            collapsed: None,
            output_collapsed: None,
            version: 0,
        };
        assert!(is_remote_content_edit(&ClientId::agent("a"), &m));
    }

    #[test]
    fn browser_source_edit_is_not_remote() {
        // The browser's own edits already live in Monaco — must NOT refresh.
        assert!(!is_remote_content_edit(
            &ClientId::browser(),
            &source_update(Some("fn main() {}"), None)
        ));
    }

    #[test]
    fn agent_label_only_edit_is_not_content() {
        assert!(!is_remote_content_edit(
            &ClientId::agent("a"),
            &source_update(None, Some("renamed"))
        ));
    }

    #[test]
    fn agent_reorder_is_not_content() {
        assert!(!is_remote_content_edit(
            &ClientId::agent("a"),
            &Mutation::CellReorder { cell_ids: vec![] }
        ));
    }
}

// ── cell_update collapse defaults ────────────────────────────────────────────

#[cfg(test)]
mod collapse_tests {
    use super::*;
    use leptos::reactive::owner::Owner;

    fn notebook_with_one_cell() -> IronpadNotebook {
        let mut nb = IronpadNotebook::new("t");
        nb.cells.push(IronpadCell {
            id: "c1".into(),
            order: 0,
            label: "Cell".into(),
            cell_type: CellType::Code,
            source: "42".into(),
            cargo_toml: None,
            shared: false,
            collapsed: false,
            output_collapsed: false,
            version: 0,
            saved_output: None,
        });
        nb
    }

    /// The header toggles persist through `CellUpdate`: the flags land on the
    /// cell, the version bumps, and the event carries the change so remote
    /// peers (agents) stay in sync.
    #[test]
    fn cell_update_persists_collapse_defaults() {
        Owner::new().with(|| {
            let nb_signal = RwSignal::new(Some(notebook_with_one_cell()));
            let cells_signal = RwSignal::new(Vec::new());
            let model = NotebookModel::new(
                nb_signal,
                cells_signal,
                RwSignal::new(HashMap::new()),
                RwSignal::new(0),
            );
            // Populate the derived manifest list first, so the assertion
            // below proves a collapse-only update REFRESHES it rather than
            // merely populating an empty one (the editor rebuilds cell rows
            // from these manifests when returning from view mode).
            model.sync_from_notebook();

            let (result, event) = model
                .apply(
                    Mutation::CellUpdate {
                        cell_id: "c1".into(),
                        source: None,
                        cargo_toml: None,
                        label: None,
                        shared: None,
                        collapsed: Some(true),
                        output_collapsed: Some(true),
                        version: 0,
                    },
                    ClientId::browser(),
                )
                .expect("collapse update should apply");

            let nb = nb_signal.get_untracked().unwrap();
            assert!(nb.cells[0].collapsed, "collapsed flag must persist");
            assert!(nb.cells[0].output_collapsed, "output flag must persist");
            assert_eq!(nb.cells[0].version, 1);

            // The manifest list must reflect the flags too: cell rows
            // re-initialize their header toggles from it on remount.
            let manifests = cells_signal.get_untracked();
            assert!(
                manifests[0].collapsed && manifests[0].output_collapsed,
                "collapse defaults must sync into the CellManifest list"
            );
            assert!(matches!(result, MutationResult::CellUpdated { .. }));
            match event.event {
                Event::CellUpdated {
                    collapsed,
                    output_collapsed,
                    ..
                } => {
                    assert_eq!(collapsed, Some(true));
                    assert_eq!(output_collapsed, Some(true));
                }
                other => panic!("unexpected event: {other:?}"),
            }
        });
    }

    /// Deleting an unknown cell id must error (`CellNotFound`), not report a
    /// phantom success — an unknown id reads version 0, so without the
    /// existence check `CellDelete { version: 0 }` slipped through the OCC
    /// gate and broadcast a delete for a cell that never existed.
    #[test]
    fn cell_delete_unknown_id_errors_instead_of_phantom_success() {
        Owner::new().with(|| {
            let nb_signal = RwSignal::new(Some(notebook_with_one_cell()));
            let model = NotebookModel::new(
                nb_signal,
                RwSignal::new(Vec::new()),
                RwSignal::new(HashMap::new()),
                RwSignal::new(0),
            );
            model.sync_from_notebook();

            let err = model
                .apply(
                    Mutation::CellDelete {
                        cell_id: "ghost".into(),
                        version: 0,
                    },
                    ClientId::browser(),
                )
                .expect_err("deleting a nonexistent cell must error");
            assert_eq!(err.code, ErrorCode::CellNotFound);

            // The real cell still deletes.
            model
                .apply(
                    Mutation::CellDelete {
                        cell_id: "c1".into(),
                        version: 0,
                    },
                    ClientId::browser(),
                )
                .expect("deleting an existing cell must succeed");
            assert!(nb_signal.get_untracked().unwrap().cells.is_empty());
        });
    }

    /// `emit_event` (PRD-0052) feeds the same buffer the WebSocket bridge
    /// drains: the envelope is browser-attributed, and the generation bump is
    /// what wakes the bridge Effect.
    #[test]
    fn emit_event_buffers_a_browser_envelope_and_bumps_generation() {
        Owner::new().with(|| {
            let model = NotebookModel::new(
                RwSignal::new(Some(notebook_with_one_cell())),
                RwSignal::new(Vec::new()),
                RwSignal::new(HashMap::new()),
                RwSignal::new(0),
            );
            let before = model.event_generation().get_untracked();

            model.emit_event(Event::CellExecuted {
                cell_id: "c1".into(),
                display_text: Some("42".into()),
                type_tag: Some("u32".into()),
                execution_time_ms: 1.0,
                success: true,
            });

            assert_eq!(model.event_generation().get_untracked(), before + 1);
            let drained = model.drain_events();
            assert_eq!(drained.len(), 1);
            assert_eq!(drained[0].by, ClientId::browser());
            assert!(matches!(
                drained[0].event,
                Event::CellExecuted { success: true, .. }
            ));
            assert!(model.drain_events().is_empty(), "drain must consume");
        });
    }
}

// ── mark_downstream_stale ────────────────────────────────────────────────────

#[cfg(test)]
mod stale_tests {
    use super::*;
    use leptos::reactive::owner::Owner;

    fn code(id: &str) -> CellManifest {
        CellManifest {
            id: id.into(),
            order: 0,
            label: String::new(),
            cell_type: CellType::Code,
            shared: false,
            collapsed: false,
            output_collapsed: false,
        }
    }

    fn model_with_cells(cells: Vec<CellManifest>) -> NotebookModel {
        NotebookModel::new(
            RwSignal::new(None),
            RwSignal::new(cells),
            RwSignal::new(HashMap::new()),
            RwSignal::new(0),
        )
    }

    #[test]
    fn marks_from_origin_cell_onward() {
        Owner::new().with(|| {
            let model = model_with_cells(vec![code("a"), code("b"), code("c")]);
            model.mark_downstream_stale("b");
            let stale = model.cell_stale.get_untracked();
            assert_eq!(stale.get("a"), None, "upstream cell must stay fresh");
            assert_eq!(stale.get("b"), Some(&true));
            assert_eq!(stale.get("c"), Some(&true));
        });
    }

    #[test]
    fn unknown_origin_id_marks_nothing() {
        Owner::new().with(|| {
            let model = model_with_cells(vec![code("a"), code("b")]);
            model.mark_downstream_stale("missing");
            assert!(
                model.cell_stale.get_untracked().is_empty(),
                "an unknown origin id must not mark any cell stale"
            );
        });
    }
}
