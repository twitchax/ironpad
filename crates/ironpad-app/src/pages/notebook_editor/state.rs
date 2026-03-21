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
    /// Downstream cell blocked by an upstream error during reactive/run-all execution.
    Blocked,
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
    /// The full notebook loaded from `IndexedDB`.
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
    /// When `true`, editing a cell auto-triggers downstream re-execution after debounce.
    pub(super) reactive_mode: RwSignal<bool>,
    /// Handle for the pending reactive debounce timer. `None` when no timer is active.
    pub(super) reactive_timer: RwSignal<Option<i32>>,
    /// Maps cell ID → the ID of the upstream cell whose error blocked it (cascade halt).
    pub(super) cell_blocked_by: RwSignal<HashMap<String, String>>,
}

// ── Reactive execution scheduling ───────────────────────────────────────────

impl NotebookState {
    /// Schedule reactive re-execution of stale downstream cells after a 500 ms
    /// debounce window.  Cancels any pending timer before starting a new one.
    #[cfg(feature = "hydrate")]
    pub(super) fn schedule_reactive_execution(&self) {
        use ironpad_common::types::CellType;
        use wasm_bindgen::JsCast;

        // Cancel existing timer if any.
        if let Some(handle) = self.reactive_timer.get_untracked() {
            if let Some(window) = web_sys::window() {
                window.clear_timeout_with_handle(handle);
            }
        }

        let cells = self.cells;
        let cell_stale = self.cell_stale;
        let run_all_queue = self.run_all_queue;
        let reactive_timer = self.reactive_timer;

        let Some(window) = web_sys::window() else {
            return;
        };

        let cb = wasm_bindgen::closure::Closure::once(move || {
            reactive_timer.set(None);

            // Don't enqueue if there's already a run in progress.
            if !run_all_queue.get_untracked().is_empty() {
                return;
            }

            // Collect stale Code cell IDs in notebook order.
            let all_cells = cells.get_untracked();
            let stale_map = cell_stale.get_untracked();
            let stale_ids: Vec<String> = all_cells
                .iter()
                .filter(|c| {
                    c.cell_type == CellType::Code && stale_map.get(&c.id).copied().unwrap_or(false)
                })
                .map(|c| c.id.clone())
                .collect();

            if !stale_ids.is_empty() {
                run_all_queue.set(stale_ids);
            }
        });

        if let Ok(handle) = window
            .set_timeout_with_callback_and_timeout_and_arguments_0(cb.as_ref().unchecked_ref(), 500)
        {
            reactive_timer.set(Some(handle));
        }

        cb.forget(); // Leak the one-shot closure.
    }

    /// No-op on the server side.
    #[cfg(not(feature = "hydrate"))]
    pub(super) fn schedule_reactive_execution(&self) {}
}

// ── Notebook state helpers ──────────────────────────────────────────────────

/// Persists the current notebook to `IndexedDB` (client-only).
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

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_status_blocked_variant_exists_and_eq() {
        let status = CellStatus::Blocked;
        assert_eq!(status, CellStatus::Blocked);
        assert_ne!(status, CellStatus::Error);
        assert_ne!(status, CellStatus::Idle);
    }

    #[test]
    fn cell_status_is_copy() {
        let original = CellStatus::Blocked;
        let copied = original;
        assert_eq!(original, copied);
    }

    #[test]
    fn cell_status_debug_format() {
        let dbg = format!("{:?}", CellStatus::Blocked);
        assert_eq!(dbg, "Blocked");
    }

    #[test]
    fn cell_status_all_variants_are_distinct() {
        let variants = [
            CellStatus::Idle,
            CellStatus::Queued,
            CellStatus::Compiling,
            CellStatus::Running,
            CellStatus::Success,
            CellStatus::Error,
            CellStatus::Blocked,
        ];
        for (i, a) in variants.iter().enumerate() {
            for (j, b) in variants.iter().enumerate() {
                if i == j {
                    assert_eq!(a, b);
                } else {
                    assert_ne!(a, b, "{a:?} should differ from {b:?}");
                }
            }
        }
    }

    #[test]
    fn cell_output_data_default_is_empty() {
        let data = CellOutputData::default();
        assert!(data.bytes.is_empty());
        assert!(data.type_tag.is_none());
    }
}
