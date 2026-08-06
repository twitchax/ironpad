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

/// Debounce (ms) for server-draft autosaves (PRD-0054): long enough to
/// coalesce a typing burst, short enough that closing the tab rarely loses
/// more than a moment of work.
#[cfg(feature = "hydrate")]
const DRAFT_SAVE_DEBOUNCE_MS: i32 = 1500;

/// Retry delay (ms) after a failed draft autosave.
#[cfg(feature = "hydrate")]
const DRAFT_SAVE_RETRY_MS: i32 = 5000;

/// Poll interval (ms) while a durable save drains in-flight autosaves.
#[cfg(feature = "hydrate")]
const DRAFT_SAVE_INFLIGHT_POLL_MS: i32 = 50;

/// Cap (ms) on that drain, so a hung autosave request can't wedge Push
/// forever — past it we proceed and rely on last-write-wins.
#[cfg(feature = "hydrate")]
const DRAFT_SAVE_INFLIGHT_WAIT_MAX_MS: i32 = 5000;

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

/// Autosave state of the server draft (PRD-0054), shown beside the Push
/// button. `Failed` self-heals: the save is rescheduled on a retry delay.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DraftSaveState {
    Synced,
    Saving,
    Failed,
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
    /// Manifests (shared + cell Cargo.toml pairs) that compiled successfully
    /// this page session — the dynamic half of the live-check warmth policy
    /// (PRD-0045): custom-deps cells only live-check once their dependency
    /// tree is known-built.
    pub(super) warm_manifests: RwSignal<std::collections::HashSet<String>>,
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
    /// `ServerDraft` mode (PRD-0054): `Some(share_id)` when this editor
    /// persists to the share's server-side draft instead of `IndexedDB`.
    pub(crate) server_draft_share: RwSignal<Option<String>>,
    /// The server draft differs from published — arms the Push button.
    pub(super) draft_dirty: RwSignal<bool>,
    /// Autosave indicator state for `ServerDraft` mode.
    pub(super) draft_save_state: RwSignal<DraftSaveState>,
    /// Monotonic epoch coalescing debounced draft saves: each schedule bumps
    /// it, and a firing task saves only if its epoch is still current.
    pub(super) draft_save_epoch: RwSignal<u64>,
    /// Count of draft-save POSTs currently in flight. The pre-Push durable
    /// save drains this before writing: the epoch guard stops tasks that
    /// have not fired yet, but a request already on the wire could land
    /// AFTER the durable write and revert the draft the promote publishes.
    pub(super) draft_save_inflight: RwSignal<u32>,
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

    /// Queue a cell for execution on behalf of a session agent (PRD-0052).
    ///
    /// Appends to the same [`run_all_queue`](Self::run_all_queue) the Run All
    /// button and cascading execution use, so unexecuted prerequisites cascade
    /// exactly as they do for a human click. Returns `Err` for a cell that
    /// does not exist or cannot run on its own (markdown, shared).
    pub(crate) fn request_cell_run(&self, cell_id: &str) -> Result<(), String> {
        use ironpad_common::types::CellType;

        let runnable = self
            .cells
            .try_get_untracked()
            .ok_or("editor is shutting down")?
            .iter()
            .any(|c| c.id == cell_id && c.cell_type == CellType::Code && !c.shared);
        if !runnable {
            return Err(format!(
                "cell {cell_id} not found or not runnable (markdown and shared cells cannot run on their own)"
            ));
        }

        self.run_all_queue.update(|q| {
            if !q.iter().any(|id| id == cell_id) {
                q.push(cell_id.to_string());
            }
        });
        Ok(())
    }

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

    /// Halt a run-all cascade at `failed_cell_id`: in reactive mode, mark
    /// the remaining queued cells blocked-by the failure, then clear the
    /// queue. ONE definition for the three error paths (execution, compile,
    /// transport) that each used to hand-roll it — the daemon's
    /// `prerequisite_failed` semantics lean on this behavior, so the copies
    /// drifting apart would desync agents from the UI.
    pub(super) fn abort_run_cascade(&self, failed_cell_id: &str) {
        let queue = self.run_all_queue.get_untracked();
        if self.reactive_mode.get_untracked() && queue.len() > 1 {
            self.cell_blocked_by.update(|blocked| {
                for queued_id in &queue[1..] {
                    blocked.insert(queued_id.clone(), failed_cell_id.to_string());
                }
            });
        }
        self.run_all_queue.set(vec![]);
    }
}

// ── Notebook state helpers ──────────────────────────────────────────────────

/// Persists the current notebook, fire-and-forget (client-only). Local
/// notebooks write `IndexedDB`; `ServerDraft` notebooks (PRD-0054) schedule a
/// debounced draft autosave instead. Callers that need to know the write
/// landed use [`persist_notebook_durable`].
#[allow(unused_variables)]
pub(crate) fn persist_notebook(state: &NotebookState) {
    #[cfg(feature = "hydrate")]
    {
        if state.server_draft_share.get_untracked().is_some() {
            let _ = state.draft_dirty.try_set(true);
            schedule_draft_save(state, DRAFT_SAVE_DEBOUNCE_MS);
            return;
        }
        let state = *state;
        leptos::task::spawn_local(async move {
            persist_notebook_durable(&state).await;
        });
    }
}

