use std::collections::HashMap;

use ironpad_common::{CellManifest, IronpadNotebook};
use leptos::prelude::*;

use crate::components::monaco_editor::MonacoEditorHandle;
use crate::components::run_flow::RunQueue;
use crate::components::toaster::{ToastIntent, Toaster};
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

/// Poll interval (ms) while [`drain_inflight_drafts`] waits.
#[cfg(feature = "hydrate")]
const DRAFT_SAVE_INFLIGHT_POLL_MS: i32 = 50;

/// Cap (ms) on that drain, so a hung autosave request can't wedge a save
/// or a discard forever — past it we proceed and rely on last-write-wins.
#[cfg(feature = "hydrate")]
const DRAFT_SAVE_INFLIGHT_WAIT_MAX_MS: i32 = 5000;

/// Delay (ms) after bumping `save_generation` before the caller re-reads the
/// notebook, giving the per-cell flush effects time to run. See
/// [`NotebookState::flush_cells`], its only user.
#[cfg(feature = "hydrate")]
const CELL_FLUSH_YIELD_MS: i32 = 120;

/// Lifetime (s) of the one toast a refused draft save gets. Long: it is shown
/// once, it carries the limit and the remedy, and the alternative to reading
/// it is losing work.
#[cfg(feature = "hydrate")]
const REFUSAL_TOAST_SECS: u32 = 15;

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
///
/// `Refused` is the server saying no, and that is the whole reason it exists
/// (PRD-0064): a write declined for a storage cap, a size cap or ownership
/// will be declined identically forever, so retrying it is a loop that cannot
/// terminate under an indicator promising that it will. The state stops the
/// loop (except for [`DraftRefusal::SignedOut`]) and says why.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DraftSaveState {
    Synced,
    Saving,
    Failed,
    /// The server refused the write, for the reason carried.
    Refused(DraftRefusal),
}

/// Why the server refused a draft save. Decides the copy, and whether a
/// retry can ever help.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DraftRefusal {
    /// The instance's or the account's storage cap.
    Quota,
    /// The notebook is over the upload cap, by the server's own check or by
    /// the request-body limit in front of it.
    TooLarge,
    /// The signed-in account does not own the notebook: it was deleted, or
    /// moved, from somewhere else.
    NotOwner,
    /// No session: signed out in another tab, revoked, or lapsed.
    SignedOut,
}

impl DraftRefusal {
    /// Whether to keep retrying anyway. Only for a lost session: signing in
    /// again from another tab fixes it without a reload, and the server
    /// answers "sign in" too when it could not read the session at all.
    pub(super) fn retries(self) -> bool {
        matches!(self, Self::SignedOut)
    }

    /// The indicator beside the publish button. Short, and with no promise
    /// of a retry that is not coming: the toast carries the remedy.
    pub(super) fn indicator(self) -> &'static str {
        match self {
            Self::Quota => "Draft not saved: storage full",
            Self::TooLarge => "Draft not saved: notebook too large",
            Self::NotOwner => "Draft not saved: not in your account",
            Self::SignedOut => "Draft not saved: signed out",
        }
    }

    /// The "Push Failed" body when the pre-push save was refused. Push has
    /// no server message to quote, so the kind carries the explanation.
    pub(super) fn push_failed_body(self) -> &'static str {
        match self {
            Self::Quota => {
                "Your account is out of storage, so the draft never reached the \
                 server and nothing was published. Delete a notebook from your \
                 account to free space, then push again."
            }
            Self::TooLarge => {
                "This notebook is over the 4 MiB size limit, so the draft never \
                 reached the server and nothing was published. Remove cells or \
                 large outputs, then push again."
            }
            Self::NotOwner => {
                "This notebook is no longer in your account (it was deleted or \
                 moved), so nothing was published. Use Download .ironpad to keep \
                 this text."
            }
            Self::SignedOut => {
                "You are signed out, so the draft never reached the server and \
                 nothing was published. Sign in again from a new tab without \
                 closing this one, then push again."
            }
        }
    }
}

/// What the toolbar's one editorial button offers (PRD-0064).
///
/// Derived from two independent flags rather than one: an account notebook
/// that has never been published is permanently `dirty` by construction (its
/// content IS the draft), so `dirty` alone cannot tell a first publish from a
/// republish, and the button must stay armed either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PublishButton {
    /// No published copy yet: publishing makes the link resolve at all.
    Publish,
    /// Published, with edits the readers have not seen.
    Push,
    /// Published and clean. The resting, disabled state.
    Published,
}

impl PublishButton {
    /// `published`: a published copy exists. `dirty`: the draft differs.
    pub(super) fn from_flags(published: bool, dirty: bool) -> Self {
        match (published, dirty) {
            (false, _) => Self::Publish,
            (true, true) => Self::Push,
            (true, false) => Self::Published,
        }
    }

    /// Whether clicking does anything.
    pub(super) fn armed(self) -> bool {
        !matches!(self, Self::Published)
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Publish => "Publish",
            Self::Push => "Push",
            Self::Published => "Published",
        }
    }

    pub(super) fn title(self) -> &'static str {
        match self {
            Self::Publish => "Publish this notebook: its link starts working for readers",
            Self::Push => "Publish your draft: readers see it after this",
            Self::Published => "The published copy matches your draft",
        }
    }
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
    /// Each executed cell's output type tag ([`type_tags_of`] over
    /// `cell_outputs`). A memo so readers that only need tags (the per-cell
    /// autocomplete context) re-run when a TAG changes, not on every
    /// byte-only write: a widget drag rewrites its cell's output bytes on
    /// each input event.
    pub(super) type_tags: Memo<HashMap<String, String>>,
    /// Triggers all cells to immediately flush their content to the server.
    // Used in cell_item.rs under #[cfg(feature = "hydrate")]; appears dead during SSR.
    #[allow(dead_code)]
    pub(super) save_generation: RwSignal<u64>,
    /// Ordered queue of cell IDs for sequential execution (Run All, Run All
    /// Below, cascades, agent runs). The front is the cell currently being
    /// executed; [`RunQueue`] owns the merge-never-replace policy.
    pub(super) run_all_queue: RunQueue,
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
    /// Whether the share has a published copy (PRD-0064). `false` for an
    /// account notebook that has never been published: it is server-stored
    /// and editable, but invisible to everyone else, so the button says
    /// Publish and the access controls stay hidden.
    pub(super) share_published: RwSignal<bool>,
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
    /// The model holds edits that no write has captured yet. Read by the
    /// editor's teardown write ([`persist_on_teardown`]): navigating away
    /// disposes the debounce timers and the scheduled draft save, so without
    /// it the last edits never leave the page. Gating that write on this
    /// flag is what keeps a plain open-and-close from bumping `updated_at`,
    /// minting a history snapshot, or creating a draft on a clean published
    /// notebook.
    ///
    /// Cleared when a write TAKES its copy of the model, not when it lands:
    /// an edit flushed in while that write is on the wire marks it again,
    /// which a clear-on-success would erase. A failed write marks it again.
    pub(super) persist_pending: RwSignal<bool>,
    /// Stop persisting, for good. Set by the flows that remove or replace
    /// what this page would write to (Delete, Save to Account, Share
    /// Mutable, Discard Draft, History Restore): a save landing after them
    /// would resurrect a deleted notebook or overwrite the restored or
    /// discarded state. The ONE halt flag; every persist path checks it.
    pub(super) persist_halted: RwSignal<bool>,
    /// A Local autosave failed and the user was told; the next write that
    /// lands clears it, so a later failure speaks again.
    pub(super) local_save_failed: RwSignal<bool>,
    /// App-level toast handle, resolved once at mount.
    ///
    /// Carried here rather than threaded through `persist_notebook`: the
    /// autosave chain starts at a dozen call sites that hold only a
    /// `NotebookState`, and `leptos::task::spawn_local` does NOT propagate
    /// the reactive owner, so a continuation cannot resolve the toaster from
    /// context itself. `Toaster` is `Copy` and app-root-owned, which is what
    /// makes dispatching from a post-await continuation disposal-safe.
    pub(super) toaster: Toaster,
}

