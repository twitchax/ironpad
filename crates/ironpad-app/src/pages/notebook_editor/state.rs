use std::collections::HashMap;

use ironpad_common::{CellManifest, CellType, IronpadCell, IronpadNotebook};
use leptos::prelude::*;

use crate::components::monaco_editor::MonacoEditorHandle;

// ── Cell status ─────────────────────────────────────────────────────────────

/// Reactive cell execution status for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CellStatus {
    Idle,
    Queued,
    Compiling,
    #[allow(dead_code)]
    Running,
    Success,
    Error,
}

// ── Per-cell output data ────────────────────────────────────────────────────

/// Stores the output bytes and optional type tag from a cell execution.
#[derive(Clone, Default, Debug)]
pub(super) struct CellOutputData {
    pub(super) bytes: Vec<u8>,
    pub(super) type_tag: Option<String>,
}

// ── Notebook-level reactive state ───────────────────────────────────────────

/// Reactive state for the notebook editor, shared among child components.
#[derive(Clone, Copy)]
pub(super) struct NotebookState {
    /// The full notebook loaded from IndexedDB.
    pub(super) notebook: RwSignal<Option<IronpadNotebook>>,
    /// The notebook UUID string (from the URL).
    pub(super) notebook_id: RwSignal<String>,
    /// The ordered list of cells in this notebook.
    pub(super) cells: RwSignal<Vec<CellManifest>>,
    /// The currently selected/active cell ID.
    pub(super) active_cell: RwSignal<Option<String>>,
    /// Triggers a notebook refetch when incremented (retained for future use).
    #[allow(dead_code)]
    pub(super) refresh_generation: RwSignal<u64>,
    /// Cell ID that should be scrolled to and focused after creation.
    pub(super) pending_focus_cell: RwSignal<Option<String>>,
    /// Per-cell output data from the last execution, keyed by cell ID.
    /// Used to pipe cell N's output as cell N+1's input.
    pub(super) cell_outputs: RwSignal<HashMap<String, CellOutputData>>,
    /// Triggers all cells to immediately flush their content to the server.
    #[allow(dead_code)]
    pub(super) save_generation: RwSignal<u64>,
    /// Ordered queue of cell IDs for "Run All Below" sequential execution.
    /// The cell at position [0] is the one currently being executed.
    pub(super) run_all_queue: RwSignal<Vec<String>>,
    /// Notebook-level shared Cargo.toml content.
    pub(super) shared_cargo_toml: RwSignal<Option<String>>,
    /// Notebook-level shared Rust source included as `src/shared.rs` in every cell.
    pub(super) shared_source: RwSignal<Option<String>>,
    /// Tracks which cells have stale (outdated) execution results.
    pub(super) cell_stale: RwSignal<HashMap<String, bool>>,
    /// Per-cell display text (JSON of `Vec<DisplayPanel>`) from the last execution.
    /// Used by the export-to-HTML feature to include cell outputs.
    pub(super) cell_display_texts: RwSignal<HashMap<String, String>>,
    /// Per-cell source editor handles, keyed by cell ID.
    /// Used for cross-cell focus (e.g. Shift+Enter → advance to next cell).
    #[allow(dead_code)]
    pub(super) editor_handles: RwSignal<HashMap<String, MonacoEditorHandle>>,
    /// Whether the notebook is in view mode (code hidden, output-focused).
    pub(super) is_view_mode: RwSignal<bool>,
}

// ── Notebook state helpers ──────────────────────────────────────────────────

/// Syncs the `cells` signal from the current notebook state.
pub(super) fn sync_cells_from_notebook(state: &NotebookState) {
    if let Some(nb) = state.notebook.get_untracked() {
        state.cells.set(
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
    }
}

/// Persists the current notebook to IndexedDB (client-only).
#[allow(unused_variables)]
pub(super) fn persist_notebook(state: &NotebookState) {
    #[cfg(feature = "hydrate")]
    {
        if let Some(mut nb) = state.notebook.get_untracked() {
            nb.updated_at = chrono::Utc::now();
            state
                .notebook
                .update_untracked(|existing| *existing = Some(nb.clone()));
            leptos::task::spawn_local(async move {
                crate::storage::client::save_notebook(&nb).await;
            });
        }
    }
}

/// Default Cargo.toml template for a new code cell.
pub(super) fn default_cell_cargo_toml(cell_id: &str) -> String {
    format!(
        "[package]\nname = \"{cell_id}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nironpad-cell = \"0.1\"\n"
    )
}

/// Adds a new cell to the notebook in-memory, syncs signals, and persists.
pub(super) fn add_cell_to_notebook(
    state: &NotebookState,
    after_cell_id: Option<String>,
    cell_type: CellType,
) {
    let new_id = {
        #[cfg(feature = "hydrate")]
        {
            uuid::Uuid::new_v4().to_string()
        }
        #[cfg(not(feature = "hydrate"))]
        {
            format!("cell_{}", state.cells.get_untracked().len())
        }
    };

    state.notebook.update(|nb_opt| {
        let Some(nb) = nb_opt else { return };

        let label = format!("Cell {}", nb.cells.len());

        let cargo_toml = if cell_type == CellType::Code {
            Some(default_cell_cargo_toml(&new_id))
        } else {
            None
        };

        let default_source = if cell_type == CellType::Markdown {
            "# New Section\n\nAdd your notes here.".to_string()
        } else {
            "42".to_string()
        };

        let new_cell = IronpadCell {
            id: new_id.clone(),
            order: 0,
            label,
            cell_type,
            source: default_source,
            cargo_toml,
        };

        if let Some(after_id) = &after_cell_id {
            if let Some(idx) = nb.cells.iter().position(|c| c.id == *after_id) {
                nb.cells.insert(idx + 1, new_cell);
            } else {
                nb.cells.push(new_cell);
            }
        } else {
            nb.cells.insert(0, new_cell);
        }

        // Re-number orders.
        for (i, cell) in nb.cells.iter_mut().enumerate() {
            cell.order = i as u32;
        }
    });

    state.pending_focus_cell.set(Some(new_id));
    sync_cells_from_notebook(state);
    persist_notebook(state);
}
