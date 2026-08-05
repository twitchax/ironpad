mod cell_item;
mod cell_output;
mod export;
mod metadata_panel;
mod shared_editor_panel;
mod sharing;
mod skeleton;
pub(crate) mod state;

use std::collections::HashMap;

use ironpad_common::{CellType, IronpadNotebook};
use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use leptos_router::NavigateOptions;

use crate::components::app_layout::LayoutContext;
use crate::components::toaster::{ToastIntent, Toaster};
use crate::model::NotebookModel;
use crate::session::SessionState;

use crate::components::session_panel::SessionButton;
use crate::components::view_only_notebook::ViewOnlyNotebook;

use self::cell_item::CellItem;
use self::metadata_panel::NotebookMetadataSection;
use self::shared_editor_panel::{SharedEditorKind, SharedEditorSection};

#[cfg(feature = "hydrate")]
use self::sharing::{
    discard_draft_current_notebook, download_current_notebook, unpublish_current_notebook,
};
use self::sharing::{
    push_mutable_current_notebook, share_current_notebook, share_mutable_current_notebook,
};
use self::skeleton::{AddCellButton, NotebookEditorSkeleton};
use self::state::{persist_notebook, DraftSaveState, NotebookState};

// ── Flush-before-serialize helper (PRD-0032 T-007) ──────────────────────────
//
// Share, Export HTML, and Download .ironpad all serialize the notebook from
// `state.notebook`. Each cell's live editor content only reaches that signal
// on a 1s debounce or when `state.save_generation` bumps (see the flush
// effect in `cell_item.rs`). Bumping the generation alone isn't enough,
// though: Leptos effects run queued, not synchronously on `signal.update()`,
// so callers must yield before re-reading the notebook.

/// Delay (ms) after bumping `state.save_generation` before re-reading the
/// notebook, giving the per-cell flush effects time to run. Used by the
/// `sharing` workflows and Export HTML, which only need the in-memory model
/// flushed.
#[cfg(feature = "hydrate")]
const CELL_FLUSH_YIELD_MS: i32 = 120;