/// Each executed cell's output type tag, keyed by cell id; outputs with no
/// tag are skipped. The projection behind [`NotebookState::type_tags`].
pub(super) fn type_tags_of(outputs: &HashMap<String, CellOutputData>) -> HashMap<String, String> {
    outputs
        .iter()
        .filter_map(|(id, data)| data.type_tag.clone().map(|tag| (id.clone(), tag)))
        .collect()
}

// ── Cell flush (PRD-0032 T-007) ─────────────────────────────────────────────
//
// Share, Export HTML, and Download .ironpad all serialize the notebook from
// `notebook`. Each cell's live editor content only reaches that signal on a
// 1s debounce or when `save_generation` bumps (see the flush effect in
// `cell_item.rs`). Bumping the generation alone isn't enough, though: Leptos
// effects run queued, not synchronously on `signal.update()`, so callers
// must yield before re-reading the notebook.

/// Awaits a `setTimeout` so queued Leptos effects — in particular each
/// cell's notebook-level save-flush effect (`cell_item.rs`) — get a chance
/// to run before the caller re-reads `notebook`. The delay is a pragmatic
/// yield for the effect queue, not a correctness guarantee. Private to
/// [`NotebookState::flush_cells`]: an unrelated timer wants
/// `run_flow::sleep_ms`, not a "cell flush" that flushes nothing.
#[cfg(feature = "hydrate")]
async fn yield_for_cell_flush() {
    crate::components::run_flow::sleep_ms(CELL_FLUSH_YIELD_MS).await;
}

impl NotebookState {
    /// Flush every cell's unsaved editor content into the model (PRD-0032
    /// T-007): bump the flush generation, then yield so each `CellItem`'s
    /// flush effect runs before the caller reads or persists the notebook.
    /// `None` means the page was disposed mid-flush (navigation) — callers
    /// just stop.
    ///
    /// The one flush for every flow that must not lose the last debounce
    /// window of typing: the serialize flows, Restore's pre-restore
    /// snapshot, Preview, and Ctrl+S.
    pub(super) async fn flush_cells(&self) -> Option<()> {
        self.save_generation.try_update(|g| *g += 1)?;
        #[cfg(feature = "hydrate")]
        yield_for_cell_flush().await;
        // Checked again AFTER the yield, which is where a navigation lands:
        // a caller told `Some` goes on to read the page's signals, and a
        // plain read of a disposed signal panics the whole wasm app.
        self.save_generation.try_get_untracked()?;
        Some(())
    }

    /// [`Self::flush_cells`], then read the flushed notebook out of the
    /// model. `None` when the page was disposed mid-flush.
    pub(super) async fn flush_and_read(&self) -> Option<IronpadNotebook> {
        self.flush_cells().await?;
        self.notebook.try_get_untracked().flatten()
    }

    /// Record that the model now holds edits no write has captured
    /// ([`Self::persist_pending`]). Untracked: nothing renders from it.
    #[cfg(feature = "hydrate")]
    pub(super) fn mark_persist_pending(&self) {
        let _ = self.persist_pending.try_update_untracked(|p| *p = true);
    }

    /// A write is taking its copy of the model now, so nothing is pending
    /// until the next edit ([`Self::persist_pending`]).
    #[cfg(feature = "hydrate")]
    pub(super) fn take_persist_pending(&self) {
        let _ = self.persist_pending.try_update_untracked(|p| *p = false);
    }

    /// Whether a flow has halted persistence ([`Self::persist_halted`]).
    #[cfg(feature = "hydrate")]
    pub(super) fn persistence_halted(&self) -> bool {
        self.persist_halted.try_get_untracked() == Some(true)
    }

    /// Halt every further persist from this page ([`Self::persist_halted`]).
    #[cfg(feature = "hydrate")]
    pub(super) fn halt_persistence(&self) {
        let _ = self.persist_halted.try_update_untracked(|h| *h = true);
    }

    /// Undo [`Self::halt_persistence`] for a flow that failed and leaves the
    /// page editing.
    #[cfg(feature = "hydrate")]
    pub(super) fn resume_persistence(&self) {
        let _ = self.persist_halted.try_update_untracked(|h| *h = false);
    }

    /// Whether this page's signals are gone (it navigated away).
    #[cfg(feature = "hydrate")]
    pub(super) fn is_disposed(&self) -> bool {
        self.save_generation.try_get_untracked().is_none()
    }

    /// The error toast for a save that did not land, worded for the storage
    /// class and, for a refused draft, the reason. The ONE copy of it, shared
    /// by Ctrl+S, the panels' Save buttons and the local autosave.
    pub(super) fn toast_not_saved(&self) {
        let server_draft = self
            .server_draft_share
            .try_with_untracked(Option::is_some)
            .unwrap_or(false);
        let refused = match self.draft_save_state.try_get_untracked() {
            Some(DraftSaveState::Refused(kind)) => Some(kind),
            _ => None,
        };
        self.toaster.toast(
            ToastIntent::Error,
            NOT_SAVED_TITLE,
            not_saved_body(server_draft, refused),
            NOT_SAVED_TOAST_SECS,
        );
    }
}

