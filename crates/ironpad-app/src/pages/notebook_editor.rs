use ironpad_common::{CellManifest, NotebookManifest};
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use thaw::{Button, Card, CardHeader, Spinner};

use crate::components::app_layout::LayoutContext;
use crate::components::monaco_editor::MonacoEditor;
use crate::server_fns::{add_cell, delete_cell, get_notebook, rename_cell, update_notebook};

// ── Notebook-level reactive state ───────────────────────────────────────────

/// Reactive state for the notebook editor, shared among child components.
#[derive(Clone, Copy)]
struct NotebookState {
    /// The notebook UUID string (from the URL).
    notebook_id: RwSignal<String>,
    /// The ordered list of cells in this notebook.
    cells: RwSignal<Vec<CellManifest>>,
    /// The currently selected/active cell ID.
    active_cell: RwSignal<Option<String>>,
    /// Triggers a notebook refetch when incremented.
    refresh_generation: RwSignal<u64>,
}

// ── Notebook editor page ────────────────────────────────────────────────────

/// Route component for `/notebook/{id}`.
///
/// Fetches the notebook manifest, sets up reactive state, wires up the
/// `LayoutContext` header/status bar, and renders the cell list skeleton.
#[component]
pub fn NotebookEditorPage() -> impl IntoView {
    let params = use_params_map();
    let notebook_id = move || params.read().get("id").unwrap_or_default();

    // Set up notebook-level reactive state.

    let state = NotebookState {
        notebook_id: RwSignal::new(notebook_id()),
        cells: RwSignal::new(Vec::new()),
        active_cell: RwSignal::new(None),
        refresh_generation: RwSignal::new(0),
    };
    provide_context(state);

    // Fetch notebook data, re-running when refresh_generation changes.

    let notebook_resource = Resource::new(
        move || (notebook_id(), state.refresh_generation.get()),
        |(id, _gen)| get_notebook(id),
    );

    // Wire up LayoutContext when notebook data arrives.

    let layout = expect_context::<LayoutContext>();
    layout.show_save_button.set(true);

    Effect::new(move || {
        if let Some(Ok(manifest)) = notebook_resource.get() {
            layout.notebook_title.set(Some(manifest.title.clone()));
            layout.cell_count.set(manifest.cells.len());
            layout
                .compiler_version
                .set(manifest.compiler_version.clone());
            state.notebook_id.set(manifest.id.to_string());
            state.cells.set(manifest.cells.clone());
        }
    });

    view! {
        <div class="ironpad-editor">
            <Suspense fallback=move || view! {
                <div class="ironpad-editor-loading">
                    <Spinner label="Loading notebook..." />
                </div>
            }>
                {move || Suspend::new(async move {
                    match notebook_resource.await {
                        Ok(manifest) => view! {
                            <NotebookContent manifest />
                        }.into_any(),

                        Err(e) => view! {
                            <p class="ironpad-error">
                                {format!("Failed to load notebook: {e}")}
                            </p>
                        }.into_any(),
                    }
                })}
            </Suspense>
        </div>
    }
}

// ── Notebook content ────────────────────────────────────────────────────────

/// Renders the notebook title and ordered cell list with add-cell buttons.
#[component]
fn NotebookContent(manifest: NotebookManifest) -> impl IntoView {
    let state = expect_context::<NotebookState>();
    let notebook_id_str = manifest.id.to_string();

    // ── Editable title ──────────────────────────────────────────────────

    let title = RwSignal::new(manifest.title.clone());
    let title_saving = RwSignal::new(false);

    let nb_id_for_title = notebook_id_str.clone();
    let save_title = Action::new(move |new_title: &String| {
        let nb_id = nb_id_for_title.clone();
        let new_title = new_title.clone();
        async move {
            let _ = update_notebook(nb_id, new_title).await;
        }
    });

    let on_title_blur = move |_| {
        let current = title.get_untracked();
        let layout = expect_context::<LayoutContext>();
        layout.notebook_title.set(Some(current.clone()));
        title_saving.set(true);
        save_title.dispatch(current);
        title_saving.set(false);
    };

    // ── Add cell action ─────────────────────────────────────────────────

    let nb_id_for_add = notebook_id_str.clone();
    let add_cell_action = Action::new(move |after_id: &Option<String>| {
        let nb_id = nb_id_for_add.clone();
        let after = after_id.clone();
        async move { add_cell(nb_id, after).await }
    });

    // Refresh notebook when a cell is added.
    Effect::new(move || {
        if let Some(Ok(_)) = add_cell_action.value().get() {
            state.refresh_generation.update(|g| *g += 1);
        }
    });

    // ── Render ──────────────────────────────────────────────────────────

    view! {
        <div class="ironpad-editor-title-row">
            <input
                class="ironpad-editor-title-input"
                type="text"
                prop:value=move || title.get()
                on:input=move |ev| {
                    let val = event_target_value(&ev);
                    title.set(val);
                }
                on:blur=on_title_blur
            />
        </div>

        <div class="ironpad-cell-list">
            <AddCellButton after_cell_id=None add_action=add_cell_action />

            <For
                each=move || state.cells.get()
                key=|cell| cell.id.clone()
                let:cell
            >
                <CellItem cell=cell.clone() />
                <AddCellButton after_cell_id=Some(cell.id.clone()) add_action=add_cell_action />
            </For>
        </div>
    }
}

