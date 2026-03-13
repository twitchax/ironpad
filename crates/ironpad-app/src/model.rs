//! Unified notebook model.
//!
//! All mutations to notebook state — whether from the browser UI or a remote
//! agent — go through [`NotebookModel::apply`]. This guarantees a single
//! codepath for state changes, version tracking, stale marking, and event
//! generation regardless of the mutation source.

use std::collections::HashMap;

use ironpad_common::protocol::*;
use ironpad_common::{CellManifest, CellType, IronpadCell, IronpadNotebook};
use leptos::prelude::*;

// ── Error type ──────────────────────────────────────────────────────────────

/// Error returned by model operations.
#[derive(Clone, Debug)]
// Fields read by the future WebSocket bridge.
#[allow(dead_code)]
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
}

impl NotebookModel {
    /// Create a model backed by the given reactive signals.
    pub(crate) fn new(
        notebook: RwSignal<Option<IronpadNotebook>>,
        cells: RwSignal<Vec<CellManifest>>,
        cell_stale: RwSignal<HashMap<String, bool>>,
    ) -> Self {
        Self {
            notebook,
            cells,
            cell_stale,
            cell_versions: RwSignal::new(HashMap::new()),
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
                version,
            } => self.cell_update(cell_id, source, cargo_toml, label, version)?,
            Mutation::CellDelete { cell_id, version } => self.cell_delete(cell_id, version)?,
            Mutation::CellReorder { cell_ids } => self.cell_reorder(cell_ids)?,
            Mutation::NotebookUpdateMeta {
                title,
                shared_cargo_toml,
                shared_source,
            } => self.notebook_update_meta(title, shared_cargo_toml, shared_source)?,
            Mutation::CellCompile { .. } | Mutation::CellExecute { .. } => {
                return Err(ModelError {
                    code: ErrorCode::InvalidMessage,
                    message: "Compile/Execute are handled by the execution layer".into(),
                });
            }
        };
        Ok((result, EventEnvelope { by, event }))
    }

    /// Read-only queries against the notebook model.
    ///
    /// Used by the future WebSocket bridge to serve agent queries.
    #[allow(dead_code)]
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
            Query::SessionStatus => Err(ModelError {
                code: ErrorCode::InvalidMessage,
                message: "SessionStatus is handled by the session layer".into(),
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
        let my_idx = cells.iter().position(|c| c.id == from_cell_id).unwrap_or(0);
        self.cell_stale.update(|stale| {
            for cell in &cells[my_idx..] {
                if cell.cell_type == CellType::Code {
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
                if cell.cell_type == CellType::Code {
                    stale.insert(cell.id.clone(), true);
                }
            }
        });
    }

    // ── Mutation implementations ────────────────────────────────────────

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

        let cell = IronpadCell {
            id: new_id.clone(),
            order: 0,
            label: new_cell.label,
            cell_type: new_cell.cell_type,
            source: new_cell.source,
            cargo_toml,
            version: 0,
        };

        self.notebook.update(|nb_opt| {
            let Some(nb) = nb_opt else { return };

            if let Some(ref after_id) = after_cell_id {
                if let Some(idx) = nb.cells.iter().position(|c| c.id == *after_id) {
                    nb.cells.insert(idx + 1, cell.clone());
                } else {
                    nb.cells.push(cell.clone());
                }
            } else {
                // No after_cell_id → insert at beginning (matches existing UI behavior:
                // the top "Add Cell" button passes None).
                nb.cells.insert(0, cell.clone());
            }

            renumber(&mut nb.cells);
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

    fn cell_update(
        &self,
        cell_id: String,
        source: Option<String>,
        cargo_toml: Option<Option<String>>,
        label: Option<String>,
        version: u64,
    ) -> Result<(MutationResult, Event), ModelError> {
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

        // Use update_untracked for content-only changes (performance: avoids
        // triggering the notebook-level Effect that syncs layout title, etc.).
        self.notebook.update_untracked(|nb_opt| {
            let Some(nb) = nb_opt else { return };
            let Some(cell) = nb.cells.iter_mut().find(|c| c.id == cell_id) else {
                return;
            };
            if let Some(ref src) = source {
                cell.source = src.clone();
            }
            if let Some(ref ct) = cargo_toml {
                cell.cargo_toml = ct.clone();
            }
            if let Some(ref lbl) = label {
                cell.label = lbl.clone();
            }
            cell.version = new_version;
        });

        self.cell_versions.update(|v| {
            v.insert(cell_id.clone(), new_version);
        });

        if content_changed {
            self.mark_downstream_stale(&cell_id);
        }

        // Label is part of CellManifest, so sync the derived list.
        if label.is_some() {
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
                version: new_version,
            },
        ))
    }

    fn cell_delete(
        &self,
        cell_id: String,
        version: u64,
    ) -> Result<(MutationResult, Event), ModelError> {
        let current = self.cell_version(&cell_id);
        if version != current {
            return Err(ModelError {
                code: ErrorCode::VersionConflict,
                message: format!("Expected version {version}, actual {current}"),
            });
        }

        self.notebook.update(|nb_opt| {
            let Some(nb) = nb_opt else { return };
            nb.cells.retain(|c| c.id != cell_id);
            renumber(&mut nb.cells);
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

    fn cell_reorder(&self, cell_ids: Vec<String>) -> Result<(MutationResult, Event), ModelError> {
        self.notebook.update(|nb_opt| {
            let Some(nb) = nb_opt else { return };
            let mut reordered = Vec::with_capacity(cell_ids.len());
            for id in &cell_ids {
                if let Some(pos) = nb.cells.iter().position(|c| &c.id == id) {
                    reordered.push(nb.cells.remove(pos));
                }
            }
            // Append any cells not in cell_ids (shouldn't happen, but safe).
            reordered.append(&mut nb.cells);
            for (i, c) in reordered.iter_mut().enumerate() {
                c.order = i as u32;
            }
            nb.cells = reordered;
        });

        self.sync_from_notebook();

        Ok((
            MutationResult::CellReordered,
            Event::CellReordered { cell_ids },
        ))
    }

    fn notebook_update_meta(
        &self,
        title: Option<String>,
        shared_cargo_toml: Option<Option<String>>,
        shared_source: Option<Option<String>>,
    ) -> Result<(MutationResult, Event), ModelError> {
        self.notebook.update(|nb_opt| {
            let Some(nb) = nb_opt else { return };
            if let Some(ref t) = title {
                nb.title = t.clone();
            }
            if let Some(ref sct) = shared_cargo_toml {
                nb.shared_cargo_toml = sct.clone();
            }
            if let Some(ref ss) = shared_source {
                nb.shared_source = ss.clone();
            }
        });

        if shared_cargo_toml.is_some() || shared_source.is_some() {
            self.mark_all_code_cells_stale();
        }

        Ok((
            MutationResult::NotebookMetaUpdated,
            Event::NotebookMetaUpdated {
                title,
                shared_cargo_toml,
                shared_source,
            },
        ))
    }
}

// ── Free helpers ────────────────────────────────────────────────────────────

/// Generate a new cell ID. Uses UUID v4 in the browser, a sequential
/// placeholder during SSR.
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

/// Renumber all cells' `order` fields to match their position in the vec.
fn renumber(cells: &mut [IronpadCell]) {
    for (i, cell) in cells.iter_mut().enumerate() {
        cell.order = i as u32;
    }
}

/// Default `Cargo.toml` template for a new code cell.
fn default_cell_cargo_toml(cell_id: &str) -> String {
    format!(
        "[package]\nname = \"{cell_id}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nironpad-cell = \"0.1\"\n"
    )
}
