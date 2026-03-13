use std::collections::HashMap;

use ironpad_common::{CellManifest, IronpadNotebook};
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
pub(crate) struct NotebookState {
    /// The full notebook loaded from IndexedDB.
    pub(super) notebook: RwSignal<Option<IronpadNotebook>>,
    /// The notebook UUID string (from the URL).
    pub(crate) notebook_id: RwSignal<String>,
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
    /// When `true`, the next compilation(s) bypass the server-side WASM cache.
    pub(super) force_recompile: RwSignal<bool>,
}

// ── Notebook state helpers ──────────────────────────────────────────────────

/// Persists the current notebook to IndexedDB (client-only).
#[allow(unused_variables)]
pub(crate) fn persist_notebook(state: &NotebookState) {
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