/// Title of the toast for a save that did not land.
const NOT_SAVED_TITLE: &str = "Not Saved";

/// Lifetime (s) of that toast: long enough to read the remedy.
const NOT_SAVED_TOAST_SECS: u32 = 8;

/// Body of the toast for a save that did not land.
fn not_saved_body(server_draft: bool, refused: Option<DraftRefusal>) -> String {
    match (server_draft, refused) {
        (true, Some(kind)) => format!(
            "{}. Your edits are still on this page; Download .ironpad keeps a copy.",
            kind.indicator()
        ),
        (true, None) => "Your draft did not reach the server. It keeps retrying in \
                         the background; the indicator beside the publish button \
                         clears once it lands."
            .to_string(),
        (false, _) => "This browser did not store the notebook (it may be out of \
                       space, or in private mode), so your latest changes exist \
                       only on this page. Download .ironpad to keep a copy."
            .to_string(),
    }
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
        use wasm_bindgen::JsCast;

        if self.reactive_timer_fn.get_value().is_some() {
            return;
        }

        let state = *self;
        let cells = self.cells;
        let cell_stale = self.cell_stale;
        let run_all_queue = self.run_all_queue;
        let reactive_timer = self.reactive_timer;

        let closure = wasm_bindgen::closure::Closure::<dyn Fn()>::new(move || {
            reactive_timer.set(None);

            // Preview unmounts the cells, so nothing would drain a queue
            // started there. Off is off: a timer armed before the toggle
            // must not run anything after it.
            if state.is_view_mode.get_untracked() || !state.reactive_mode.get_untracked() {
                return;
            }

            // A run is in progress: try again once it can have drained,
            // rather than dropping the edit. Nothing else would: the stale
            // map does not change when the queue empties, so the effect that
            // schedules this timer never fires again for these cells.
            if !run_all_queue.is_empty_untracked() {
                state.schedule_reactive_execution();
                return;
            }

            // Collect stale runnable cell IDs in notebook order.
            let all_cells = cells.get_untracked();
            let stale_map = cell_stale.get_untracked();
            let stale_ids: Vec<String> = all_cells
                .iter()
                .filter(|c| c.is_runnable() && stale_map.get(&c.id).copied().unwrap_or(false))
                .map(|c| c.id.clone())
                .collect();

            run_all_queue.set_if_idle(stale_ids);
        });
        let func: js_sys::Function = closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
        // Hold the closure in the reactive arena (dropped on disposal) instead
        // of leaking it with forget().
        StoredValue::new_local(closure);
        self.reactive_timer_fn.set_value(Some(func));

        // A timer still pending at unmount would call the dropped closure,
        // which throws. The re-arm above keeps one pending for as long as a
        // queue runs in reactive mode, so leaving mid-run is the ordinary
        // way to get there.
        on_cleanup(move || {
            if let (Some(window), Some(Some(handle))) =
                (web_sys::window(), reactive_timer.try_get_untracked())
            {
                window.clear_timeout_with_handle(handle);
            }
        });
    }

    /// No-op on the server side.
    #[cfg(not(feature = "hydrate"))]
    pub(super) fn init_reactive_timer(&self) {}

    /// Run All from `from_id` down (the whole notebook for `None`, or when
    /// the cell is gone): queue every runnable cell in notebook order, and
    /// leave the queue alone when there is none.
    ///
    /// Merges into a queue already in flight (`RunQueue::enqueue_many`): the
    /// running front stays in front and nothing queued is dropped, so an
    /// agent waiting on a queued cell is never stranded by a click.
    pub(super) fn enqueue_runnable_from(&self, from_id: Option<&str>) {
        self.cells.with_untracked(|cells| {
            let start = from_id
                .and_then(|id| cells.iter().position(|c| c.id == id))
                .unwrap_or(0);
            let ids = crate::components::executor::runnable_ids(&cells[start..]);
            let order: Vec<&str> = cells.iter().map(|c| c.id.as_str()).collect();
            self.run_all_queue.enqueue_many(&order, &ids);
        });
    }

    /// Queue a cell for execution on behalf of a session agent (PRD-0052).
    ///
    /// Appends to the same [`run_all_queue`](Self::run_all_queue) the Run All
    /// button and cascading execution use, so unexecuted prerequisites cascade
    /// exactly as they do for a human click. Returns `Err` for a cell that
    /// does not exist or cannot run on its own (markdown, shared).
    pub(crate) fn request_cell_run(&self, cell_id: &str) -> Result<(), String> {
        use ironpad_common::types::CellType;

        // The editor's per-cell run-queue watchers only exist while the cell
        // list is mounted, and View mode unmounts them (it renders a separate
        // ViewOnlyNotebook with its own private queue). Queuing here would
        // push onto a queue nothing drains — the daemon would wait out its
        // timeout, and the run would fire as a surprise when the user flips
        // back to Edit. Refuse with a distinct error the agent can surface.
        if self.is_view_mode.try_get_untracked() == Some(true) {
            return Err(format!(
                "cell {cell_id} cannot run: the host notebook is in View mode (the editor is not mounted). Switch it to Edit mode and retry."
            ));
        }

        let cells = self
            .cells
            .try_get_untracked()
            .ok_or("editor is shutting down")?;
        let Some(cell) = cells.iter().find(|c| c.id == cell_id) else {
            return Err(format!("cell {cell_id} not found"));
        };
        // Linux cells are refused for a different reason than markdown, and an
        // agent told its Linux cell "cannot run on its own" learns nothing
        // (PRD-0066 T-008). Refused deliberately rather than incidentally: a
        // run boots a metered pod, and a CLI run has no human watching it.
        if cell.cell_type == CellType::Linux {
            return Err(format!(
                "cell {cell_id} is a Linux cell, which an agent cannot run: every run boots a metered pod and a CLI run has nobody watching it. Run it from the notebook in a browser."
            ));
        }
        if !cell.is_runnable() {
            return Err(format!(
                "cell {cell_id} is not runnable (markdown and shared cells cannot run on their own)"
            ));
        }

        self.run_all_queue.push_unique(cell_id);
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

    /// Each cell's CURRENT source text, for dependency detection: the model
    /// as the base (it lags a live edit by the 1s save debounce), overlaid
    /// with the mounted Monaco editors' live values, which are exact.
    /// Collapsed cells have no mounted editor and cannot have unsaved edits,
    /// so the model is already correct for them.
    pub(super) fn current_cell_sources(&self) -> HashMap<String, String> {
        let mut map: HashMap<String, String> = self
            .notebook
            .try_with_untracked(|nb| {
                nb.as_ref().map(|nb| {
                    nb.cells
                        .iter()
                        .map(|c| (c.id.clone(), c.source.clone()))
                        .collect()
                })
            })
            .flatten()
            .unwrap_or_default();
        #[cfg(feature = "hydrate")]
        self.editor_handles.with_untracked(|handles| {
            for (id, handle) in handles {
                map.insert(id.clone(), handle.get_value());
            }
        });
        map
    }

    /// Execution-state cleanup for a deleted cell: cached outputs, display
    /// texts, the run-all queue, and BOTH sides of the blame map. ONE
    /// definition for the UI delete handler (`cell_item.rs`) and the agent's
    /// `CellDelete` mutation (`session/connection.rs`) — a queued cell
    /// deleted by either path would otherwise linger in the queue as a ghost
    /// front no watcher can advance, stalling everything behind it, and
    /// blame entries naming it could never clear except by page reload.
    pub(crate) fn scrub_deleted_cell(&self, cell_id: &str) {
        self.cell_outputs.update(|map| {
            map.remove(cell_id);
        });
        self.cell_display_texts.update(|map| {
            map.remove(cell_id);
        });
        self.run_all_queue.remove(cell_id);
        self.cell_blocked_by.update(|b| {
            b.remove(cell_id);
            b.retain(|_, blocker| blocker != cell_id);
        });
    }

    /// Continue-past-failures policy (PRD-0060): drop `failed_cell_id` and
    /// its transitive dependents from the run queue — marking the dependents
    /// blocked-by the failure so the UI says WHY they were skipped — while
    /// every independent queued cell keeps running. ONE definition for the
    /// three error paths (execution, compile, transport), delegating to the
    /// shared bookkeeping in `run_flow` (the viewer runs the same helper);
    /// the daemon's `prerequisite_failed` semantics mirror this recipe, so a
    /// fork here would desync agents from the UI. Returns the dropped
    /// dependents so the caller can report them (`Event::CellBlocked`).
    pub(super) fn fail_in_run_queue(&self, failed_cell_id: &str) -> Vec<String> {
        let cells = self.cells.get_untracked();
        let queue = self.run_all_queue.snapshot_untracked();
        let sources = self.current_cell_sources();
        let dependents = crate::components::executor::dependents_in_queue(
            &cells,
            &queue,
            failed_cell_id,
            |id| sources.get(id).cloned(),
        );
        self.run_all_queue
            .fail(self.cell_blocked_by, failed_cell_id, &dependents);
        dependents
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
        if state.persistence_halted() {
            return;
        }
        // Pending until a write takes it: the debounce below dies with the
        // page, and the spawned write cannot read a disposed one, so the
        // teardown write is what catches the edit then.
        state.mark_persist_pending();
        // `try_`: callers reach this after awaits, on pages that may be gone.
        let Some(share) = state.server_draft_share.try_get_untracked() else {
            return;
        };
        if share.is_some() {
            let _ = state.draft_dirty.try_set(true);
            schedule_draft_save(state, DRAFT_SAVE_DEBOUNCE_MS);
            return;
        }
        let state = *state;
        leptos::task::spawn_local(async move {
            if persist_notebook_durable(&state, false).await {
                return;
            }
            // This autosave runs on every change and has no caller to report
            // to, so a browser that refuses the write (out of space, private
            // mode) gets ONE toast per failure streak; a write that lands
            // re-arms it. Not when the page is gone or a flow halted saving:
            // those are not failures.
            if state.persistence_halted()
                || state.local_save_failed.try_get_untracked() != Some(false)
            {
                return;
            }
            let _ = state.local_save_failed.try_update_untracked(|f| *f = true);
            state.toast_not_saved();
        });
    }
}