/// Awaits a `setTimeout` so queued Leptos effects — in particular each
/// cell's notebook-level save-flush effect (`cell_item.rs`) — get a chance
/// to run before the caller re-reads `state.notebook`. There's no
/// async-timer dependency in this workspace, so this wraps `setTimeout` in a
/// `Promise` (mirrors the `set_timeout_with_callback_and_timeout_and_arguments_0`
/// usage elsewhere in this crate). The
/// delay is a pragmatic yield for the effect queue, not a correctness
/// guarantee — see the `CELL_FLUSH_YIELD_MS` docs.
#[cfg(feature = "hydrate")]
pub(super) async fn yield_for_cell_flush(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if let Some(window) = web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        } else {
            // No window (shouldn't happen in hydrate) — resolve immediately.
            let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// Call `.destroy()` on a stored `SortableJS` instance (if present) and clear the
/// slot, so re-initialisation and unmount don't leave a second drag handler
/// bound to the cell list.
#[cfg(feature = "hydrate")]
fn destroy_sortable(instance: StoredValue<Option<wasm_bindgen::JsValue>, LocalStorage>) {
    use wasm_bindgen::JsCast;

    let existing = instance.try_update_value(Option::take).flatten();
    if let Some(existing) = existing {
        if let Some(destroy) = js_sys::Reflect::get(&existing, &"destroy".into())
            .ok()
            .and_then(|f| f.dyn_into::<js_sys::Function>().ok())
        {
            let _ = destroy.call0(&existing);
        }
    }
}

/// Route component for `/local/{id}`: the editor in Local (`IndexedDB`)
/// mode.
#[component]
pub fn NotebookEditorPage() -> impl IntoView {
    let params = use_params_map();
    // Tracked: the router reuses this outlet on a param-only change
    // (/local/a -> /local/b), so a frozen untracked read would keep editing
    // notebook `a` under `b`'s URL. Re-running the closure remounts the
    // editor wholesale — its cleanup (session teardown, listener removal)
    // already handles remounts.
    let notebook_id = Memo::new(move |_| params.read().get("id").unwrap_or_default());
    view! { {move || view! { <NotebookEditor notebook_id=notebook_id.get() /> }} }
}

/// The editor proper, storage-agnostic (PRD-0054). `/local/{id}` mounts it
/// in Local mode (loads from `IndexedDB`); the owner's view of
/// `/mutable/{id}` mounts it in `ServerDraft` mode with the preloaded draft,
/// and every persist then routes to the server (see `state.rs`).
#[component]
pub fn NotebookEditor(
    /// The notebook uuid: the `IndexedDB` key in Local mode, the embedded id
    /// in `ServerDraft` mode. Sessions key on it either way.
    notebook_id: String,
    /// `Some((share_id, notebook, server_dirty))` mounts `ServerDraft` mode.
    #[prop(default = None)]
    server_draft: Option<(String, IronpadNotebook, bool)>,
) -> impl IntoView {
    // Set up notebook-level reactive state.

    let state = NotebookState {
        notebook: RwSignal::new(None),
        notebook_id: RwSignal::new(notebook_id.clone()),
        cells: RwSignal::new(Vec::new()),
        active_cell: RwSignal::new(None),
        pending_focus_cell: RwSignal::new(None),
        cell_outputs: RwSignal::new(HashMap::new()),
        save_generation: RwSignal::new(0),
        run_all_queue: RwSignal::new(Vec::new()),
        shared_cargo_toml: RwSignal::new(None),
        shared_source: RwSignal::new(None),
        cell_stale: RwSignal::new(HashMap::new()),
        warm_manifests: RwSignal::new(std::collections::HashSet::new()),
        cell_display_texts: RwSignal::new(HashMap::new()),
        editor_handles: RwSignal::new(HashMap::new()),
        is_view_mode: RwSignal::new(false),
        force_recompile: RwSignal::new(false),
        reactive_mode: RwSignal::new(false),
        reactive_timer: RwSignal::new(None),
        cell_blocked_by: RwSignal::new(HashMap::new()),
        external_content_generation: RwSignal::new(0),
        #[cfg(feature = "hydrate")]
        reactive_timer_fn: StoredValue::new_local(None),
        server_draft_share: RwSignal::new(None),
        draft_dirty: RwSignal::new(false),
        draft_save_state: RwSignal::new(DraftSaveState::Synced),
        draft_save_epoch: RwSignal::new(0),
        draft_save_inflight: RwSignal::new(0),
    };
    // Build the reactive-debounce callback once, under this component's owner,
    // so it's dropped on unmount instead of leaked per edit.
    #[cfg(feature = "hydrate")]
    state.init_reactive_timer();
    let model = NotebookModel::new(
        state.notebook,
        state.cells,
        state.cell_stale,
        state.external_content_generation,
    );
    let session_state = SessionState::new();
    provide_context(state);
    provide_context(model);
    provide_context(session_state);

    // The session cannot outlive this page: the browser IS the model server,
    // and the socket's handlers capture this page's signals. Left open after
    // disposal, the next incoming agent message would read disposed signals
    // and panic the reactive runtime (leaving every page render-only until a
    // hard reload). Guests receive SessionEnded and can reconnect to a new
    // session.
    #[cfg(feature = "hydrate")]
    on_cleanup(move || crate::session::end_session(&session_state));

    // Load the notebook: `ServerDraft` mode arrives preloaded (the mutable
    // page already fetched the draft); Local mode reads IndexedDB.

    if let Some((share_id, nb, server_dirty)) = server_draft {
        state.server_draft_share.set(Some(share_id));
        state.draft_dirty.set(server_dirty);
        state.notebook.set(Some(nb));
        model.sync_from_notebook();
    } else {
        #[cfg(feature = "hydrate")]
        {
            let nb_id = notebook_id;
            leptos::task::spawn_local(async move {
                if let Some(nb) = crate::storage::client::get_notebook(&nb_id).await {
                    // The page may already be gone (navigated away while the
                    // IndexedDB read was in flight); sync_from_notebook READS
                    // the notebook signal, which panics once disposed.
                    if state.notebook.try_get_untracked().is_none() {
                        return;
                    }
                    state.notebook.set(Some(nb));
                    model.sync_from_notebook();
                }
            });
        }
    }

    // Wire up LayoutContext when notebook data arrives.

    let layout = expect_context::<LayoutContext>();

    Effect::new(move || {
        if let Some(nb) = state.notebook.get() {
            layout.notebook_title.set(Some(nb.title.clone()));
            layout.cell_count.set(nb.cells.len());
            state.notebook_id.set(nb.id.to_string());
            state.shared_cargo_toml.set(nb.shared_cargo_toml.clone());
            state.shared_source.set(nb.shared_source.clone());
            state.reactive_mode.set(nb.reactive_mode.unwrap_or(false));
        }
    });

    // Clear blocked-by state when reactive mode is turned off.
    Effect::new(move || {
        let reactive = state.reactive_mode.get();
        if !reactive {
            state.cell_blocked_by.set(HashMap::new());
        }
    });

    // ── Global keyboard shortcuts ───────────────────────────────────────

    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::prelude::*;

        let closure =
            Closure::<dyn Fn(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
                if (e.ctrl_key() || e.meta_key()) && e.key() == "s" {
                    e.prevent_default();
                    layout.save_generation.update(|g| *g += 1);
                }

                // Ctrl+Shift+Enter — run all cells from top.
                if (e.ctrl_key() || e.meta_key()) && e.shift_key() && e.key() == "Enter" {
                    e.prevent_default();
                    let cell_ids: Vec<String> = state
                        .cells
                        .get_untracked()
                        .iter()
                        .filter(|c| c.is_runnable())
                        .map(|c| c.id.clone())
                        .collect();
                    if !cell_ids.is_empty() {
                        state.run_all_queue.set(cell_ids);
                    }
                }

                // Ctrl+Shift+N — add new cell below the current active cell.
                if (e.ctrl_key() || e.meta_key())
                    && e.shift_key()
                    && (e.key() == "N" || e.key() == "n")
                {
                    e.prevent_default();
                    let after_cell_id = state.active_cell.get_untracked();
                    if let Ok((result, _event)) = model.apply(
                        ironpad_common::protocol::Mutation::CellAdd {
                            cell: ironpad_common::protocol::NewCell {
                                source: "42".to_string(),
                                cell_type: CellType::Code,
                                shared: false,
                                label: format!(
                                    "Cell {}",
                                    state.notebook.with_untracked(|nb| nb
                                        .as_ref()
                                        .map_or(0, |n| n.cells.len()))
                                ),
                                cargo_toml: None,
                            },
                            after_cell_id,
                        },
                        ironpad_common::protocol::ClientId::browser(),
                    ) {
                        if let ironpad_common::protocol::MutationResult::CellAdded {
                            cell_id, ..
                        } = result
                        {
                            state.pending_focus_cell.set(Some(cell_id));
                        }
                        persist_notebook(&state);
                    }
                }
            });

        web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .add_event_listener_with_callback("keydown", closure.as_ref().unchecked_ref())
            .unwrap();

        // Store the closure so it's dropped on dispose, and remove the listener
        // on unmount — otherwise each notebook mount stacks another document
        // keydown handler firing Ctrl+S / Ctrl+Shift+N against a disposed scope.
        let stored = StoredValue::new_local(closure);
        on_cleanup(move || {
            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                stored.try_with_value(|closure| {
                    let _ = doc.remove_event_listener_with_callback(
                        "keydown",
                        closure.as_ref().unchecked_ref(),
                    );
                });
            }
        });
    }

    // ── Save-generation watcher ─────────────────────────────────────────
    //
    // When a save fires (Ctrl+S, title commit), propagate to cells,
    // persist the notebook to IndexedDB, and show feedback.

    #[cfg(feature = "hydrate")]
    {
        let toaster = Toaster::expect_context();
        let prev_gen = RwSignal::new(layout.save_generation.get_untracked());

        Effect::new(move || {
            let gen = layout.save_generation.get();
            if gen == prev_gen.get_untracked() {
                return;
            }
            prev_gen.set(gen);

            // Signal all cells to flush their pending content.
            state.save_generation.update(|g| *g += 1);

            // Update title from layout into the notebook signal.
            let title = layout.notebook_title.get_untracked().unwrap_or_default();
            let _ = model.apply(
                ironpad_common::protocol::Mutation::NotebookUpdateMeta {
                    meta: ironpad_common::protocol::NotebookMetaPatch {
                        title: Some(title),
                        ..Default::default()
                    },
                },
                ironpad_common::protocol::ClientId::browser(),
            );

            // Persist to IndexedDB.
            persist_notebook(&state);

            layout.last_save_time.set(Some(js_sys::Date::now()));

            toaster.toast(
                ToastIntent::Success,
                "Notebook saved",
                "All changes have been saved.",
                3,
            );
        });
    }

    // Gate on a memoized boolean so NotebookContent is built once when the
    // notebook loads, not rebuilt on every content mutation. cell_add/delete/
    // reorder/meta all call the tracked notebook.update; reading notebook
    // directly here tore down and rebuilt every CellItem (resetting its local
    // status/output/diagnostics signals). The inner <For> reacts to
    // state.cells incrementally, so nothing else is needed to keep the list live.
    let notebook_loaded = Memo::new(move |_| state.notebook.with(Option::is_some));
    view! {
        <div class="ironpad-editor">
            <Show
                when=move || notebook_loaded.get()
                fallback=|| view! { <NotebookEditorSkeleton /> }
            >
                <NotebookContent />
            </Show>
        </div>
    }
}

