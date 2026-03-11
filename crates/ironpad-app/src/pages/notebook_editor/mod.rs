mod cell_item;
mod cell_output;
mod export;
mod shared_deps;
mod shared_source;
mod skeleton;
mod state;

use std::collections::HashMap;

use ironpad_common::CellType;
use leptos::prelude::*;
use leptos_router::hooks::{use_navigate, use_params_map};
use leptos_router::NavigateOptions;
use thaw::{Toast, ToastBody, ToastTitle, ToasterInjection};

use crate::components::app_layout::LayoutContext;
use crate::server_fns::share_notebook;

use self::cell_item::CellItem;
use self::shared_deps::SharedDepsPanel;
use self::shared_source::SharedSourcePanel;
use self::skeleton::{AddCellButton, NotebookEditorSkeleton};
use self::state::{
    add_cell_to_notebook, persist_notebook, sync_cells_from_notebook, NotebookState,
};

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
        refresh_generation: RwSignal::new(0),
        pending_focus_cell: RwSignal::new(None),
        cell_outputs: RwSignal::new(HashMap::new()),
        save_generation: RwSignal::new(0),
        run_all_queue: RwSignal::new(Vec::new()),
        shared_cargo_toml: RwSignal::new(None),
        shared_source: RwSignal::new(None),
        cell_stale: RwSignal::new(HashMap::new()),
        cell_display_texts: RwSignal::new(HashMap::new()),
        editor_handles: RwSignal::new(HashMap::new()),
        is_view_mode: RwSignal::new(false),
    };
    provide_context(state);

    // Load notebook from IndexedDB on the client side.

    #[cfg(feature = "hydrate")]
    {
        let nb_id = notebook_id;
        leptos::task::spawn_local(async move {
            if let Some(nb) = crate::storage::client::get_notebook(&nb_id).await {
                state.notebook.set(Some(nb));
                sync_cells_from_notebook(&state);
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
                        .filter(|c| c.cell_type == CellType::Code)
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
                    add_cell_to_notebook(&state, after_cell_id, CellType::Code);
                }
            });

        web_sys::window()
            .unwrap()
            .document()
            .unwrap()
            .add_event_listener_with_callback("keydown", closure.as_ref().unchecked_ref())
            .unwrap();

        closure.forget();
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
            state.notebook.update(|nb_opt| {
                if let Some(nb) = nb_opt {
                    nb.title = title;
                }
            });

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

            // Reset to Idle after 2 seconds.
            let reset_closure = Closure::<dyn Fn()>::new(move || {
                layout.save_status.set(SaveStatus::Idle);
            });
            let _ = web_sys::window()
                .unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_0(
                    reset_closure.as_ref().unchecked_ref(),
                    2_000,
                );
            reset_closure.forget();
        });
    }

    view! {
        <div class="ironpad-editor">
            {move || {
                if state.notebook.get().is_some() {
                    view! { <NotebookContent /> }.into_any()
                } else {
                    view! { <NotebookEditorSkeleton /> }.into_any()
                }
            }}
        </div>
    }
}

// ── Notebook content ────────────────────────────────────────────────────────