/// Awaitable persist: resolves after the write completes, returning whether
/// it landed. In `ServerDraft` mode this saves the draft IMMEDIATELY (no
/// debounce): durable callers (the pre-Push flush) need the server to hold
/// current content, and Push aborts when this returns `false` — promoting a
/// draft the server never received would publish stale content.
///
/// `enrich_outputs` embeds the cells' last-run outputs into the draft
/// (PRD-0056). Push alone passes `true`: output embedding is for editorial
/// moments, and an explicit save that carried every output would also run
/// into the upload cap sooner.
#[cfg(feature = "hydrate")]
pub(super) async fn persist_notebook_durable(state: &NotebookState, enrich_outputs: bool) -> bool {
    if state.persistence_halted() {
        return false;
    }
    // Stamp the model in place: cloning the whole notebook to set one field
    // and cloning it again to write it back cost two deep copies per save.
    let stamped = state.notebook.try_update_untracked(|nb| {
        nb.as_mut()
            .map(|nb| nb.updated_at = chrono::Utc::now())
            .is_some()
    });
    if stamped != Some(true) {
        return false;
    }
    if state.server_draft_share.get_untracked().is_some() {
        let _ = state.draft_dirty.try_set(true);
        state.mark_persist_pending();
        // Ours must land last: an earlier request arriving after this write
        // would revert the draft the promote then publishes. `false` also
        // means the page was disposed during the wait, and nothing after an
        // await may assume it is alive.
        if !drain_inflight_drafts(state, None).await {
            return false;
        }
        // Supersede any pending debounce so it can't double-write after us.
        let _ = state.draft_save_epoch.try_update_untracked(|e| *e += 1);
        save_draft_now(state, enrich_outputs).await
    } else {
        // The one copy this path needs: the IndexedDB write awaits, so it
        // cannot borrow the signal across it.
        let Some(nb) = state.notebook.try_get_untracked().flatten() else {
            return false;
        };
        state.take_persist_pending();
        if let Err(e) = crate::storage::client::save_notebook(&nb).await {
            leptos::logging::error!("failed to persist notebook to IndexedDB: {e:?}");
            state.mark_persist_pending();
            false
        } else {
            let _ = state.local_save_failed.try_update_untracked(|f| *f = false);
            true
        }
    }
}