// ── Notebook content ────────────────────────────────────────────────────────

/// Renders the ordered cell list with add-cell buttons.
#[component]
fn NotebookContent() -> impl IntoView {
    let state = expect_context::<NotebookState>();
    let model = expect_context::<NotebookModel>();

    // ── Add cell callback ───────────────────────────────────────────────

    let add_cell_cb = Callback::new(move |(after, cell_type): (Option<String>, CellType)| {
        let default_source = if cell_type == CellType::Markdown {
            "# New Section\n\nAdd your notes here.".to_string()
        } else {
            "42".to_string()
        };
        if let Ok((result, _event)) = model.apply(
            ironpad_common::protocol::Mutation::CellAdd {
                cell: ironpad_common::protocol::NewCell {
                    source: default_source,
                    cell_type,
                    shared: false,
                    label: format!(
                        "Cell {}",
                        state
                            .notebook
                            .with_untracked(|nb| nb.as_ref().map_or(0, |n| n.cells.len()))
                    ),
                    cargo_toml: None,
                },
                after_cell_id: after,
            },
            ironpad_common::protocol::ClientId::browser(),
        ) {
            if let ironpad_common::protocol::MutationResult::CellAdded { cell_id, .. } = result {
                state.pending_focus_cell.set(Some(cell_id));
            }
            persist_notebook(&state);
        }
    });

    // ── Dropdown state ─────────────────────────────────────────────────

    let hamburger_open = RwSignal::new(false);
    let gear_open = RwSignal::new(false);

    // ── Mutable-share binding (PRD-0054) ────────────────────────────────
    //
    // `Some(share_id)` iff this editor is in `ServerDraft` mode — set at mount
    // from the route, never from local storage (there is none for published
    // notebooks anymore). Drives the menu swaps and the metadata panel.
    let mutable_binding = state.server_draft_share;

    // ── Cells container ref for SortableJS ──────────────────────────────

    let cells_container_ref = NodeRef::new();

    // ── Close button navigation ─────────────────────────────────────────

    // Held as a StoredValue: the render tree below sits inside a re-runnable
    // <Show>, whose children closure must be Fn — moving the navigator into
    // per-call closures would make it FnOnce. get_value() clones per use.
    let navigate = StoredValue::new_local(use_navigate());

    // ── Outside-click handler to close dropdowns ────────────────────────

    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::prelude::*;

        let click_closure =
            Closure::<dyn Fn(web_sys::MouseEvent)>::new(move |e: web_sys::MouseEvent| {
                if let Some(target) = e.target() {
                    let el: &web_sys::Element = target.unchecked_ref();
                    if el
                        .closest(".ironpad-toolbar-dropdown")
                        .ok()
                        .flatten()
                        .is_none()
                    {
                        hamburger_open.set(false);
                        gear_open.set(false);
                    }
                }
            });
        web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .add_event_listener_with_callback("click", click_closure.as_ref().unchecked_ref())
            .unwrap();
        // Store the closure so it's dropped on dispose, and remove the listener
        // on unmount so remounts don't stack stale outside-click handlers.
        let stored = StoredValue::new_local(click_closure);
        on_cleanup(move || {
            if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
                stored.try_with_value(|closure| {
                    let _ = doc.remove_event_listener_with_callback(
                        "click",
                        closure.as_ref().unchecked_ref(),
                    );
                });
            }
        });
    }

    // ── SortableJS initialization ───────────────────────────────────────

    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::prelude::*;

        /// `SortableJS` `onEnd` callback type, held at component scope.
        type SortableOnEnd = Closure<dyn Fn(JsValue)>;

        // Holds the live SortableJS instance so it can be destroyed on unmount.
        // Without destroy(), a client-side nav away-and-back would leave the old
        // instance bound to the (recycled) list and stack a second drag handler.
        let sortable_instance: StoredValue<Option<JsValue>, LocalStorage> =
            StoredValue::new_local(None);
        // Holds the live onEnd closure at COMPONENT scope. It must not live in
        // the effect's per-run scope: a re-run disposes that scope, and if any
        // early-return path left the old Sortable instance alive it would call
        // a dropped closure on the next drag.
        let sortable_on_end: StoredValue<Option<SortableOnEnd>, LocalStorage> =
            StoredValue::new_local(None);

        let cells_ref = cells_container_ref;
        Effect::new(move || {
            // Tear down any instance from a previous run FIRST — before every
            // early return — so no path can leave an old instance alive while
            // its closure is replaced below.
            destroy_sortable(sortable_instance);

            // The container only renders in edit mode (view mode swaps in the
            // ViewOnlyNotebook renderer), so a Some ref implies edit mode.
            let Some(el) = cells_ref.get() else { return };
            let el: JsValue = JsValue::from(el);

            let sortable_class =
                js_sys::Reflect::get(&web_sys::window().unwrap(), &"Sortable".into()).ok();
            let Some(sortable_class) = sortable_class.filter(JsValue::is_function) else {
                return;
            };

            let options = js_sys::Object::new();
            let _ =
                js_sys::Reflect::set(&options, &"handle".into(), &".ironpad-drag-handle".into());
            let _ = js_sys::Reflect::set(&options, &"animation".into(), &150.into());
            let _ = js_sys::Reflect::set(
                &options,
                &"ghostClass".into(),
                &"ironpad-sortable-ghost".into(),
            );

            let on_end = Closure::<dyn Fn(JsValue)>::new(move |evt: JsValue| {
                // JS indices are non-negative integers that fit in usize.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let old_index = js_sys::Reflect::get(&evt, &"oldIndex".into())
                    .ok()
                    .and_then(|v| v.as_f64())
                    .map(|v| v as usize);
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let new_index = js_sys::Reflect::get(&evt, &"newIndex".into())
                    .ok()
                    .and_then(|v| v.as_f64())
                    .map(|v| v as usize);

                if let (Some(old_idx), Some(new_idx)) = (old_index, new_index) {
                    if old_idx != new_idx {
                        let mut ids: Vec<String> = state
                            .cells
                            .get_untracked()
                            .iter()
                            .map(|c| c.id.clone())
                            .collect();
                        if old_idx < ids.len() && new_idx < ids.len() {
                            let id = ids.remove(old_idx);
                            ids.insert(new_idx, id);
                            if model
                                .apply(
                                    ironpad_common::protocol::Mutation::CellReorder {
                                        cell_ids: ids,
                                    },
                                    ironpad_common::protocol::ClientId::browser(),
                                )
                                .is_ok()
                            {
                                persist_notebook(&state);
                            }
                        }
                    }
                }
            });
            let _ =
                js_sys::Reflect::set(&options, &"onEnd".into(), on_end.as_ref().unchecked_ref());
            // Store at component scope (replacing any previous closure — its
            // instance was already destroyed above) instead of leaking with
            // forget() or dying with this effect run's scope.
            sortable_on_end.set_value(Some(on_end));

            let create_fn = js_sys::Reflect::get(&sortable_class, &"create".into())
                .ok()
                .filter(JsValue::is_function);
            if let Some(create_fn) = create_fn {
                let create_fn: js_sys::Function = create_fn.unchecked_into();
                if let Ok(instance) = create_fn.call2(&sortable_class, &el, &options) {
                    sortable_instance.set_value(Some(instance));
                }
            }
        });

        // Destroy the SortableJS instance when this notebook view unmounts.
        on_cleanup(move || destroy_sortable(sortable_instance));
    }

    // ── Reactive dataflow: schedule re-execution when cells go stale ────

    Effect::new(move || {
        let stale = state.cell_stale.get();
        let reactive = state.reactive_mode.get_untracked();

        if reactive && stale.values().any(|&s| s) {
            state.schedule_reactive_execution();
        }
    });

    // ── Render ──────────────────────────────────────────────────────────

    view! {
        // Edit mode renders the editing scaffold; view mode swaps in the SAME
        // renderer the public/shared/embed pages use, so the preview cannot
        // drift from the published look. The swap remounts per entry with a
        // fresh snapshot (agent edits mid-view appear on re-toggle).
        //
        // The toolbar renders in BOTH modes: the notebook-level actions
        // (hamburger menu, close) apply to view mode too. Editing-only
        // chrome (Run All, session, gear) stays behind the edit-mode Show —
        // view mode's own header already carries Run All and the
        // cache/fresh toggle, and Reactive Mode is an edit-time setting.
        <div class="ironpad-notebook-toolbar">
            <Show when=move || !state.is_view_mode.get()>
                // ── Run All button ──────────────────────────────────────
                <button
                    class="ironpad-run-all-button"
                    title="Run all code cells (Ctrl+Shift+Enter)"
                    on:click=move |_| {
                        let cell_ids: Vec<String> = state
                            .cells
                            .get_untracked()
                            .iter()
                            .filter(|c| c.is_runnable())
                            .map(|c| c.id.clone())
                            .collect();
                        if !cell_ids.is_empty() {
                            state.run_all_queue.set(cell_ids);
                        }
                    }
                >
                    "▶▶ Run All"
                </button>

                <SessionButton />

                // ── Push button + draft indicator (PRD-0054) ────────────
                // `ServerDraft` mode only: the one editorial control. Grayed
                // "Published" when the draft matches; an active "Push" the
                // moment an edit lands.
                {move || state.server_draft_share.get().map(|share_id| view! {
                    <span class="ironpad-draft-indicator">
                        {move || match state.draft_save_state.get() {
                            DraftSaveState::Saving => "Saving draft…",
                            DraftSaveState::Failed => "Draft not saved; retrying",
                            DraftSaveState::Synced => "",
                        }}
                    </span>
                    <button
                        class="ironpad-push-button"
                        disabled=move || !state.draft_dirty.get()
                        title=move || if state.draft_dirty.get() {
                            "Publish your draft: readers see it after this"
                        } else {
                            "The published copy matches your draft"
                        }
                        on:click=move |_| {
                            if !state.draft_dirty.get_untracked() {
                                return;
                            }
                            push_mutable_current_notebook(
                                &state,
                                Toaster::expect_context(),
                                share_id.clone(),
                            );
                        }
                    >
                        {move || if state.draft_dirty.get() { "⬆ Push" } else { "Published ✓" }}
                    </button>
                })}
            </Show>

            <div class="ironpad-toolbar-right">
                // ── Hamburger dropdown (☰) ──────────────────────────────
                <div class="ironpad-toolbar-dropdown">
                    <button
                        class="ironpad-toolbar-dropdown-toggle"
                        title="Notebook menu"
                        aria-label="Notebook menu"
                        on:click=move |_| {
                            gear_open.set(false);
                            hamburger_open.update(|v| *v = !*v);
                        }
                    >
                        "☰"
                    </button>
                    {move || {
                        if hamburger_open.get() {
                            view! {
                                <div class="ironpad-toolbar-dropdown-menu">
                                    // Share
                                    <button
                                        class="ironpad-toolbar-dropdown-item"
                                        on:click=move |_| {
                                            hamburger_open.set(false);
                                            // The sharing workflows own the
                                            // flush-before-serialize discipline
                                            // (PRD-0032 T-007).
                                            share_current_notebook(
                                                &state,
                                                Toaster::expect_context(),
                                            );
                                        }
                                    >
                                        "↗ Share Immutable"
                                    </button>
                                    // Share Mutable / Push Update (PRD-0049)
                                    {move || match mutable_binding.get() {
                                        None => view! {
                                            <button
                                                class="ironpad-toolbar-dropdown-item"
                                                on:click=move |_| {
                                                    hamburger_open.set(false);
                                                    share_mutable_current_notebook(
                                                        &state,
                                                        Toaster::expect_context(),
                                                    );
                                                }
                                            >
                                                "⇅ Share Mutable"
                                            </button>
                                        }.into_any(),
                                        Some(share_id) => {
                                            // Push lives in the toolbar button now
                                            // (PRD-0054); the menu keeps the
                                            // secondary actions.
                                            let reader_href =
                                                format!("/mutable/{share_id}?view=reader");
                                            let discard_share_id = share_id.clone();
                                            view! {
                                                <a
                                                    class="ironpad-toolbar-dropdown-item"
                                                    href=reader_href
                                                    rel="external"
                                                    on:click=move |_| hamburger_open.set(false)
                                                >
                                                    "◎ View as Reader"
                                                </a>
                                                <button
                                                    class="ironpad-toolbar-dropdown-item"
                                                    on:click=move |_| {
                                                        hamburger_open.set(false);
                                                        #[cfg(feature = "hydrate")]
                                                        discard_draft_current_notebook(
                                                            &state,
                                                            Toaster::expect_context(),
                                                            discard_share_id.clone(),
                                                        );
                                                    }
                                                >
                                                    "⎌ Discard Draft"
                                                </button>
                                            }.into_any()
                                        },
                                    }}
                                    // Export HTML
                                    <button
                                        class="ironpad-toolbar-dropdown-item"
                                        on:click=move |_| {
                                            hamburger_open.set(false);
                                            #[cfg(feature = "hydrate")]
                                            {
                                                // Flush cells' in-progress editor content
                                                // before building the export, so "type then
                                                // immediately Export" doesn't produce a stale
                                                // artifact (PRD-0032 T-007).
                                                state.save_generation.update(|g| *g += 1);
                                                leptos::task::spawn_local(async move {
                                                    yield_for_cell_flush(CELL_FLUSH_YIELD_MS)
                                                        .await;
                                                    // try_: disposal can land
                                                    // during the yield.
                                                    let Some(nb) = state
                                                        .notebook
                                                        .try_get_untracked()
                                                        .flatten()
                                                    else {
                                                        return;
                                                    };
                                                    let Some(display_texts) = state
                                                        .cell_display_texts
                                                        .try_get_untracked()
                                                    else {
                                                        return;
                                                    };
                                                    let html =
                                                        export::build_export_html(&nb, &display_texts);
                                                    export::trigger_html_download(&html, &nb.title);
                                                });
                                            }
                                        }
                                    >
                                        "⊞ Export HTML"
                                    </button>
                                    // Download .ironpad
                                    <button
                                        class="ironpad-toolbar-dropdown-item"
                                        on:click=move |_| {
                                            hamburger_open.set(false);
                                            #[cfg(feature = "hydrate")]
                                            download_current_notebook(
                                                &state,
                                                Toaster::expect_context(),
                                            );
                                        }
                                    >
                                        "↓ Download .ironpad"
                                    </button>
                                    // Delete (private) / Unpublish (mutable, PRD-0049)
                                    {move || match mutable_binding.get() {
                                        Some(share_id) => view! {
                                            <button
                                                class="ironpad-toolbar-dropdown-item ironpad-toolbar-dropdown-item--danger"
                                                on:click=move |_| {
                                                    hamburger_open.set(false);
                                                    #[cfg(feature = "hydrate")]
                                                    unpublish_current_notebook(
                                                        &state,
                                                        Toaster::expect_context(),
                                                        share_id.clone(),
                                                    );
                                                }
                                            >
                                                "⊗ Unpublish"
                                            </button>
                                        }.into_any(),
                                        None => view! {
                                            <button
                                                class="ironpad-toolbar-dropdown-item ironpad-toolbar-dropdown-item--danger"
                                                on:click=move |_| {
                                                    hamburger_open.set(false);
                                                    #[cfg(feature = "hydrate")]
                                                    {
                                                        let id = state.notebook_id.get_untracked();
                                                        let confirmed = web_sys::window()
                                                            .unwrap()
                                                            .confirm_with_message(
                                                                "Delete this notebook? This cannot be undone.",
                                                            )
                                                            .unwrap_or(false);
                                                        if confirmed {
                                                            let navigate = navigate.get_value();
                                                            leptos::task::spawn_local(async move {
                                                                crate::storage::client::delete_notebook(&id)
                                                                    .await;
                                                                navigate("/", NavigateOptions::default());
                                                            });
                                                        }
                                                    }
                                                }
                                            >
                                                "╳ Delete"
                                            </button>
                                        }.into_any(),
                                    }}
                                </div>
                            }
                                .into_any()
                        } else {
                            view! { <div style="display:none" /> }.into_any()
                        }
                    }}
                </div>

                // ── Gear dropdown (⚙) — edit mode only ──────────────────
                <Show when=move || !state.is_view_mode.get()>
                <div class="ironpad-toolbar-dropdown">
                    <button
                        class="ironpad-toolbar-dropdown-toggle"
                        title="Notebook settings"
                        aria-label="Notebook settings"
                        on:click=move |_| {
                            hamburger_open.set(false);
                            gear_open.update(|v| *v = !*v);
                        }
                    >
                        "⚙"
                    </button>
                    {move || {
                        if gear_open.get() {
                            view! {
                                <div class="ironpad-toolbar-dropdown-menu">
                                    <button
                                        class="ironpad-toolbar-dropdown-item"
                                        on:click=move |_| {
                                            state.force_recompile.update(|v| *v = !*v);
                                        }
                                    >
                                        {move || {
                                            if state.force_recompile.get() {
                                                "↻ Force Recompile ✓"
                                            } else {
                                                "↻ Force Recompile"
                                            }
                                        }}
                                    </button>
                                    <button
                                        class="ironpad-toolbar-dropdown-item"
                                        on:click=move |_| {
                                            let on = !state.reactive_mode.get_untracked();
                                            state.reactive_mode.set(on);
                                            // Persist: the toggle used to flip only the
                                            // in-memory signal and was lost on reload.
                                            if model
                                                .apply(
                                                    ironpad_common::protocol::Mutation::NotebookUpdateMeta {
                                                        meta: ironpad_common::protocol::NotebookMetaPatch {
                                                            reactive_mode: Some(on),
                                                            ..Default::default()
                                                        },
                                                    },
                                                    ironpad_common::protocol::ClientId::browser(),
                                                )
                                                .is_ok()
                                            {
                                                persist_notebook(&state);
                                            }
                                        }
                                    >
                                        {move || {
                                            if state.reactive_mode.get() {
                                                "⚡ Reactive Mode ✓"
                                            } else {
                                                "⚡ Reactive Mode"
                                            }
                                        }}
                                    </button>
                                </div>
                            }
                                .into_any()
                        } else {
                            view! { <div style="display:none" /> }.into_any()
                        }
                    }}
                </div>
                </Show>

                // ── Close button (✕) ────────────────────────────────────
                <button
                    class="ironpad-toolbar-close"
                    title="Back to notebook list"
                    on:click=move |_| {
                        navigate.get_value()("/", NavigateOptions::default());
                    }
                >
                    "✕"
                </button>
            </div>
        </div>

        <Show when=move || !state.is_view_mode.get()>
        <div class="ironpad-cell-list ironpad-cells-container" node_ref=cells_container_ref>
            <AddCellButton after_cell_id=None on_add=add_cell_cb />

            <Show when=move || state.cells.get().is_empty()>
                <div class="ironpad-empty-notebook">
                    <p class="ironpad-empty-notebook-title">"This notebook is empty"</p>
                    <p class="ironpad-empty-notebook-hint">
                        "Add a Code or Markdown cell above to get started."
                    </p>
                </div>
            </Show>

            <For
                each=move || state.cells.get()
                key=|cell| cell.id.clone()
                let:cell
            >
                <CellItem cell=cell.clone() />
                <AddCellButton after_cell_id=Some(cell.id.clone()) on_add=add_cell_cb />
            </For>
        </div>

        // ── Shared source / dependencies appendix ───────────────────────
        //
        // Notebook-level shared code lives below the cells as collapsed
        // sections, mirroring the view-only pages: the cells are the story,
        // the shared code is the footnotes. Outside the sortable container so
        // drag-reorder indices stay cell-only.
        <div class="ironpad-editor-shared-appendix">
            <SharedEditorSection kind=SharedEditorKind::Source />
            <SharedEditorSection kind=SharedEditorKind::Dependencies />
        </div>

        // Its own wrapper rather than a third child of the appendix above:
        // the shared-appendix e2e spec indexes that container positionally,
        // so adding a sibling there would silently retarget its assertions.
        <div class="ironpad-editor-metadata-appendix">
            <NotebookMetadataSection mutable_binding=mutable_binding />
        </div>
        </Show>

        // ── View mode: the canonical public renderer ────────────────────
        {move || {
            state.is_view_mode.get().then(|| {
                // Untracked snapshot: the preview is a still image of the
                // model at entry (the toggle handler flushes edits first).
                state.notebook.get_untracked().map(|nb| {
                    view! {
                        <ViewOnlyNotebook
                            notebook=nb
                            hide_fork=true
                            autorun=true
                            cell_outputs=state.cell_outputs
                        />
                    }
                })
            })
        }}

        // ── Edit / View mode toggle (fixed bottom-left) ────────────────
        <div class="ironpad-mode-toggle">
            <button
                class=move || if state.is_view_mode.get() { "ironpad-mode-toggle-segment" } else { "ironpad-mode-toggle-segment ironpad-mode-toggle-segment--active" }
                title="Edit mode"
                on:click=move |_| state.is_view_mode.set(false)
            >
                "✎"
            </button>
            <button
                class=move || if state.is_view_mode.get() { "ironpad-mode-toggle-segment ironpad-mode-toggle-segment--active" } else { "ironpad-mode-toggle-segment" }
                title="View mode"
                on:click=move |_| {
                    // Flush in-progress editor content into the model before
                    // the view branch snapshots it (PRD-0032 T-007
                    // discipline). TWO rounds: an open markdown editor
                    // commits on the first bump, but effect order within a
                    // round is unspecified, so the cell flush may read the
                    // pre-commit source; the second round re-reads it
                    // post-commit.
                    #[cfg(feature = "hydrate")]
                    {
                        state.save_generation.update(|g| *g += 1);
                        leptos::task::spawn_local(async move {
                            yield_for_cell_flush(CELL_FLUSH_YIELD_MS).await;
                            state.save_generation.update(|g| *g += 1);
                            yield_for_cell_flush(CELL_FLUSH_YIELD_MS).await;
                            state.is_view_mode.set(true);
                        });
                    }
                    #[cfg(not(feature = "hydrate"))]
                    state.is_view_mode.set(true);
                }
            >
                "◉"
            </button>
        </div>
    }
}