/// Awaitable persist: resolves after the write completes, returning whether
/// it landed. In `ServerDraft` mode this saves the draft IMMEDIATELY (no
/// debounce): durable callers (the pre-Push flush) need the server to hold
/// current content, and Push aborts when this returns `false` — promoting a
/// draft the server never received would publish stale content.
#[cfg(feature = "hydrate")]
pub(super) async fn persist_notebook_durable(state: &NotebookState) -> bool {
    let Some(mut nb) = state.notebook.try_get_untracked().flatten() else {
        return false;
    };
    nb.updated_at = chrono::Utc::now();
    state
        .notebook
        .update_untracked(|existing| *existing = Some(nb.clone()));
    if state.server_draft_share.get_untracked().is_some() {
        let _ = state.draft_dirty.try_set(true);
        // Drain any autosave POST already on the wire so ours lands last —
        // an earlier request arriving after this write would revert the
        // draft the promote then publishes. Bounded: past the cap we
        // proceed and rely on last-write-wins.
        let mut waited_ms = 0;
        while state.draft_save_inflight.try_get_untracked().unwrap_or(0) > 0
            && waited_ms < DRAFT_SAVE_INFLIGHT_WAIT_MAX_MS
        {
            super::yield_for_cell_flush(DRAFT_SAVE_INFLIGHT_POLL_MS).await;
            waited_ms += DRAFT_SAVE_INFLIGHT_POLL_MS;
        }
        // Supersede any pending debounce so it can't double-write after us.
        state.draft_save_epoch.update_untracked(|e| *e += 1);
        save_draft_now(state, true).await
    } else if let Err(e) = crate::storage::client::save_notebook(&nb).await {
        leptos::logging::error!("failed to persist notebook to IndexedDB: {e:?}");
        false
    } else {
        true
    }
}

/// Schedule a coalesced draft save `after_ms` from now. Every call bumps the
/// epoch; only the task holding the newest epoch actually writes, so a typing
/// burst produces one server round trip.
#[cfg(feature = "hydrate")]
pub(super) fn schedule_draft_save(state: &NotebookState, after_ms: i32) {
    let state = *state;
    let epoch = state
        .draft_save_epoch
        .try_get_untracked()
        .unwrap_or(0)
        .wrapping_add(1);
    state.draft_save_epoch.update_untracked(|e| *e = epoch);
    leptos::task::spawn_local(async move {
        super::yield_for_cell_flush(after_ms).await;
        if state.draft_save_epoch.try_get_untracked() != Some(epoch) {
            return; // superseded by a newer edit or an immediate save
        }
        // Lean: the debounce path never embeds outputs (PRD-0056) — only
        // the pre-Push durable save pays that weight.
        save_draft_now(&state, false).await;
    });
}

/// Serialize the current notebook and write the server draft, updating the
/// indicator. Returns whether the write landed; a failure schedules a retry
/// (epoch-guarded, so an intervening edit's own save wins).
///
/// `enrich_outputs` embeds the author's last-run panels (PRD-0056) into the
/// outgoing JSON. Only the pre-Push durable save sets it: the promote then
/// publishes those outputs, while the 1.5s debounce autosaves stay lean.
#[cfg(feature = "hydrate")]
pub(super) async fn save_draft_now(state: &NotebookState, enrich_outputs: bool) -> bool {
    let Some(share_id) = state.server_draft_share.try_get_untracked().flatten() else {
        return false;
    };
    let Some(mut nb) = state.notebook.try_get_untracked().flatten() else {
        return false;
    };
    if enrich_outputs {
        if let Some(texts) = state.cell_display_texts.try_get_untracked() {
            nb.embed_saved_outputs(&texts, ironpad_common::types::SAVED_OUTPUT_BUDGET_BYTES);
        }
    }
    let json = match serde_json::to_string(&nb) {
        Ok(json) => json,
        Err(e) => {
            leptos::logging::error!("draft serialize failed: {e}");
            return false;
        }
    };
    let _ = state.draft_save_state.try_set(DraftSaveState::Saving);
    let _ = state.draft_save_inflight.try_update(|n| *n += 1);
    let result = crate::server_fns::save_mutable_draft(share_id, json).await;
    let _ = state
        .draft_save_inflight
        .try_update(|n| *n = n.saturating_sub(1));
    match result {
        Ok(()) => {
            let _ = state.draft_save_state.try_set(DraftSaveState::Synced);
            true
        }
        Err(e) => {
            leptos::logging::warn!("draft autosave failed (will retry): {e}");
            if state
                .draft_save_state
                .try_set(DraftSaveState::Failed)
                .is_none()
            {
                schedule_draft_save(state, DRAFT_SAVE_RETRY_MS);
            }
            false
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