/// The editor's last write, as the page is torn down (navigation) or hidden
/// (Local mode's `pagehide`): if the model holds edits no write has landed
/// ([`NotebookState::persist_pending`]) and no flow halted persistence,
/// write them now.
///
/// Every read happens HERE, synchronously, while the signals are still
/// alive; the spawned write touches no signal, because by the time it runs
/// the page's arena is gone. On teardown this runs after every `CellItem`
/// has flushed its unsaved panes into the model (an owner cleans up its
/// children before running its own cleanups), so it carries the last
/// keystrokes too.
#[cfg(feature = "hydrate")]
pub(super) fn persist_on_teardown(state: &NotebookState) {
    if state.persist_pending.try_get_untracked() != Some(true) || state.persistence_halted() {
        return;
    }
    let Some(Some(mut nb)) = state.notebook.try_get_untracked() else {
        return;
    };
    let Some(share) = state.server_draft_share.try_get_untracked() else {
        return;
    };
    // A later teardown signal (a `pagehide`, then the page's disposal on
    // the way back from the bfcache) must not write the same content twice.
    state.take_persist_pending();
    if let Some(share_id) = share {
        let json = match serde_json::to_string(&nb) {
            Ok(json) => json,
            Err(e) => {
                leptos::logging::error!("draft serialize failed on teardown: {e}");
                return;
            }
        };
        leptos::task::spawn_local(async move {
            if let Err(e) = crate::server_fns::save_mutable_draft(share_id, json).await {
                leptos::logging::warn!("draft save on teardown failed: {e}");
            }
        });
    } else {
        nb.updated_at = chrono::Utc::now();
        leptos::task::spawn_local(async move {
            if let Err(e) = crate::storage::client::save_notebook(&nb).await {
                leptos::logging::error!("notebook save on teardown failed: {e:?}");
            }
        });
    }
}

/// Minimum time (ms) a panel's Saving… state stays visible: the write
/// usually completes imperceptibly fast, so the indicator is floored at the
/// longer of the actual write or this.
#[cfg(feature = "hydrate")]
const MIN_SAVING_MS: i32 = 500;

/// A panel's explicit Save: persist durably with `saving` held for at least
/// [`MIN_SAVING_MS`], then run `on_done` with whether the write landed (the
/// panel's saved toast, or [`NotebookState::toast_not_saved`]).
///
/// Awaits the actual write and reports its result, so a success toast means
/// "durably saved" rather than "save dispatched". `saving` is cleared with
/// `try_set`: the panel may have been disposed mid-save (navigation, a
/// section collapse). A write that landed still reports, and stays truthful;
/// one that stopped because the page went away does not, since the editor's
/// teardown write takes over and neither toast would be true.
#[allow(unused_variables)] // `saving` is read only under `hydrate`.
pub(super) fn persist_with_saving_floor(
    state: &NotebookState,
    saving: RwSignal<bool>,
    on_done: impl FnOnce(bool) + 'static,
) {
    #[cfg(feature = "hydrate")]
    {
        let state = *state;
        saving.set(true);
        leptos::task::spawn_local(async move {
            let started = js_sys::Date::now();
            let landed = persist_notebook_durable(&state, false).await;
            #[allow(clippy::cast_possible_truncation)]
            let remaining = MIN_SAVING_MS - (js_sys::Date::now() - started) as i32;
            if remaining > 0 {
                crate::components::run_flow::sleep_ms(remaining).await;
            }
            let _ = saving.try_set(false);
            if landed || !state.is_disposed() {
                on_done(landed);
            }
        });
    }
    #[cfg(not(feature = "hydrate"))]
    {
        persist_notebook(state);
        on_done(true);
    }
}

/// Wait until no draft-save POST is on the wire, so the caller's write (or
/// discard) lands after every earlier one: the server applies whichever
/// request arrives last, and an older save arriving late would revert it.
/// Bounded by [`DRAFT_SAVE_INFLIGHT_WAIT_MAX_MS`]: past the cap the caller
/// proceeds and relies on last-write-wins, so a hung request cannot wedge it.
///
/// Returns whether the caller should go ahead. `false` when the page was
/// disposed while waiting, or, given `stop_if_epoch_changed`, when a newer
/// save superseded the caller's epoch meanwhile.
#[cfg(feature = "hydrate")]
pub(super) async fn drain_inflight_drafts(
    state: &NotebookState,
    stop_if_epoch_changed: Option<u64>,
) -> bool {
    let mut waited_ms = 0;
    loop {
        let Some(inflight) = state.draft_save_inflight.try_get_untracked() else {
            return false;
        };
        if let Some(epoch) = stop_if_epoch_changed {
            if state.draft_save_epoch.try_get_untracked() != Some(epoch) {
                return false;
            }
        }
        if inflight == 0 || waited_ms >= DRAFT_SAVE_INFLIGHT_WAIT_MAX_MS {
            return true;
        }
        crate::components::run_flow::sleep_ms(DRAFT_SAVE_INFLIGHT_POLL_MS).await;
        waited_ms += DRAFT_SAVE_INFLIGHT_POLL_MS;
    }
}

/// Schedule a coalesced draft save `after_ms` from now. Every call bumps the
/// epoch; only the task holding the newest epoch actually writes, so a typing
/// burst produces one server round trip.
/// Single-flight: the task waits for any save already on the wire before
/// sending its own, so this page's saves reach the server in order.
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
        crate::components::run_flow::sleep_ms(after_ms).await;
        if state.draft_save_epoch.try_get_untracked() != Some(epoch) {
            return; // superseded by a newer edit or an immediate save
        }
        // Single-flight: an earlier save still on the wire (a slow
        // connection, a large notebook) would otherwise land after this
        // newer one and leave the server holding older content under a
        // clean indicator. Wait for it, unless a newer save supersedes
        // this one meanwhile; `save_draft_now` reads the model afresh.
        if !drain_inflight_drafts(&state, Some(epoch)).await {
            return;
        }
        // Lean: the debounce path never embeds outputs (PRD-0056) — only
        // the pre-Push durable save pays that weight.
        save_draft_now(&state, false).await;
    });
}

// ── Refused draft saves (PRD-0064) ──────────────────────────────────────────

/// Substrings marking a server refusal, lowercase, with the kind each one
/// means.
///
/// The autosave loop is retry-until-it-lands, which is right for a dropped
/// connection and wrong for a full account: the same bytes are refused every
/// time, forever, under an indicator that promises a retry. The wording is
/// produced in `server_fns.rs` (`admit_mutable_write`, `validate_notebook_upload`,
/// `ensure_share_owner`, `require_login`), plus `length limit` from the
/// request-body cap in front of every server fn, and
/// `quota_refusals_are_classified_permanent` reads those functions so the
/// pairs cannot drift apart in silence.
#[cfg(any(feature = "hydrate", test))]
const REFUSAL_MARKERS: [(&str, DraftRefusal); 7] = [
    ("store full", DraftRefusal::Quota),
    ("storage full", DraftRefusal::Quota),
    ("quota", DraftRefusal::Quota),
    ("too large", DraftRefusal::TooLarge),
    ("length limit", DraftRefusal::TooLarge),
    ("you do not own this share", DraftRefusal::NotOwner),
    ("sign in with github", DraftRefusal::SignedOut),
];

