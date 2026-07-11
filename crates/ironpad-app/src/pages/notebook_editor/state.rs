use std::collections::HashMap;

use ironpad_common::{CellManifest, IronpadNotebook};
use leptos::prelude::*;

use crate::components::monaco_editor::MonacoEditorHandle;
// Re-exported so the rest of `notebook_editor` keeps importing it as
// `super::state::CellOutputData`; the single definition lives in `output_render`.
pub(super) use crate::components::output_render::CellOutputData;

// ── Constants ───────────────────────────────────────────────────────────────

/// Debounce delay (ms) before reactive re-evaluation after a cell edit.
const REACTIVE_DEBOUNCE_MS: i32 = 500;

// ── Cell status ─────────────────────────────────────────────────────────────

/// Reactive cell execution status for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CellStatus {
    Idle,
    Queued,
    Compiling,
    Running,
    Success,
    Error,
    /// Downstream cell blocked by an upstream error during reactive/run-all execution.
    Blocked,
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
    /// Cell ID that should be scrolled to and focused after creation.
    pub(super) pending_focus_cell: RwSignal<Option<String>>,
    /// Per-cell output data from the last execution, keyed by cell ID.
    /// Used to pipe cell N's output as cell N+1's input.
    pub(super) cell_outputs: RwSignal<HashMap<String, CellOutputData>>,
    /// Triggers all cells to immediately flush their content to the server.
    // Used in cell_item.rs under #[cfg(feature = "hydrate")]; appears dead during SSR.
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
    // Used in cell_item.rs under #[cfg(feature = "hydrate")]; appears dead during SSR.
    #[allow(dead_code)]
    pub(super) editor_handles: RwSignal<HashMap<String, MonacoEditorHandle>>,
    /// Whether the notebook is in view mode (code hidden, output-focused).
    pub(super) is_view_mode: RwSignal<bool>,
    /// When `true`, the next compilation(s) bypass the server-side WASM cache.
    pub(super) force_recompile: RwSignal<bool>,
    /// When `true`, editing a cell auto-triggers downstream re-execution after debounce.
    pub(super) reactive_mode: RwSignal<bool>,
    /// Mirror of the notebook's `expand_code` flag (view-only pages render
    /// code cells expanded). Toggled from the gear menu and persisted.
    pub(super) expand_code: RwSignal<bool>,
    /// Handle for the pending reactive debounce timer. `None` when no timer is active.
    pub(super) reactive_timer: RwSignal<Option<i32>>,
    /// Maps cell ID → the ID of the upstream cell whose error blocked it (cascade halt).
    pub(super) cell_blocked_by: RwSignal<HashMap<String, String>>,
    /// Bumped when a remote (agent) edit changes a cell's content, so each cell's
    /// editor can pull the new source into Monaco. Browser-originated edits do
    /// NOT bump this — Monaco already holds the host's own text.
    // Used in cell_item.rs under #[cfg(feature = "hydrate")]; appears dead during SSR.
    #[allow(dead_code)]
    pub(super) external_content_generation: RwSignal<u64>,
    /// Reusable JS callback backing the reactive debounce timer. Built once by
    /// [`NotebookState::init_reactive_timer`] at editor setup and held in the
    /// reactive arena so it drops on unmount — instead of leaking a fresh
    /// `Closure::once` on every edit (the previous `forget()` behaviour).
    #[cfg(feature = "hydrate")]
    pub(super) reactive_timer_fn: StoredValue<Option<js_sys::Function>, LocalStorage>,
}

// ── Reactive execution scheduling ───────────────────────────────────────────

impl NotebookState {
    /// Build the reusable reactive-debounce callback once and cache it in
    /// [`Self::reactive_timer_fn`]. Call at editor setup so the closure is
    /// created under the component's owner and dropped on unmount; it reads the
    /// *current* notebook state via `get_untracked()` at fire time, so a single
    /// closure serves every debounce and nothing is leaked per edit.
    #[cfg(feature = "hydrate")]
    pub(super) fn init_reactive_timer(&self) {
        use ironpad_common::types::CellType;
        use wasm_bindgen::JsCast;

        if self.reactive_timer_fn.get_value().is_some() {
            return;
        }

        let cells = self.cells;
        let cell_stale = self.cell_stale;
        let run_all_queue = self.run_all_queue;
        let reactive_timer = self.reactive_timer;

        let closure = wasm_bindgen::closure::Closure::<dyn Fn()>::new(move || {
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
                    c.cell_type == CellType::Code
                        && !c.shared
                        && stale_map.get(&c.id).copied().unwrap_or(false)
                })
                .map(|c| c.id.clone())
                .collect();

            if !stale_ids.is_empty() {
                run_all_queue.set(stale_ids);
            }
        });
        let func: js_sys::Function = closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
        // Hold the closure in the reactive arena (dropped on disposal) instead
        // of leaking it with forget().
        StoredValue::new_local(closure);
        self.reactive_timer_fn.set_value(Some(func));
    }

    /// No-op on the server side.
    #[cfg(not(feature = "hydrate"))]
    pub(super) fn init_reactive_timer(&self) {}

    /// Schedule reactive re-execution of stale downstream cells after a 500 ms
    /// debounce window. Cancels any pending timer before starting a new one, and
    /// reuses the cached callback built by [`Self::init_reactive_timer`].
    #[cfg(feature = "hydrate")]
    pub(super) fn schedule_reactive_execution(&self) {
        let Some(window) = web_sys::window() else {
            return;
        };

        // Cancel existing timer if any.
        if let Some(handle) = self.reactive_timer.get_untracked() {
            window.clear_timeout_with_handle(handle);
        }

        if let Some(func) = self.reactive_timer_fn.get_value() {
            if let Ok(handle) = window
                .set_timeout_with_callback_and_timeout_and_arguments_0(&func, REACTIVE_DEBOUNCE_MS)
            {
                self.reactive_timer.set(Some(handle));
            }
        }
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
                if let Err(e) = crate::storage::client::save_notebook(&nb).await {
                    leptos::logging::error!("failed to persist notebook to IndexedDB: {e:?}");
                }
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
