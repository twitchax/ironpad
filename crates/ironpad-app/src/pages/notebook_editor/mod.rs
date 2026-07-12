mod cell_item;
mod cell_output;
mod export;
mod shared_deps;
mod shared_editor_panel;
mod shared_source;
mod skeleton;
pub(crate) mod state;

use std::collections::HashMap;

use ironpad_common::CellType;
use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use leptos_router::NavigateOptions;
use thaw::{Toast, ToastBody, ToastTitle, ToasterInjection};

use crate::components::app_layout::LayoutContext;
use crate::model::NotebookModel;
use crate::server_fns::share_notebook;
use crate::session::SessionState;

use crate::components::session_panel::SessionButton;

use self::cell_item::CellItem;
use self::shared_deps::SharedDepsPanel;

/// Delay (ms) before resetting the save status indicator back to idle.
const SAVE_STATUS_RESET_MS: i32 = 2_000;
use self::shared_source::SharedSourcePanel;
use self::skeleton::{AddCellButton, NotebookEditorSkeleton};
use self::state::{persist_notebook, NotebookState};

// ── Flush-before-serialize helper (PRD-0032 T-007) ──────────────────────────
//
// Share, Export HTML, and Download .ironpad all serialize the notebook from
// `state.notebook`. Each cell's live editor content only reaches that signal
// on a 1s debounce or when `state.save_generation` bumps (see the flush
// effect in `cell_item.rs`). Bumping the generation alone isn't enough,
// though: Leptos effects run queued, not synchronously on `signal.update()`,
// so callers must yield before re-reading the notebook.

/// Delay (ms) after bumping `state.save_generation` before re-reading the
/// notebook, giving the per-cell flush effects time to run. Used by Share
/// and Export HTML, which only need the in-memory model flushed.
#[cfg(feature = "hydrate")]
const CELL_FLUSH_YIELD_MS: i32 = 120;

/// Delay (ms) used by Download .ironpad, which bumps `layout.save_generation`
/// instead (flushing cells AND persisting to `IndexedDB` via
/// `persist_notebook`), so it needs extra time for the `IndexedDB` write.
#[cfg(feature = "hydrate")]
const CELL_FLUSH_PERSIST_YIELD_MS: i32 = 200;