/// Whether a failed draft save was REFUSED, and why, rather than lost
/// (`None`, transient).
///
/// Substring matching over the server's own prose, because that is all the
/// wire carries: a server-fn error reaches the client wrapped ("error running
/// server function: …"), so the marker has to survive being embedded rather
/// than being the whole message. Erring toward "transient" is the safe
/// direction for anything unknown: that is a retry loop.
#[cfg(any(feature = "hydrate", test))]
pub(super) fn classify_draft_failure(message: &str) -> Option<DraftRefusal> {
    let lowered = message.to_ascii_lowercase();
    REFUSAL_MARKERS
        .iter()
        .find(|(marker, _)| lowered.contains(marker))
        .map(|&(_, kind)| kind)
}

/// Body of the one-time toast for a refused draft save: the server's message,
/// which names the limit where there is one, plus the act that changes the
/// answer.
///
/// Naming the remedy matters more here than usual — the same refusal also
/// aborts Push, so an owner it hits is locked out of both saving and
/// publishing, and nothing else on the page says why or what to do. Toasts
/// are text only, so the remedy is said rather than linked.
#[cfg(any(feature = "hydrate", test))]
pub(super) fn refusal_toast_body(kind: DraftRefusal, server_message: &str) -> String {
    let remedy = match kind {
        DraftRefusal::Quota => {
            "Delete a notebook from your account to free space (Download \
             .ironpad first if you want to keep it)."
        }
        DraftRefusal::TooLarge => {
            "A notebook can be at most 4 MiB. Remove cells or large outputs to \
             bring it under the limit; Download .ironpad keeps a copy meanwhile."
        }
        DraftRefusal::NotOwner => {
            "This notebook is no longer in your account: it was deleted or \
             moved. Use Download .ironpad to keep this text."
        }
        DraftRefusal::SignedOut => {
            "Sign in again from a new tab without closing this one; the draft \
             saves itself once you are signed in."
        }
    };
    format!(
        "{server_message}. {remedy} Nothing you have typed since the last \
         successful save has reached the server."
    )
}

/// Serialize the current notebook and write the server draft, updating the
/// indicator. Returns whether the write landed; a failure schedules a retry
/// (epoch-guarded, so an intervening edit's own save wins) UNLESS the server
/// refused it, which no retry can change.
///
/// `enrich_outputs` embeds the author's last-run panels (PRD-0056) into the
/// outgoing JSON, and on success into the model as well, so later lean
/// saves keep what was published. Only the pre-Push durable save sets it:
/// the promote then publishes those outputs, while the 1.5s debounce
/// autosaves embed nothing new.
#[cfg(feature = "hydrate")]
pub(super) async fn save_draft_now(state: &NotebookState, enrich_outputs: bool) -> bool {
    // Checked here as well as by the callers: a debounce that wakes after a
    // Delete or Discard went ahead must not write.
    if state.persistence_halted() {
        return false;
    }
    let Some(share_id) = state.server_draft_share.try_get_untracked().flatten() else {
        return false;
    };
    // The captures this save embeds, kept for writing back on success.
    let texts = if enrich_outputs {
        state.cell_display_texts.try_get_untracked()
    } else {
        None
    };
    let serialized = if enrich_outputs {
        // Enrichment edits the outgoing copy, and reaches the model only
        // once the server has it (below), so this path needs its own clone.
        let Some(mut nb) = state.notebook.try_get_untracked().flatten() else {
            return false;
        };
        if let Some(texts) = &texts {
            nb.embed_saved_outputs(texts, ironpad_common::types::SAVED_OUTPUT_BUDGET_BYTES);
        }
        serde_json::to_string(&nb)
    } else {
        // The debounced autosave serializes straight from the model.
        let Some(serialized) = state
            .notebook
            .try_with_untracked(|nb| nb.as_ref().map(serde_json::to_string))
            .flatten()
        else {
            return false;
        };
        serialized
    };
    let json = match serialized {
        Ok(json) => json,
        Err(e) => {
            leptos::logging::error!("draft serialize failed: {e}");
            return false;
        }
    };
    state.take_persist_pending();
    let prior = state
        .draft_save_state
        .try_get_untracked()
        .unwrap_or(DraftSaveState::Synced);
    let _ = state.draft_save_state.try_set(state_while_saving(prior));
    let _ = state.draft_save_inflight.try_update(|n| *n += 1);
    let result = crate::server_fns::save_mutable_draft(share_id, json).await;
    let _ = state
        .draft_save_inflight
        .try_update(|n| *n = n.saturating_sub(1));
    match result {
        Ok(()) => {
            let _ = state.draft_save_state.try_set(DraftSaveState::Synced);
            // The server's draft now carries these outputs, and the promote
            // publishes them. The model must carry them too: the next lean
            // autosave serializes the model, and without them it would put
            // the older outputs (or none) back into the draft, which the
            // next Push without a fresh run would then publish.
            if let Some(texts) = &texts {
                let _ = state.notebook.try_update_untracked(|nb| {
                    if let Some(nb) = nb {
                        nb.embed_saved_outputs(
                            texts,
                            ironpad_common::types::SAVED_OUTPUT_BUDGET_BYTES,
                        );
                    }
                });
            }
            true
        }
        Err(e) => {
            state.mark_persist_pending();
            let message = e.to_string();
            let refusal = classify_draft_failure(&message);
            // `None`: the page is gone, and nothing below applies to it.
            let Some(current) = state.draft_save_state.try_get_untracked() else {
                return false;
            };
            let step = after_save_failure(current, refusal);
            if refusal.is_some() {
                leptos::logging::warn!("draft autosave refused ({refusal:?}): {message}");
            } else {
                leptos::logging::warn!("draft autosave failed (will retry): {message}");
            }
            let _ = state.draft_save_state.try_set(step.next);
            if let (true, Some(kind)) = (step.toast_refusal, refusal) {
                state.toaster.toast(
                    ToastIntent::Error,
                    "Draft Not Saved",
                    refusal_toast_body(kind, &message),
                    REFUSAL_TOAST_SECS,
                );
            }
            if step.retry {
                schedule_draft_save(state, DRAFT_SAVE_RETRY_MS);
            }
            false
        }
    }
}