// ── Cell item ───────────────────────────────────────────────────────────────

/// A single cell card in the notebook editor (skeleton).
///
/// Displays the cell label, order number, and provides a delete button.
/// Clicking the cell selects it as the active cell.
#[component]
fn CellItem(cell: CellManifest) -> impl IntoView {
    let state = expect_context::<NotebookState>();
    let cell_id = cell.id.clone();
    let cell_id_for_click = cell.id.clone();
    let cell_id_for_delete = cell.id.clone();

    let is_active = move || state.active_cell.get().as_deref() == Some(cell_id.as_str());

    let on_click = move |_| {
        state.active_cell.set(Some(cell_id_for_click.clone()));
    };

    // ── Delete action ───────────────────────────────────────────────────

    let nb_id = state.notebook_id.get_untracked();
    let delete_action = Action::new(move |_: &()| {
        let nb_id = nb_id.clone();
        let cid = cell_id_for_delete.clone();
        async move { delete_cell(nb_id, cid).await }
    });

    Effect::new(move || {
        if let Some(Ok(())) = delete_action.value().get() {
            state.refresh_generation.update(|g| *g += 1);
        }
    });

    // ── Rename action ───────────────────────────────────────────────────

    let label = RwSignal::new(cell.label.clone());
    let cell_id_for_rename = cell.id.clone();
    let nb_id_for_rename = state.notebook_id.get_untracked();
    let rename_action = Action::new(move |new_label: &String| {
        let nb_id = nb_id_for_rename.clone();
        let cid = cell_id_for_rename.clone();
        let lbl = new_label.clone();
        async move { rename_cell(nb_id, cid, lbl).await }
    });

    let on_label_blur = move |_| {
        let current = label.get_untracked();
        rename_action.dispatch(current);
    };

    let cell_class = Signal::derive(move || {
        if is_active() {
            "ironpad-cell-card ironpad-cell-card--active".to_string()
        } else {
            "ironpad-cell-card".to_string()
        }
    });

    view! {
        <Card
            class=cell_class
            on:click=on_click
        >
            <CardHeader>
                <div class="ironpad-cell-header">
                    <span class="ironpad-cell-order">
                        {format!("[{}]", cell.order)}
                    </span>
                    <input
                        class="ironpad-cell-label-input"
                        type="text"
                        prop:value=move || label.get()
                        on:input=move |ev| {
                            let val = event_target_value(&ev);
                            label.set(val);
                        }
                        on:blur=on_label_blur
                        on:click=move |ev| {
                            ev.stop_propagation();
                        }
                    />
                    <div class="ironpad-cell-actions">
                        <Button
                            on_click=move |ev: leptos::ev::MouseEvent| {
                                ev.stop_propagation();
                                delete_action.dispatch(());
                            }
                        >
                            "✕"
                        </Button>
                    </div>
                </div>
            </CardHeader>

            <MonacoEditor
                initial_value=""
                language="rust"
            />
        </Card>
    }
}

// ── Add cell button ─────────────────────────────────────────────────────────

/// "Add Cell" button, rendered between cells and at the end of the list.
#[component]
fn AddCellButton(
    after_cell_id: Option<String>,
    add_action: Action<Option<String>, Result<CellManifest, ServerFnError>>,
) -> impl IntoView {
    let after = after_cell_id.clone();
    let on_add = move |_| {
        add_action.dispatch(after.clone());
    };

    view! {
        <div class="ironpad-add-cell-row">
            <button class="ironpad-add-cell-btn" on:click=on_add>
                "+ Add Cell"
            </button>
        </div>
    }
}