/// Awaits a `setTimeout` so queued Leptos effects — in particular each
/// cell's notebook-level save-flush effect (`cell_item.rs`) — get a chance
/// to run before the caller re-reads `state.notebook`. There's no
/// async-timer dependency in this workspace, so this wraps `setTimeout` in a
/// `Promise` (mirrors the `set_timeout_with_callback_and_timeout_and_arguments_0`
/// usage elsewhere in this file, e.g. the save-status reset above). The
/// delays are a pragmatic yield for the effect queue, not a correctness
/// guarantee — see `CELL_FLUSH_YIELD_MS`/`CELL_FLUSH_PERSIST_YIELD_MS` docs.
#[cfg(feature = "hydrate")]
async fn yield_for_cell_flush(ms: i32) {
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

/// Serialize the current notebook, upload it via `share_notebook`, copy the
/// resulting `/shared/{hash}` URL to the clipboard, and surface the outcome as a
/// toast. The caller must bump `save_generation` first to flush in-progress cell
/// edits into the model; this yields once so those flush effects run before the
/// notebook is re-read.
fn share_current_notebook(state: &NotebookState, toaster: ToasterInjection) {
    let state = *state;
    leptos::task::spawn_local(async move {
        #[cfg(feature = "hydrate")]
        yield_for_cell_flush(CELL_FLUSH_YIELD_MS).await;

        let Some(nb) = state.notebook.get_untracked() else {
            return;
        };
        let json = match serde_json::to_string(&nb) {
            Ok(j) => j,
            Err(e) => {
                toaster.dispatch_toast(
                    move || {
                        view! {
                            <Toast>
                                <ToastTitle>"Share Failed"</ToastTitle>
                                <ToastBody>{format!("Failed to serialize: {e}")}</ToastBody>
                            </Toast>
                        }
                    },
                    thaw::ToastOptions::default(),
                );
                return;
            }
        };

        match share_notebook(json).await {
            Ok(hash) => {
                #[cfg(target_arch = "wasm32")]
                let origin = web_sys::window()
                    .and_then(|w| w.location().origin().ok())
                    .unwrap_or_default();
                #[cfg(not(target_arch = "wasm32"))]
                let origin = String::new();
                let url = format!("{origin}/shared/{hash}");
                #[cfg(target_arch = "wasm32")]
                if let Some(window) = web_sys::window() {
                    let clipboard = window.navigator().clipboard();
                    let _ = wasm_bindgen_futures::JsFuture::from(clipboard.write_text(&url)).await;
                }
                let url_clone = url.clone();
                toaster.dispatch_toast(
                    move || {
                        view! {
                            <Toast>
                                <ToastTitle>"Link Copied!"</ToastTitle>
                                <ToastBody>{url_clone.clone()}</ToastBody>
                            </Toast>
                        }
                    },
                    thaw::ToastOptions::default()
                        .with_intent(thaw::ToastIntent::Success)
                        .with_timeout(std::time::Duration::from_secs(5)),
                );
            }
            Err(e) => {
                toaster.dispatch_toast(
                    move || {
                        view! {
                            <Toast>
                                <ToastTitle>"Share Failed"</ToastTitle>
                                <ToastBody>{format!("{e}")}</ToastBody>
                            </Toast>
                        }
                    },
                    thaw::ToastOptions::default(),
                );
            }
        }
    });
}

// ── Notebook editor page ────────────────────────────────────────────────────

/// Route component for `/notebook/{id}`.
///
/// Fetches the notebook manifest, sets up reactive state, wires up the
/// `LayoutContext` header/status bar, and renders the cell list skeleton.
#[component]
pub fn NotebookEditorPage() -> impl IntoView {
    let params = use_params_map();
    let notebook_id = params.read_untracked().get("id").unwrap_or_default();

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
        expand_code: RwSignal::new(false),
        reactive_timer: RwSignal::new(None),
        cell_blocked_by: RwSignal::new(HashMap::new()),
        external_content_generation: RwSignal::new(0),
        #[cfg(feature = "hydrate")]
        reactive_timer_fn: StoredValue::new_local(None),
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

    // Load notebook from IndexedDB on the client side.

    #[cfg(feature = "hydrate")]
    {
        let nb_id = notebook_id;
        leptos::task::spawn_local(async move {
            if let Some(nb) = crate::storage::client::get_notebook(&nb_id).await {
                state.notebook.set(Some(nb));
                model.sync_from_notebook();
            }
        });
    }

    // Wire up LayoutContext when notebook data arrives.

    let layout = expect_context::<LayoutContext>();
    layout.show_save_button.set(false);

    Effect::new(move || {
        if let Some(nb) = state.notebook.get() {
            layout.notebook_title.set(Some(nb.title.clone()));
            layout.notebook_id.set(Some(nb.id.to_string()));
            layout.cell_count.set(nb.cells.len());
            state.notebook_id.set(nb.id.to_string());
            state.shared_cargo_toml.set(nb.shared_cargo_toml.clone());
            state.shared_source.set(nb.shared_source.clone());
            state.reactive_mode.set(nb.reactive_mode.unwrap_or(false));
            state.expand_code.set(nb.expand_code.unwrap_or(false));
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
                        .filter(|c| c.cell_type == CellType::Code && !c.shared)
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
    // When the save button (or Ctrl+S) fires, propagate to cells,
    // persist the notebook to IndexedDB, and show feedback.

    #[cfg(feature = "hydrate")]
    {
        use crate::components::app_layout::SaveStatus;
        use std::time::Duration;
        use thaw::{ToastIntent, ToastOptions};
        use wasm_bindgen::prelude::*;

        let toaster = ToasterInjection::expect_context();
        let prev_gen = RwSignal::new(layout.save_generation.get_untracked());

        Effect::new(move || {
            let gen = layout.save_generation.get();
            if gen == prev_gen.get_untracked() {
                return;
            }
            prev_gen.set(gen);

            // Signal all cells to flush their pending content.
            state.save_generation.update(|g| *g += 1);

            layout.save_status.set(SaveStatus::Saving);

            // Update title from layout into the notebook signal.
            let title = layout.notebook_title.get_untracked().unwrap_or_default();
            let _ = model.apply(
                ironpad_common::protocol::Mutation::NotebookUpdateMeta {
                    title: Some(title),
                    shared_cargo_toml: None,
                    shared_source: None,
                    reactive_mode: None,
                    expand_code: None,
                },
                ironpad_common::protocol::ClientId::browser(),
            );

            // Persist to IndexedDB.
            persist_notebook(&state);

            layout.save_status.set(SaveStatus::Saved);
            layout.last_save_time.set(Some(js_sys::Date::now()));

            let toaster = toaster;
            toaster.dispatch_toast(
                move || {
                    view! {
                        <Toast>
                            <ToastTitle>"Notebook saved"</ToastTitle>
                            <ToastBody>"All changes have been saved."</ToastBody>
                        </Toast>
                    }
                },
                ToastOptions::default()
                    .with_intent(ToastIntent::Success)
                    .with_timeout(Duration::from_secs(3)),
            );

            // Reset to Idle after 2 seconds. One-shot: `once_into_js` frees the
            // Rust closure when the timer fires. It must NOT live in this
            // effect's scope (a second save within the window would re-run the
            // effect, dispose this run's scope, and drop a still-pending
            // closure — "closure invoked after being dropped").
            let reset_cb = Closure::once_into_js(move || {
                layout.save_status.set(SaveStatus::Idle);
            });
            let _ = web_sys::window()
                .unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_0(
                    reset_cb.unchecked_ref(),
                    SAVE_STATUS_RESET_MS,
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
    // Only the Download .ironpad handler needs this (to flush + persist via
    // the layout save watcher); avoid an unused-variable warning on ssr-only
    // builds where that handler's body is compiled out entirely.
    #[cfg(feature = "hydrate")]
    let layout = expect_context::<LayoutContext>();

    // ── Shared deps panel toggle ────────────────────────────────────────

    let shared_deps_open = RwSignal::new(false);
    let shared_source_open = RwSignal::new(false);

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

    // ── Cells container ref for SortableJS ──────────────────────────────

    let cells_container_ref = NodeRef::new();

    // ── Close button navigation ─────────────────────────────────────────

    let navigate = use_navigate();
    let navigate_close = navigate.clone();

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

            let Some(el) = cells_ref.get() else { return };
            let el: JsValue = JsValue::from(el);

            // Only init in edit mode.
            if state.is_view_mode.get_untracked() {
                return;
            }

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

    // ── Auto-run all Code cells when entering view mode ────────────────

    Effect::new(move || {
        if state.is_view_mode.get() {
            let cell_ids: Vec<String> = state
                .cells
                .get_untracked()
                .iter()
                .filter(|c| c.cell_type == CellType::Code && !c.shared)
                .map(|c| c.id.clone())
                .collect();
            if !cell_ids.is_empty() {
                state.run_all_queue.set(cell_ids);
            }
        }
    });

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
        <div class="ironpad-notebook-toolbar">
            // ── Run All button ──────────────────────────────────────────
            <button
                class="ironpad-run-all-button"
                title="Run all code cells (Ctrl+Shift+Enter)"
                on:click=move |_| {
                    let cell_ids: Vec<String> = state
                        .cells
                        .get_untracked()
                        .iter()
                        .filter(|c| c.cell_type == CellType::Code && !c.shared)
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
                        let navigate = navigate.clone();
                        let _ = &navigate; // Used inside #[cfg(feature = "hydrate")] below.
                        if hamburger_open.get() {
                            view! {
                                <div class="ironpad-toolbar-dropdown-menu">
                                    // Share
                                    <button
                                        class="ironpad-toolbar-dropdown-item"
                                        on:click=move |_| {
                                            hamburger_open.set(false);
                                            // Flush cells' in-progress editor content into the
                                            // model before serializing, so "type then
                                            // immediately Share" doesn't produce a stale
                                            // artifact (PRD-0032 T-007). share_current_notebook
                                            // yields once so those flush effects run before it
                                            // re-reads the notebook.
                                            state.save_generation.update(|g| *g += 1);
                                            share_current_notebook(
                                                &state,
                                                expect_context::<ToasterInjection>(),
                                            );
                                        }
                                    >
                                        "↗ Share"
                                    </button>
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
                                                    let nb = state.notebook.get_untracked();
                                                    if let Some(nb) = nb {
                                                        let display_texts =
                                                            state.cell_display_texts.get_untracked();
                                                        let html =
                                                            export::build_export_html(&nb, &display_texts);
                                                        export::trigger_html_download(&html, &nb.title);
                                                    }
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
                                            {
                                                let nb_id = state.notebook_id.get_untracked();
                                                let title = state.notebook.with_untracked(|nb| {
                                                    nb.as_ref().map_or_else(
                                                        || "notebook".to_string(),
                                                        |n| n.title.clone(),
                                                    )
                                                });
                                                // Download reads from IndexedDB
                                                // (export_notebook), so the flush must reach
                                                // persistence, not just the in-memory model.
                                                // Bumping layout.save_generation runs the same
                                                // watcher the Save button uses: it flushes
                                                // cells into the model AND calls
                                                // persist_notebook (PRD-0032 T-007).
                                                layout.save_generation.update(|g| *g += 1);
                                                leptos::task::spawn_local(async move {
                                                    yield_for_cell_flush(
                                                        CELL_FLUSH_PERSIST_YIELD_MS,
                                                    )
                                                    .await;
                                                    if let Some(json) =
                                                        crate::storage::client::export_notebook(
                                                            &nb_id,
                                                        )
                                                        .await
                                                    {
                                                        export::trigger_ironpad_download(
                                                            &json, &title,
                                                        );
                                                    }
                                                });
                                            }
                                        }
                                    >
                                        "↓ Download .ironpad"
                                    </button>
                                    // Delete
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
                                                    let navigate = navigate.clone();
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
                                </div>
                            }
                                .into_any()
                        } else {
                            view! { <div style="display:none" /> }.into_any()
                        }
                    }}
                </div>

                // ── Gear dropdown (⚙) ───────────────────────────────────
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
                                            gear_open.set(false);
                                            shared_deps_open.update(|v| *v = !*v);
                                        }
                                    >
                                        {move || {
                                            if shared_deps_open.get() {
                                                "⬡ Hide Shared Deps"
                                            } else {
                                                "⬡ Shared Deps"
                                            }
                                        }}
                                    </button>
                                    <button
                                        class="ironpad-toolbar-dropdown-item"
                                        on:click=move |_| {
                                            gear_open.set(false);
                                            shared_source_open.update(|v| *v = !*v);
                                        }
                                    >
                                        {move || {
                                            if shared_source_open.get() {
                                                "✎ Hide Shared Source"
                                            } else {
                                                "✎ Shared Source"
                                            }
                                        }}
                                    </button>
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
                                                        title: None,
                                                        shared_cargo_toml: None,
                                                        shared_source: None,
                                                        reactive_mode: Some(on),
                                                        expand_code: None,
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
                                                "⚡ Reactive Mode (On)"
                                            } else {
                                                "⚡ Reactive Mode"
                                            }
                                        }}
                                    </button>
                                    <button
                                        class="ironpad-toolbar-dropdown-item"
                                        on:click=move |_| {
                                            let on = !state.expand_code.get_untracked();
                                            state.expand_code.set(on);
                                            if model
                                                .apply(
                                                    ironpad_common::protocol::Mutation::NotebookUpdateMeta {
                                                        title: None,
                                                        shared_cargo_toml: None,
                                                        shared_source: None,
                                                        reactive_mode: None,
                                                        expand_code: Some(on),
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
                                            if state.expand_code.get() {
                                                "▤ Expand Code in View (On)"
                                            } else {
                                                "▤ Expand Code in View"
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

                // ── Close button (✕) ────────────────────────────────────
                <button
                    class="ironpad-toolbar-close"
                    title="Back to notebook list"
                    on:click=move |_| {
                        let navigate_close = navigate_close.clone();
                        navigate_close("/", NavigateOptions::default());
                    }
                >
                    "✕"
                </button>
            </div>
        </div>

        {move || {
            if shared_deps_open.get() {
                view! { <SharedDepsPanel /> }.into_any()
            } else {
                view! { <div /> }.into_any()
            }
        }}

        {move || {
            if shared_source_open.get() {
                view! { <SharedSourcePanel /> }.into_any()
            } else {
                view! { <div /> }.into_any()
            }
        }}

        <div class="ironpad-cell-list ironpad-cells-container" node_ref=cells_container_ref>
            <Show when=move || !state.is_view_mode.get()>
                <AddCellButton after_cell_id=None on_add=add_cell_cb />
            </Show>

            <Show when=move || state.cells.get().is_empty() && !state.is_view_mode.get()>
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
                <Show when=move || !state.is_view_mode.get()>
                    <AddCellButton after_cell_id=Some(cell.id.clone()) on_add=add_cell_cb />
                </Show>
            </For>
        </div>

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
                on:click=move |_| state.is_view_mode.set(true)
            >
                "◉"
            </button>
        </div>
    }
}