/// Renders the ordered cell list with add-cell buttons.
#[component]
fn NotebookContent() -> impl IntoView {
    let state = expect_context::<NotebookState>();

    // ── Shared deps panel toggle ────────────────────────────────────────

    let shared_deps_open = RwSignal::new(false);
    let shared_source_open = RwSignal::new(false);

    // ── Add cell callback ───────────────────────────────────────────────

    let add_cell_cb = Callback::new(move |(after, cell_type): (Option<String>, CellType)| {
        add_cell_to_notebook(&state, after, cell_type);
    });

    // ── Dropdown state ─────────────────────────────────────────────────

    let hamburger_open = RwSignal::new(false);
    let gear_open = RwSignal::new(false);

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
        click_closure.forget();
    }

    // ── Auto-run all Code cells when entering view mode ────────────────

    Effect::new(move || {
        if state.is_view_mode.get() {
            let cell_ids: Vec<String> = state
                .cells
                .get_untracked()
                .iter()
                .filter(|c| c.cell_type == CellType::Code)
                .map(|c| c.id.clone())
                .collect();
            if !cell_ids.is_empty() {
                state.run_all_queue.set(cell_ids);
            }
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
                        .filter(|c| c.cell_type == CellType::Code)
                        .map(|c| c.id.clone())
                        .collect();
                    if !cell_ids.is_empty() {
                        state.run_all_queue.set(cell_ids);
                    }
                }
            >
                "▶▶ Run All"
            </button>

            <div class="ironpad-toolbar-right">
                // ── Hamburger dropdown (☰) ──────────────────────────────
                <div class="ironpad-toolbar-dropdown">
                    <button
                        class="ironpad-toolbar-dropdown-toggle"
                        on:click=move |_| {
                            gear_open.set(false);
                            hamburger_open.update(|v| *v = !*v);
                        }
                    >
                        "☰"
                    </button>
                    {move || {
                        let navigate = navigate.clone();
                        if hamburger_open.get() {
                            view! {
                                <div class="ironpad-toolbar-dropdown-menu">
                                    // Share
                                    <button
                                        class="ironpad-toolbar-dropdown-item"
                                        on:click=move |_| {
                                            hamburger_open.set(false);
                                            let notebook = state.notebook.get_untracked();
                                            if let Some(nb) = notebook {
                                                let toaster =
                                                    expect_context::<ToasterInjection>();
                                                leptos::task::spawn_local(async move {
                                                    let json =
                                                        match serde_json::to_string(&nb) {
                                                            Ok(j) => j,
                                                            Err(e) => {
                                                                toaster.dispatch_toast(
                                                                    move || {
                                                                        view! {
                                                                            <Toast>
                                                                                <ToastTitle>"Share Failed"</ToastTitle>
                                                                                <ToastBody>
                                                                                    {format!(
                                                                                        "Failed to serialize: {e}"
                                                                                    )}
                                                                                </ToastBody>
                                                                            </Toast>
                                                                        }
                                                                    },
                                                                    Default::default(),
                                                                );
                                                                return;
                                                            }
                                                        };
                                                    match share_notebook(json).await {
                                                        Ok(hash) => {
                                                            #[cfg(target_arch = "wasm32")]
                                                            let origin = web_sys::window()
                                                                .and_then(|w| {
                                                                    w.location().origin().ok()
                                                                })
                                                                .unwrap_or_default();
                                                            #[cfg(not(target_arch = "wasm32"))]
                                                            let origin = String::new();
                                                            let url = format!(
                                                                "{origin}/shared/{hash}"
                                                            );
                                                            #[cfg(target_arch = "wasm32")]
                                                            if let Some(window) =
                                                                web_sys::window()
                                                            {
                                                                let clipboard = window
                                                                    .navigator()
                                                                    .clipboard();
                                                                let _ = wasm_bindgen_futures::JsFuture::from(
                                                                    clipboard.write_text(&url),
                                                                )
                                                                .await;
                                                            }
                                                            let url_clone = url.clone();
                                                            toaster.dispatch_toast(
                                                                move || {
                                                                    view! {
                                                                        <Toast>
                                                                            <ToastTitle>"Link Copied!"</ToastTitle>
                                                                            <ToastBody>
                                                                                {url_clone.clone()}
                                                                            </ToastBody>
                                                                        </Toast>
                                                                    }
                                                                },
                                                                thaw::ToastOptions::default()
                                                                    .with_intent(
                                                                        thaw::ToastIntent::Success,
                                                                    )
                                                                    .with_timeout(
                                                                        std::time::Duration::from_secs(
                                                                            5,
                                                                        ),
                                                                    ),
                                                            );
                                                        }
                                                        Err(e) => {
                                                            toaster.dispatch_toast(
                                                                move || {
                                                                    view! {
                                                                        <Toast>
                                                                            <ToastTitle>"Share Failed"</ToastTitle>
                                                                            <ToastBody>
                                                                                {format!("{e}")}
                                                                            </ToastBody>
                                                                        </Toast>
                                                                    }
                                                                },
                                                                Default::default(),
                                                            );
                                                        }
                                                    }
                                                });
                                            }
                                        }
                                    >
                                        "🔗 Share"
                                    </button>
                                    // Export HTML
                                    <button
                                        class="ironpad-toolbar-dropdown-item"
                                        on:click=move |_| {
                                            hamburger_open.set(false);
                                            #[cfg(feature = "hydrate")]
                                            {
                                                let nb = state.notebook.get_untracked();
                                                if let Some(nb) = nb {
                                                    let display_texts =
                                                        state.cell_display_texts.get_untracked();
                                                    let html =
                                                        export::build_export_html(&nb, &display_texts);
                                                    export::trigger_html_download(&html, &nb.title);
                                                }
                                            }
                                        }
                                    >
                                        "📄 Export HTML"
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
                                        "🗑 Delete"
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
                                                "📦 Hide Shared Deps"
                                            } else {
                                                "📦 Shared Deps"
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
                                                "🔧 Hide Shared Source"
                                            } else {
                                                "🔧 Shared Source"
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

        <div class="ironpad-cell-list">
            <Show when=move || !state.is_view_mode.get()>
                <AddCellButton after_cell_id=None on_add=add_cell_cb />
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
                "✏️"
            </button>
            <button
                class=move || if state.is_view_mode.get() { "ironpad-mode-toggle-segment ironpad-mode-toggle-segment--active" } else { "ironpad-mode-toggle-segment" }
                title="View mode"
                on:click=move |_| state.is_view_mode.set(true)
            >
                "👁"
            </button>
        </div>
    }
}