// ── Draft-save state machine ────────────────────────────────────────────────
//
// Pure, so the transitions are unit-tested: `save_draft_now` is hydrate-only
// and `cargo make test` cannot reach it.

/// What a failed draft save leads to.
#[cfg(any(feature = "hydrate", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DraftSaveStep {
    /// The indicator state to show.
    pub(super) next: DraftSaveState,
    /// Whether this refusal gets its one toast.
    pub(super) toast_refusal: bool,
    /// Whether to schedule a retry.
    pub(super) retry: bool,
}

/// The indicator while a save is on the wire.
///
/// A refusal stays on screen rather than flickering to "Saving draft…": it
/// is still the truth until a save lands, and it is what the post-await read
/// in `save_draft_now` relies on to see that this refusal was already
/// announced, across any number of overlapping saves.
#[cfg(any(feature = "hydrate", test))]
pub(super) fn state_while_saving(prior: DraftSaveState) -> DraftSaveState {
    match prior {
        DraftSaveState::Refused(kind) => DraftSaveState::Refused(kind),
        _ => DraftSaveState::Saving,
    }
}

/// The transition for a failed save, from the indicator state `current` read
/// after the request returned, and the refusal if it was one.
///
/// A refusal is said ONCE: the debounce fires on every typing burst, so a
/// toast per refusal would bury the message under copies of itself. Only a
/// success (which sets `Synced`) or a different refusal re-arms it. A
/// transient failure retries, and so does a lost session.
#[cfg(any(feature = "hydrate", test))]
pub(super) fn after_save_failure(
    current: DraftSaveState,
    refusal: Option<DraftRefusal>,
) -> DraftSaveStep {
    match refusal {
        Some(kind) => DraftSaveStep {
            next: DraftSaveState::Refused(kind),
            toast_refusal: current != DraftSaveState::Refused(kind),
            retry: kind.retries(),
        },
        None => DraftSaveStep {
            next: DraftSaveState::Failed,
            toast_refusal: false,
            retry: true,
        },
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
    fn publish_button_stays_armed_while_unpublished() {
        // An account notebook that has never been published (PRD-0064) is
        // permanently dirty by construction, but the button must not depend
        // on that: even a (nominally) clean unpublished notebook offers
        // Publish, because nothing is readable until it happens.
        assert_eq!(
            PublishButton::from_flags(false, true),
            PublishButton::Publish
        );
        assert_eq!(
            PublishButton::from_flags(false, false),
            PublishButton::Publish
        );
        assert!(PublishButton::from_flags(false, false).armed());
    }

    #[test]
    fn publish_button_reads_push_then_published_once_published() {
        assert_eq!(PublishButton::from_flags(true, true), PublishButton::Push);
        assert!(PublishButton::from_flags(true, true).armed());
        assert_eq!(
            PublishButton::from_flags(true, false),
            PublishButton::Published
        );
        assert!(
            !PublishButton::from_flags(true, false).armed(),
            "a published, clean notebook is the one resting state"
        );
    }

    #[test]
    fn publish_button_labels_and_titles_are_distinct() {
        let states = [
            PublishButton::Publish,
            PublishButton::Push,
            PublishButton::Published,
        ];
        for (i, a) in states.iter().enumerate() {
            for b in &states[i + 1..] {
                assert_ne!(a.label(), b.label(), "{a:?} and {b:?} share a label");
                assert_ne!(a.title(), b.title(), "{a:?} and {b:?} share a tooltip");
            }
        }
    }

    // ── Refused draft saves (PRD-0064) ──────────────────────────────────

    /// The bodies of the functions in `server_fns.rs` that word refusals.
    const SERVER_FNS: &str = include_str!("../../server_fns.rs");

    /// The body of the function whose signature starts `signature`.
    fn fn_body(signature: &str) -> &'static str {
        let body = SERVER_FNS
            .split_once(signature)
            .unwrap_or_else(|| panic!("{signature} was renamed or moved out of server_fns.rs"))
            .1;
        &body[..body
            .find("\n}\n")
            .unwrap_or_else(|| panic!("could not find the end of {signature}"))]
    }

    /// The first string literal after each `call` in `body`: the wording of
    /// each refusal it emits.
    fn literals_after(body: &'static str, call: &str) -> Vec<&'static str> {
        body.split(call)
            .skip(1)
            .map(|rest| {
                let open = rest.find('"').expect("refusal has no message literal") + 1;
                let len = rest[open..].find('"').expect("unterminated literal");
                &rest[open..open + len]
            })
            .collect()
    }

    #[test]
    fn quota_refusals_are_classified_permanent() {
        // The classifier keys on the SERVER's prose, so this pairing is only
        // as good as a check that reads the server. These functions are the
        // source of truth; a reworded refusal there would otherwise put the
        // autosave loop back on an infinite retry, silently, and the first
        // person to find out would be a user who had already lost the
        // afternoon's typing.
        let quota = literals_after(fn_body("async fn admit_mutable_write("), "anyhow::bail!(");
        assert_eq!(
            quota.len(),
            2,
            "expected the instance cap and the per-user cap to be the two \
             refusals admit_mutable_write can emit; found {quota:?}"
        );
        let size = literals_after(fn_body("fn validate_notebook_upload("), "anyhow::bail!(");
        let owner = literals_after(fn_body("async fn ensure_share_owner("), "anyhow::bail!(");
        let login = literals_after(fn_body("async fn require_login("), "ServerFnError::new(");
        assert_eq!(size.len(), 1, "validate_notebook_upload: {size:?}");
        assert_eq!(owner.len(), 1, "ensure_share_owner: {owner:?}");
        assert_eq!(login.len(), 1, "require_login: {login:?}");

        for (refusals, kind) in [
            (quota, DraftRefusal::Quota),
            (size, DraftRefusal::TooLarge),
            (owner, DraftRefusal::NotOwner),
            (login, DraftRefusal::SignedOut),
        ] {
            for refusal in refusals {
                assert_eq!(
                    classify_draft_failure(refusal),
                    Some(kind),
                    "misclassified: {refusal}"
                );
                // The client never sees the bare message: server_fn's
                // Display wraps it.
                let wrapped = format!("error running server function: {refusal}");
                assert_eq!(classify_draft_failure(&wrapped), Some(kind), "{wrapped}");
            }
        }
    }

    #[test]
    fn the_body_limit_refusal_is_too_large() {
        // The request-body cap answers before the server fn runs, with text
        // the client cannot even decode as a server-fn error.
        assert_eq!(
            classify_draft_failure(
                "error deserializing server function results: Invalid format: \
                 missing delimiter in \"Failed to buffer the request body: length \
                 limit exceeded\""
            ),
            Some(DraftRefusal::TooLarge)
        );
    }

    #[test]
    fn transport_failures_stay_transient() {
        // Everything that is not a refusal keeps today's behaviour, a retry.
        for message in [
            "error running server function: Failed to fetch",
            "error reaching server: NetworkError when attempting to fetch resource",
            "error running server function: mutable share not found or not yours",
        ] {
            assert_eq!(
                classify_draft_failure(message),
                None,
                "{message} would stop the retry loop"
            );
        }
    }

    #[test]
    fn refusal_toast_repeats_the_server_message_and_names_the_remedy() {
        let server = "account storage full: 40000000 bytes stored for this \
                      account, cap is 33554432 bytes";
        let body = refusal_toast_body(DraftRefusal::Quota, server);
        assert!(
            body.contains(server),
            "the limit is only named in the server's own message"
        );
        assert!(body.to_ascii_lowercase().contains("delete a notebook"));
    }

    #[test]
    fn each_refusal_names_its_own_remedy() {
        let message = "error running server function: refused";
        let too_large = refusal_toast_body(DraftRefusal::TooLarge, message);
        assert!(too_large.contains("4 MiB"), "{too_large}");
        assert!(
            !too_large.to_ascii_lowercase().contains("delete a notebook"),
            "deleting another notebook does not shrink this one"
        );
        let not_owner = refusal_toast_body(DraftRefusal::NotOwner, message);
        assert!(not_owner.contains("Download .ironpad"), "{not_owner}");
        let signed_out = refusal_toast_body(DraftRefusal::SignedOut, message);
        assert!(signed_out.contains("new tab"), "{signed_out}");

        let kinds = [
            DraftRefusal::Quota,
            DraftRefusal::TooLarge,
            DraftRefusal::NotOwner,
            DraftRefusal::SignedOut,
        ];
        for (i, a) in kinds.iter().enumerate() {
            for b in &kinds[i + 1..] {
                assert_ne!(a.indicator(), b.indicator());
                assert_ne!(a.push_failed_body(), b.push_failed_body());
            }
            assert!(!a.indicator().contains("retrying"));
        }
    }

    #[test]
    fn refused_stays_refused_while_the_next_save_is_in_flight() {
        let refused = DraftSaveState::Refused(DraftRefusal::Quota);
        assert_eq!(state_while_saving(refused), refused);
        for prior in [
            DraftSaveState::Synced,
            DraftSaveState::Saving,
            DraftSaveState::Failed,
        ] {
            assert_eq!(state_while_saving(prior), DraftSaveState::Saving);
        }
    }

    #[test]
    fn a_second_refusal_does_not_toast() {
        // The first refusal toasts; the indicator then holds Refused through
        // the next save, so the second refusal reads it back and stays quiet.
        let quota = Some(DraftRefusal::Quota);
        let first = after_save_failure(state_while_saving(DraftSaveState::Synced), quota);
        assert!(first.toast_refusal);
        assert_eq!(first.next, DraftSaveState::Refused(DraftRefusal::Quota));
        let second = after_save_failure(state_while_saving(first.next), quota);
        assert!(!second.toast_refusal, "the refusal was already announced");
        assert_eq!(second.next, first.next);
        // A DIFFERENT refusal is news.
        let other = after_save_failure(first.next, Some(DraftRefusal::TooLarge));
        assert!(other.toast_refusal);
    }

    #[test]
    fn success_then_refusal_toasts_again() {
        // A save that lands sets Synced, which re-arms the toast.
        let after = after_save_failure(
            state_while_saving(DraftSaveState::Synced),
            Some(DraftRefusal::Quota),
        );
        assert!(after.toast_refusal);
    }

    #[test]
    fn a_transient_failure_retries_and_a_refusal_does_not() {
        let transient = after_save_failure(DraftSaveState::Saving, None);
        assert_eq!(
            transient,
            DraftSaveStep {
                next: DraftSaveState::Failed,
                toast_refusal: false,
                retry: true,
            }
        );
        for kind in [
            DraftRefusal::Quota,
            DraftRefusal::TooLarge,
            DraftRefusal::NotOwner,
        ] {
            let refused = after_save_failure(DraftSaveState::Saving, Some(kind));
            assert!(!refused.retry, "{kind:?} is the same answer every time");
        }
    }

    #[test]
    fn a_lost_session_retries_but_toasts_once() {
        // Signing in again in another tab fixes it, so the retry stays; the
        // toast still says it once.
        let signed_out = Some(DraftRefusal::SignedOut);
        let first = after_save_failure(DraftSaveState::Saving, signed_out);
        assert!(first.retry && first.toast_refusal);
        let again = after_save_failure(state_while_saving(first.next), signed_out);
        assert!(again.retry && !again.toast_refusal);
    }

    #[test]
    fn draft_save_states_are_distinct() {
        let refused = DraftSaveState::Refused(DraftRefusal::Quota);
        assert_ne!(refused, DraftSaveState::Failed);
        assert_ne!(refused, DraftSaveState::Synced);
        assert_ne!(refused, DraftSaveState::Refused(DraftRefusal::SignedOut));
    }

    #[test]
    fn type_tags_skip_untagged_outputs() {
        let outputs: HashMap<String, CellOutputData> = [
            (
                "tagged",
                CellOutputData {
                    bytes: vec![1, 2, 3],
                    type_tag: Some("u32".to_string()),
                },
            ),
            ("untagged", CellOutputData::default()),
        ]
        .into_iter()
        .map(|(id, data)| (id.to_string(), data))
        .collect();
        assert_eq!(
            type_tags_of(&outputs),
            HashMap::from([("tagged".to_string(), "u32".to_string())])
        );
    }

    #[test]
    fn type_tags_ignore_output_bytes() {
        // The memo's point: a byte-only rewrite (a widget drag) projects to
        // an EQUAL map, so Memo equality stops it re-running every reader.
        let before = HashMap::from([(
            "w".to_string(),
            CellOutputData {
                bytes: vec![0],
                type_tag: Some("f64".to_string()),
            },
        )]);
        let mut after = before.clone();
        after.get_mut("w").expect("seeded").bytes = vec![9, 9, 9];
        assert_eq!(type_tags_of(&before), type_tags_of(&after));
    }

    #[test]
    fn cell_output_data_default_is_empty() {
        let data = CellOutputData::default();
        assert!(data.bytes.is_empty());
        assert!(data.type_tag.is_none());
    }
}
