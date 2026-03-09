use std::collections::HashMap;

use ironpad_common::{
    CellManifest, CellType, CompileRequest, CompileResponse, Diagnostic, ExecutionResult,
    IronpadCell, IronpadNotebook, Severity,
};
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use thaw::{
    Button, ButtonAppearance, Card, CardHeader, Skeleton, SkeletonItem, Tab, TabList, Tag, TagSize,
    Toast, ToastBody, ToastTitle, ToasterInjection,
};

use crate::components::app_layout::LayoutContext;
use crate::components::error_panel::ErrorPanel;
use crate::components::markdown_cell::MarkdownCell;
use crate::components::monaco_editor::{MonacoEditor, MonacoEditorHandle};
use crate::server_fns::compile_cell;

// ── Display panels ──────────────────────────────────────────────────────────

/// Display panel types matching ironpad-cell's DisplayPanel enum.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
enum DisplayPanel {
    Text(String),
    Html(String),
    Svg(String),
}

// ── Cell status ─────────────────────────────────────────────────────────────

/// Reactive cell execution status for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CellStatus {
    Idle,
    Queued,
    Compiling,
    Running,
    Success,
    Error,
}

// ── Per-cell output data ────────────────────────────────────────────────────

/// Stores the output bytes and optional type tag from a cell execution.
#[derive(Clone, Default, Debug)]
struct CellOutputData {
    bytes: Vec<u8>,
    type_tag: Option<String>,
}

// ── Notebook-level reactive state ───────────────────────────────────────────

/// Reactive state for the notebook editor, shared among child components.
#[derive(Clone, Copy)]
struct NotebookState {
    /// The full notebook loaded from IndexedDB.
    notebook: RwSignal<Option<IronpadNotebook>>,
    /// The notebook UUID string (from the URL).
    notebook_id: RwSignal<String>,
    /// The ordered list of cells in this notebook.
    cells: RwSignal<Vec<CellManifest>>,
    /// The currently selected/active cell ID.
    active_cell: RwSignal<Option<String>>,
    /// Triggers a notebook refetch when incremented (retained for future use).
    #[allow(dead_code)]
    refresh_generation: RwSignal<u64>,
    /// Cell ID that should be scrolled to and focused after creation.
    pending_focus_cell: RwSignal<Option<String>>,
    /// Per-cell output data from the last execution, keyed by cell ID.
    /// Used to pipe cell N's output as cell N+1's input.
    cell_outputs: RwSignal<HashMap<String, CellOutputData>>,
    /// Triggers all cells to immediately flush their content to the server.
    save_generation: RwSignal<u64>,
    /// Ordered queue of cell IDs for "Run All Below" sequential execution.
    /// The cell at position [0] is the one currently being executed.
    run_all_queue: RwSignal<Vec<String>>,
    /// Notebook-level shared Cargo.toml content.
    shared_cargo_toml: RwSignal<Option<String>>,
    /// Tracks which cells have stale (outdated) execution results.
    cell_stale: RwSignal<HashMap<String, bool>>,
}

// ── Notebook state helpers ──────────────────────────────────────────────────

/// Syncs the `cells` signal from the current notebook state.
fn sync_cells_from_notebook(state: &NotebookState) {
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
fn persist_notebook(state: &NotebookState) {
    #[cfg(feature = "hydrate")]
    {
        if let Some(mut nb) = state.notebook.get_untracked() {
            nb.updated_at = chrono::Utc::now();
            state
                .notebook
                .update(|existing| *existing = Some(nb.clone()));
            leptos::task::spawn_local(async move {
                crate::storage::client::save_notebook(&nb).await;
            });
        }
    }
}

/// Default Cargo.toml template for a new code cell.
fn default_cell_cargo_toml(cell_id: &str) -> String {
    format!(
        "[package]\nname = \"{cell_id}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[lib]\ncrate-type = [\"cdylib\"]\n\n[dependencies]\nironpad-cell = \"0.1\"\n"
    )
}

/// Adds a new cell to the notebook in-memory, syncs signals, and persists.
fn add_cell_to_notebook(state: &NotebookState, after_cell_id: Option<String>, cell_type: CellType) {
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
        cell_stale: RwSignal::new(HashMap::new()),
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
    layout.show_save_button.set(true);

    Effect::new(move || {
        if let Some(nb) = state.notebook.get() {
            layout.notebook_title.set(Some(nb.title.clone()));
            layout.notebook_id.set(Some(nb.id.to_string()));
            layout.cell_count.set(nb.cells.len());
            state.notebook_id.set(nb.id.to_string());
            state.shared_cargo_toml.set(nb.shared_cargo_toml.clone());
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

    // ── Add cell callback ───────────────────────────────────────────────

    let add_cell_cb = Callback::new(move |(after, cell_type): (Option<String>, CellType)| {
        add_cell_to_notebook(&state, after, cell_type);
    });

    // ── Render ──────────────────────────────────────────────────────────

    view! {
        <div class="ironpad-notebook-toolbar">
            <Button
                appearance=ButtonAppearance::Subtle
                on_click=move |_| shared_deps_open.update(|v| *v = !*v)
            >
                {move || if shared_deps_open.get() { "📦 Hide Shared Deps" } else { "📦 Shared Deps" }}
            </Button>
            <Button
                appearance=ButtonAppearance::Subtle
                on_click=move |_| {
                    let cells = state.cells.get_untracked();
                    let stale = state.cell_stale.get_untracked();
                    let stale_cells: Vec<String> = cells.iter()
                        .filter(|c| c.cell_type == CellType::Code && stale.get(&c.id).copied().unwrap_or(false))
                        .map(|c| c.id.clone())
                        .collect();
                    if !stale_cells.is_empty() {
                        state.run_all_queue.set(stale_cells);
                    }
                }
            >
                "⟳ Run Stale"
            </Button>
        </div>

        {move || {
            if shared_deps_open.get() {
                view! { <SharedDepsPanel /> }.into_any()
            } else {
                view! { <div /> }.into_any()
            }
        }}

        <div class="ironpad-cell-list">
            <AddCellButton after_cell_id=None on_add=add_cell_cb />

            <For
                each=move || state.cells.get()
                key=|cell| cell.id.clone()
                let:cell
            >
                <CellItem cell=cell.clone() />
                <AddCellButton after_cell_id=Some(cell.id.clone()) on_add=add_cell_cb />
            </For>
        </div>
    }
}

// ── Shared dependencies panel ───────────────────────────────────────────────

const SHARED_DEPS_DEFAULT: &str = "\
[dependencies]
# Add shared dependencies here.
# These will be available in all cells.
# Cell-level dependencies override shared ones.

[profile.release]
# Optimized for fast compilation (interactive notebook use).
opt-level = 1
lto = false
codegen-units = 16
";

/// Panel for editing the notebook-level shared Cargo.toml.
#[component]
fn SharedDepsPanel() -> impl IntoView {
    let state = expect_context::<NotebookState>();
    let toaster = ToasterInjection::expect_context();

    let editor_text = RwSignal::new(
        state
            .shared_cargo_toml
            .get_untracked()
            .unwrap_or_else(|| SHARED_DEPS_DEFAULT.to_string()),
    );
    let saving = RwSignal::new(false);

    let on_save = move |_| {
        let content = editor_text.get_untracked();

        // Update notebook in-memory.
        state.shared_cargo_toml.set(Some(content.clone()));
        state.notebook.update(|nb_opt| {
            if let Some(nb) = nb_opt {
                nb.shared_cargo_toml = Some(content.clone());
            }
        });

        // Mark all code cells as stale when shared deps change.
        state.cell_stale.update(|stale| {
            let cells = state.cells.get_untracked();
            for cell in &cells {
                if cell.cell_type == CellType::Code {
                    stale.insert(cell.id.clone(), true);
                }
            }
        });

        // Persist to IndexedDB.
        persist_notebook(&state);

        let toaster = toaster;
        toaster.dispatch_toast(
            move || {
                view! {
                    <Toast>
                        <ToastTitle>"Shared dependencies saved"</ToastTitle>
                        <ToastBody>"Changes will apply on next cell compile."</ToastBody>
                    </Toast>
                }
            },
            thaw::ToastOptions::default()
                .with_intent(thaw::ToastIntent::Success)
                .with_timeout(std::time::Duration::from_secs(3)),
        );
    };

    view! {
        <Card class="ironpad-shared-deps">
            <CardHeader>
                <div class="ironpad-shared-deps-header">
                    <span class="ironpad-shared-deps-title">"📦 Shared Dependencies (Cargo.toml)"</span>
                    <Button
                        appearance=ButtonAppearance::Primary
                        on_click=on_save
                        disabled=Signal::derive(move || saving.get())
                    >
                        {move || if saving.get() { "Saving…" } else { "Save" }}
                    </Button>
                </div>
            </CardHeader>
            <div class="ironpad-shared-deps-editor-wrapper">
                <MonacoEditor
                    initial_value=editor_text.get_untracked()
                    language="toml"
                    on_change=Callback::new(move |val: String| {
                        editor_text.set(val);
                    })
                />
            </div>
        </Card>
    }
}

// ── Cell item ───────────────────────────────────────────────────────────────

/// A single cell card in the notebook editor.
///
/// Features a tab bar (Code / Cargo.toml) with independent Monaco editors,
/// a header with order badge, editable label, status placeholder, run button,
/// and menu button. The cell body is collapsible.
#[component]
fn CellItem(cell: CellManifest) -> impl IntoView {
    let state = expect_context::<NotebookState>();
    let cell_id = cell.id.clone();
    let cell_id_for_click = cell.id.clone();
    let cell_id_for_delete = cell.id.clone();
    let cell_id_for_delete_cleanup = cell.id.clone();
    let cell_id_for_focus = cell.id.clone();
    let cell_id_for_flush = cell.id.clone();
    let cell_id_for_stale_src = cell.id.clone();
    let cell_id_for_stale_toml = cell.id.clone();
    let cell_id_for_stale_header = cell.id.clone();
    let cell_id_for_markdown = StoredValue::new(cell.id.clone());

    let is_markdown = cell.cell_type == CellType::Markdown;

    let is_active = move || state.active_cell.get().as_deref() == Some(cell_id.as_str());

    let on_click = move |_| {
        state.active_cell.set(Some(cell_id_for_click.clone()));
    };

    // ── Cell status & compile result ────────────────────────────────────

    let cell_status = RwSignal::new(CellStatus::Idle);
    let last_compile: RwSignal<Option<CompileResponse>> = RwSignal::new(None);
    let compile_time_ms: RwSignal<Option<f64>> = RwSignal::new(None);
    let execution_result: RwSignal<Option<ExecutionResult>> = RwSignal::new(None);

    // ── Collapse state ──────────────────────────────────────────────────

    let collapsed = RwSignal::new(false);

    // ── Tab state ───────────────────────────────────────────────────────

    let selected_tab = RwSignal::new("code".to_string());

    // ── Editor handles (for compile flow) ───────────────────────────────

    let source_handle: RwSignal<Option<MonacoEditorHandle>> = RwSignal::new(None);
    let cargo_toml_handle: RwSignal<Option<MonacoEditorHandle>> = RwSignal::new(None);

    // ── Reactive source / cargo_toml state ──────────────────────────────

    let initial_content = state.notebook.with_untracked(|nb_opt| {
        nb_opt.as_ref().and_then(|nb| {
            nb.cells
                .iter()
                .find(|c| c.id == cell.id)
                .map(|c| (c.source.clone(), c.cargo_toml.clone().unwrap_or_default()))
        })
    });
    let source = RwSignal::new(
        initial_content
            .as_ref()
            .map(|c| c.0.clone())
            .unwrap_or_default(),
    );
    let cargo_toml = RwSignal::new(
        initial_content
            .as_ref()
            .map(|c| c.1.clone())
            .unwrap_or_default(),
    );

    // ── Dirty state (unsaved changes indicator) ─────────────────────────

    let source_dirty = RwSignal::new(false);
    let cargo_toml_dirty = RwSignal::new(false);

    // ── Delete action ───────────────────────────────────────────────────

    let cell_id_for_delete_sv = StoredValue::new(cell_id_for_delete);
    let cell_id_for_delete_cleanup_sv = StoredValue::new(cell_id_for_delete_cleanup);

    let delete_cell_fn = move || {
        let cid = cell_id_for_delete_sv.get_value();
        let cid_cleanup = cell_id_for_delete_cleanup_sv.get_value();
        state.notebook.update(|nb_opt| {
            if let Some(nb) = nb_opt {
                nb.cells.retain(|c| c.id != cid);
                for (i, cell) in nb.cells.iter_mut().enumerate() {
                    cell.order = i as u32;
                }
            }
        });
        state.cell_outputs.update(|map| {
            map.remove(&cid_cleanup);
        });
        sync_cells_from_notebook(&state);
        persist_notebook(&state);
    };

    // ── Rename action ───────────────────────────────────────────────────

    let label = RwSignal::new(cell.label.clone());
    let cell_id_for_rename = cell.id.clone();

    let on_label_blur = move |_| {
        let current = label.get_untracked();
        let cid = cell_id_for_rename.clone();
        state.notebook.update(|nb_opt| {
            if let Some(nb) = nb_opt {
                if let Some(cell) = nb.cells.iter_mut().find(|c| c.id == cid) {
                    cell.label = current;
                }
            }
        });
        sync_cells_from_notebook(&state);
        persist_notebook(&state);
    };

    // ── Menu state ──────────────────────────────────────────────────────

    let menu_open = RwSignal::new(false);

    // ── Move (reorder) action ───────────────────────────────────────────

    let cell_id_for_move = StoredValue::new(cell.id.clone());

    let reorder_cells_fn = move |new_ids: Vec<String>| {
        state.notebook.update(|nb_opt| {
            let Some(nb) = nb_opt else { return };
            let mut reordered = Vec::with_capacity(new_ids.len());
            for id in &new_ids {
                if let Some(pos) = nb.cells.iter().position(|c| &c.id == id) {
                    reordered.push(nb.cells.remove(pos));
                }
            }
            // Append any cells not in new_ids (shouldn't happen, but safe).
            reordered.append(&mut nb.cells);
            for (i, cell) in reordered.iter_mut().enumerate() {
                cell.order = i as u32;
            }
            nb.cells = reordered;
        });
        sync_cells_from_notebook(&state);
        persist_notebook(&state);
    };

    let reorder_for_up = reorder_cells_fn;
    let on_move_up = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        menu_open.set(false);
        let cid = cell_id_for_move.get_value();
        let cells = state.cells.get_untracked();
        let Some(my_idx) = cells.iter().position(|c| c.id == cid) else {
            return;
        };
        if my_idx == 0 {
            return;
        }
        let mut ids: Vec<String> = cells.iter().map(|c| c.id.clone()).collect();
        ids.swap(my_idx, my_idx - 1);
        reorder_for_up(ids);
    };

    let reorder_for_down = reorder_cells_fn;
    let on_move_down = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        menu_open.set(false);
        let cid = cell_id_for_move.get_value();
        let cells = state.cells.get_untracked();
        let Some(my_idx) = cells.iter().position(|c| c.id == cid) else {
            return;
        };
        if my_idx + 1 >= cells.len() {
            return;
        }
        let mut ids: Vec<String> = cells.iter().map(|c| c.id.clone()).collect();
        ids.swap(my_idx, my_idx + 1);
        reorder_for_down(ids);
    };

    // ── Duplicate action ────────────────────────────────────────────────

    let cell_id_for_dup = StoredValue::new(cell.id.clone());

    let on_duplicate = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        menu_open.set(false);

        let cid = cell_id_for_dup.get_value();
        let new_id = {
            #[cfg(feature = "hydrate")]
            {
                uuid::Uuid::new_v4().to_string()
            }
            #[cfg(not(feature = "hydrate"))]
            {
                format!("{cid}_dup")
            }
        };

        state.notebook.update(|nb_opt| {
            let Some(nb) = nb_opt else { return };
            let Some(idx) = nb.cells.iter().position(|c| c.id == cid) else {
                return;
            };
            let original = nb.cells[idx].clone();
            let new_cell = IronpadCell {
                id: new_id.clone(),
                order: 0,
                label: format!("{} (copy)", original.label),
                cell_type: original.cell_type,
                source: original.source,
                cargo_toml: original.cargo_toml,
            };
            nb.cells.insert(idx + 1, new_cell);
            for (i, cell) in nb.cells.iter_mut().enumerate() {
                cell.order = i as u32;
            }
        });

        state.pending_focus_cell.set(Some(new_id));
        sync_cells_from_notebook(&state);
        persist_notebook(&state);
    };

    // ── Delete with confirmation ────────────────────────────────────────

    let on_delete_confirmed = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        menu_open.set(false);
        #[cfg(feature = "hydrate")]
        {
            let confirmed = web_sys::window()
                .unwrap()
                .confirm_with_message("Delete this cell? This cannot be undone.")
                .unwrap_or(false);
            if confirmed {
                delete_cell_fn();
            }
        }
        #[cfg(not(feature = "hydrate"))]
        {
            delete_cell_fn();
        }
    };

    // ── Move boundary checks ────────────────────────────────────────────

    let cell_id_for_boundary = StoredValue::new(cell.id.clone());
    let is_first = Signal::derive(move || {
        let cid = cell_id_for_boundary.get_value();
        state
            .cells
            .get()
            .first()
            .map(|c| c.id == cid)
            .unwrap_or(true)
    });

    let is_last = Signal::derive(move || {
        let cid = cell_id_for_boundary.get_value();
        state
            .cells
            .get()
            .last()
            .map(|c| c.id == cid)
            .unwrap_or(true)
    });

    // ── Run cell action (compile flow) ──────────────────────────────────

    // Trigger signal: incrementing this dispatches a compile.
    let run_trigger = RwSignal::new(0u64);
    let cell_id_for_run = StoredValue::new(cell.id.clone());

    // ── Run-all queue watcher ───────────────────────────────────────────
    //
    // When this cell appears at the front of the run-all queue, trigger
    // its compile flow.  Non-front cells show a "Queued" status badge.

    let cell_id_for_queue = StoredValue::new(cell.id.clone());

    Effect::new(move || {
        let queue = state.run_all_queue.get();
        let cid = cell_id_for_queue.get_value();

        let my_pos = queue.iter().position(|id| id == &cid);

        match my_pos {
            Some(0) => {
                // Markdown cells skip compilation — advance the queue immediately.
                if is_markdown {
                    state.run_all_queue.update(|q| {
                        if q.first().map(|id| id == &cid).unwrap_or(false) {
                            q.remove(0);
                        }
                    });
                    return;
                }

                // At the front — trigger compile if not already in progress.
                if !matches!(
                    cell_status.get_untracked(),
                    CellStatus::Compiling | CellStatus::Running
                ) {
                    run_trigger.update(|g| *g += 1);
                }
            }
            Some(_) => {
                // Waiting in queue — show queued indicator.
                if !is_markdown
                    && !matches!(
                        cell_status.get_untracked(),
                        CellStatus::Compiling | CellStatus::Running | CellStatus::Queued
                    )
                {
                    cell_status.set(CellStatus::Queued);
                }
            }
            None => {
                // Not in queue — reset from Queued back to Idle.
                if cell_status.get_untracked() == CellStatus::Queued {
                    cell_status.set(CellStatus::Idle);
                }
            }
        }
    });

    // ── Run All Below trigger ───────────────────────────────────────────

    let cell_id_for_run_all = StoredValue::new(cell.id.clone());
    let on_run_all_below = move |ev: leptos::ev::MouseEvent| {
        ev.stop_propagation();
        menu_open.set(false);
        let cid = cell_id_for_run_all.get_value();
        let cells = state.cells.get_untracked();
        let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
        let queue: Vec<String> = cells[my_idx..]
            .iter()
            .filter(|c| c.cell_type == CellType::Code)
            .map(|c| c.id.clone())
            .collect();
        if !queue.is_empty() {
            state.run_all_queue.set(queue);
        }
    };

    // The actual compile flow, driven by `run_trigger`.
    Effect::new(move || {
        let gen = run_trigger.get();
        if gen == 0 {
            return;
        }

        // Markdown cells skip compilation entirely.
        if is_markdown {
            return;
        }

        // Avoid double-dispatch while already compiling or running.
        if matches!(
            cell_status.get_untracked(),
            CellStatus::Compiling | CellStatus::Running
        ) {
            return;
        }

        let cid = cell_id_for_run.get_value();
        let current_source = source.get_untracked();
        let current_cargo_toml = cargo_toml.get_untracked();

        // ── Cascading execution ─────────────────────────────────────────
        // If any predecessor Code cells have not been executed yet, auto-queue
        // them (plus this cell) so they run in order first.
        // Markdown cells are skipped — they don't need execution.
        {
            let cells = state.cells.get_untracked();
            let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
            let outputs = state.cell_outputs.get_untracked();
            let unexecuted: Vec<String> = cells[..my_idx]
                .iter()
                .filter(|c| c.cell_type == CellType::Code && !outputs.contains_key(&c.id))
                .map(|c| c.id.clone())
                .collect();

            if !unexecuted.is_empty() {
                let mut queue = unexecuted;
                queue.push(cid.clone());
                state.run_all_queue.set(queue);
                return;
            }
        }

        // Collect previous Code cell outputs for the I/O pipeline.
        // Markdown cells are skipped — they produce no output.
        let (input_bytes, previous_cell_types) = {
            let cells = state.cells.get_untracked();
            let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
            let outputs = state.cell_outputs.get_untracked();

            if my_idx == 0 {
                (vec![], vec![])
            } else {
                let prev_code_cells: Vec<&CellManifest> = cells[..my_idx]
                    .iter()
                    .filter(|c| c.cell_type == CellType::Code)
                    .collect();
                let mut all_outputs: Vec<&[u8]> = Vec::new();
                let mut types: Vec<Option<String>> = Vec::new();

                for c in &prev_code_cells {
                    if let Some(data) = outputs.get(&c.id) {
                        all_outputs.push(&data.bytes);
                        types.push(data.type_tag.clone());
                    } else {
                        all_outputs.push(&[]);
                        types.push(None);
                    }
                }

                // Serialize using CellInputs wire format (length-prefixed):
                // [u32 LE: count][u32 LE: len0][bytes0...][u32 LE: len1][bytes1...]...
                let mut buf = Vec::new();
                buf.extend_from_slice(&(all_outputs.len() as u32).to_le_bytes());
                for output in &all_outputs {
                    buf.extend_from_slice(&(output.len() as u32).to_le_bytes());
                    buf.extend_from_slice(output);
                }

                (buf, types)
            }
        };

        // Invalidate downstream Code cells' cached outputs (this cell and all after it).
        {
            let cells = state.cells.get_untracked();
            let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
            let downstream_ids: Vec<String> = cells[my_idx..]
                .iter()
                .filter(|c| c.cell_type == CellType::Code)
                .map(|c| c.id.clone())
                .collect();
            state.cell_outputs.update(|map| {
                for id in &downstream_ids {
                    map.remove(id);
                }
            });
        }

        cell_status.set(CellStatus::Compiling);
        last_compile.set(None);
        compile_time_ms.set(None);

        // Clear inline markers before each compile.
        #[cfg(feature = "hydrate")]
        {
            if let Some(handle) = source_handle.get_untracked() {
                handle.clear_markers();
            }
        }

        leptos::task::spawn_local(async move {
            #[cfg(feature = "hydrate")]
            let start = js_sys::Date::now();

            let cell_id_for_exec = cid.clone();

            let request = CompileRequest {
                notebook_id: state.notebook_id.get_untracked(),
                cell_id: cid,
                source: current_source,
                cargo_toml: current_cargo_toml,
                previous_cell_types,
                shared_cargo_toml: state.shared_cargo_toml.get_untracked(),
            };

            let result = compile_cell(request).await;

            #[cfg(feature = "hydrate")]
            compile_time_ms.set(Some(js_sys::Date::now() - start));
            #[cfg(not(feature = "hydrate"))]
            compile_time_ms.set(Some(0.0));

            match result {
                Ok(response) => {
                    let has_errors = response
                        .diagnostics
                        .iter()
                        .any(|d| d.severity == Severity::Error);

                    if !response.wasm_blob.is_empty() && !has_errors {
                        // Compilation succeeded — load and execute the WASM blob.
                        #[cfg(feature = "hydrate")]
                        {
                            use crate::components::executor;

                            cell_status.set(CellStatus::Running);

                            let blob = response.wasm_blob.clone();
                            let js_glue = response.js_glue.clone();
                            last_compile.set(Some(response));

                            let hash = executor::hash_wasm_blob(&blob);

                            let exec_err =
                                match executor::load_blob(&cell_id_for_exec, &hash, &blob, js_glue)
                                    .await
                                {
                                    Ok(()) => {
                                        let exec_start = js_sys::Date::now();
                                        match executor::execute_cell(
                                            &cell_id_for_exec,
                                            &input_bytes,
                                        )
                                        .await
                                        {
                                            Ok((output_bytes, display_text, type_tag)) => {
                                                // Store output for downstream cells.
                                                state.cell_outputs.update(|map| {
                                                    map.insert(
                                                        cell_id_for_exec.clone(),
                                                        CellOutputData {
                                                            bytes: output_bytes.clone(),
                                                            type_tag: type_tag.clone(),
                                                        },
                                                    );
                                                });

                                                execution_result.set(Some(ExecutionResult {
                                                    display_text,
                                                    output_bytes,
                                                    execution_time_ms: js_sys::Date::now()
                                                        - exec_start,
                                                    type_tag,
                                                }));
                                                cell_status.set(CellStatus::Success);

                                                // Clear stale flag on successful execution.
                                                state.cell_stale.update(|stale| {
                                                    stale.remove(&cell_id_for_exec);
                                                });

                                                // Advance run-all queue on success.
                                                state.run_all_queue.update(|q| {
                                                    if q.first()
                                                        .map(|id| id == &cell_id_for_exec)
                                                        .unwrap_or(false)
                                                    {
                                                        q.remove(0);
                                                    }
                                                });

                                                None
                                            }
                                            Err(e) => Some(format!("Execution error: {e}")),
                                        }
                                    }
                                    Err(e) => Some(format!("WASM load error: {e}")),
                                };

                            if let Some(err_msg) = exec_err {
                                execution_result.set(Some(ExecutionResult {
                                    display_text: Some(err_msg),
                                    output_bytes: vec![],
                                    execution_time_ms: 0.0,
                                    type_tag: None,
                                }));
                                cell_status.set(CellStatus::Error);

                                // Stop run-all on execution error.
                                state.run_all_queue.set(vec![]);
                            }
                        }

                        #[cfg(not(feature = "hydrate"))]
                        {
                            cell_status.set(CellStatus::Success);
                            last_compile.set(Some(response));

                            // Clear stale flag on successful execution (SSR path).
                            state.cell_stale.update(|stale| {
                                stale.remove(&cell_id_for_exec);
                            });

                            // Advance run-all queue (SSR path).
                            state.run_all_queue.update(|q| {
                                if q.first().map(|id| id == &cell_id_for_exec).unwrap_or(false) {
                                    q.remove(0);
                                }
                            });
                        }
                    } else {
                        cell_status.set(CellStatus::Error);
                        last_compile.set(Some(response));

                        // Stop run-all on compile error.
                        state.run_all_queue.set(vec![]);
                    }
                }
                Err(e) => {
                    cell_status.set(CellStatus::Error);
                    last_compile.set(Some(CompileResponse {
                        wasm_blob: vec![],
                        diagnostics: vec![Diagnostic {
                            message: format!("Server error: {e}"),
                            severity: Severity::Error,
                            spans: vec![],
                            code: None,
                        }],
                        cached: false,
                        preamble_lines: 0,
                        js_glue: None,
                    }));

                    // Stop run-all on server error.
                    state.run_all_queue.set(vec![]);
                }
            }
        });
    });

    // ── Shift+Enter keybinding registration ─────────────────────────────
    //
    // Once the source Monaco editor handle is available, register a
    // Shift+Enter action that triggers the compile flow.

    #[cfg(feature = "hydrate")]
    {
        use wasm_bindgen::prelude::*;

        Effect::new(move || {
            let Some(handle) = source_handle.get() else {
                return;
            };

            let closure = Closure::<dyn Fn()>::new(move || {
                run_trigger.update(|g| *g += 1);
            });
            let cb: js_sys::Function = closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
            closure.forget();

            // Monaco KeyMod.Shift (1024) | KeyCode.Enter (3) = 1027
            handle.add_action("ironpad.runCell", &[1027], &cb);
        });
    }

    // ── Autocomplete context ────────────────────────────────────────────
    //
    // Push cell variable context to the Monaco completion provider whenever
    // the editor handle appears or cell outputs change (types may update).

    #[cfg(feature = "hydrate")]
    {
        let cell_id_for_ctx = StoredValue::new(cell.id.clone());

        Effect::new(move || {
            let Some(handle) = source_handle.get() else {
                return;
            };

            // Re-run when cell_outputs change (new type_tags after execution).
            let outputs = state.cell_outputs.get();
            let cells = state.cells.get_untracked();
            let cid = cell_id_for_ctx.get_value();
            let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);

            let variables = js_sys::Array::new();

            // Only Code cells produce output — skip markdown cells and use
            // sequential indices so variable names match CellInputs indices.
            for (i, c) in cells[..my_idx]
                .iter()
                .filter(|c| c.cell_type == CellType::Code)
                .enumerate()
            {
                let type_str = outputs
                    .get(&c.id)
                    .and_then(|d| d.type_tag.as_deref())
                    .unwrap_or("unknown");

                let var = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &var,
                    &wasm_bindgen::JsValue::from_str("name"),
                    &wasm_bindgen::JsValue::from_str(&format!("cell{i}")),
                );
                let _ = js_sys::Reflect::set(
                    &var,
                    &wasm_bindgen::JsValue::from_str("type"),
                    &wasm_bindgen::JsValue::from_str(type_str),
                );
                let _ = js_sys::Reflect::set(
                    &var,
                    &wasm_bindgen::JsValue::from_str("doc"),
                    &wasm_bindgen::JsValue::from_str(&format!(
                        "Output of cell {} ({})",
                        c.label, type_str
                    )),
                );
                variables.push(&var);
            }

            // Add `last` alias pointing to the most recent typed Code cell.
            let has_prev_code = cells[..my_idx]
                .iter()
                .any(|c| c.cell_type == CellType::Code);
            if has_prev_code {
                // Walk backwards to find the most recent Code cell with a type_tag.
                let last_type = cells[..my_idx]
                    .iter()
                    .rev()
                    .filter(|c| c.cell_type == CellType::Code)
                    .find_map(|c| {
                        outputs
                            .get(&c.id)
                            .and_then(|d| d.type_tag.as_deref())
                            .map(|t| t.to_string())
                    })
                    .unwrap_or_else(|| "unknown".to_string());

                let var = js_sys::Object::new();
                let _ = js_sys::Reflect::set(
                    &var,
                    &wasm_bindgen::JsValue::from_str("name"),
                    &wasm_bindgen::JsValue::from_str("last"),
                );
                let _ = js_sys::Reflect::set(
                    &var,
                    &wasm_bindgen::JsValue::from_str("type"),
                    &wasm_bindgen::JsValue::from_str(&last_type),
                );
                let _ = js_sys::Reflect::set(
                    &var,
                    &wasm_bindgen::JsValue::from_str("doc"),
                    &wasm_bindgen::JsValue::from_str("Output of the most recent cell"),
                );
                variables.push(&var);
            }

            let context = js_sys::Object::new();
            let _ = js_sys::Reflect::set(
                &context,
                &wasm_bindgen::JsValue::from_str("variables"),
                &variables,
            );
            handle.set_cell_context(&wasm_bindgen::JsValue::from(context));
        });
    }

    // ── Inline error markers ────────────────────────────────────────────
    //
    // When `last_compile` changes, convert diagnostics with spans into
    // Monaco model markers so errors/warnings appear inline in the editor.

    #[cfg(feature = "hydrate")]
    {
        Effect::new(move || {
            let compile = last_compile.get();
            let Some(handle) = source_handle.get_untracked() else {
                return;
            };

            let Some(response) = compile else {
                // Compile was cleared (e.g. new compile started); markers
                // already cleared by the compile-start code above.
                return;
            };

            let markers = js_sys::Array::new();

            for diag in &response.diagnostics {
                let severity: u32 = match diag.severity {
                    Severity::Error => 8,
                    Severity::Warning => 4,
                    Severity::Note => 2,
                };

                // If spans are available, create a marker per span.
                if diag.spans.is_empty() {
                    continue;
                }

                for span in &diag.spans {
                    let marker = js_sys::Object::new();
                    let _ = js_sys::Reflect::set(
                        &marker,
                        &"startLineNumber".into(),
                        &(span.line_start).into(),
                    );
                    let _ = js_sys::Reflect::set(
                        &marker,
                        &"startColumn".into(),
                        &(span.col_start).into(),
                    );
                    let _ = js_sys::Reflect::set(
                        &marker,
                        &"endLineNumber".into(),
                        &(span.line_end).into(),
                    );
                    let _ =
                        js_sys::Reflect::set(&marker, &"endColumn".into(), &(span.col_end).into());

                    // Use span label if available, else fall back to diagnostic message.
                    let msg = span.label.as_deref().unwrap_or(&diag.message);
                    let _ = js_sys::Reflect::set(&marker, &"message".into(), &msg.into());
                    let _ = js_sys::Reflect::set(&marker, &"severity".into(), &severity.into());

                    markers.push(&marker);
                }
            }

            handle.set_markers(&markers);
        });
    }

    // ── On-change callbacks for Monaco editors ──────────────────────────
    //
    // Source editor: debounce saves via setTimeout / clearTimeout so that
    // rapid keystrokes are batched into a single server call after 1 s of
    // inactivity.  The debounce plumbing only exists in the `hydrate`
    // (client-side) build; SSR simply updates the reactive signal.

    #[cfg(feature = "hydrate")]
    let on_source_change = {
        use wasm_bindgen::prelude::*;

        let cid_save = cell.id.clone();
        let debounce_handle: RwSignal<i32> = RwSignal::new(0);

        // Build a reusable JS function that reads the *current* source from
        // the signal and persists it to IndexedDB via the notebook signal.
        let closure = Closure::<dyn Fn()>::new(move || {
            let val = source.get_untracked();
            let cid = cid_save.clone();
            state.notebook.update(|nb_opt| {
                if let Some(nb) = nb_opt {
                    if let Some(cell) = nb.cells.iter_mut().find(|c| c.id == cid) {
                        cell.source = val;
                    }
                }
            });
            persist_notebook(&state);
            source_dirty.set(false);
        });
        let save_fn: js_sys::Function =
            closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
        closure.forget();

        Callback::new(move |val: String| {
            source.set(val);
            source_dirty.set(true);

            // Mark this cell and all subsequent Code cells as stale.
            if !is_markdown {
                let cid = cell_id_for_stale_src.clone();
                state.cell_stale.update(|stale| {
                    let cells = state.cells.get_untracked();
                    let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
                    for cell in &cells[my_idx..] {
                        if cell.cell_type == CellType::Code {
                            stale.insert(cell.id.clone(), true);
                        }
                    }
                });
            }

            // Clear the previous debounce timer and start a fresh 1 s window.
            let win = web_sys::window().unwrap();
            let prev = debounce_handle.get_untracked();
            if prev != 0 {
                win.clear_timeout_with_handle(prev);
            }
            let handle = win
                .set_timeout_with_callback_and_timeout_and_arguments_0(&save_fn, 1_000)
                .unwrap();
            debounce_handle.set(handle);
        })
    };

    #[cfg(not(feature = "hydrate"))]
    let on_source_change = Callback::new(move |val: String| {
        source.set(val);
    });

    #[cfg(feature = "hydrate")]
    let on_cargo_toml_change = {
        use wasm_bindgen::prelude::*;

        let cid_save = cell.id.clone();
        let debounce_handle: RwSignal<i32> = RwSignal::new(0);

        let closure = Closure::<dyn Fn()>::new(move || {
            let val = cargo_toml.get_untracked();
            let cid = cid_save.clone();
            state.notebook.update(|nb_opt| {
                if let Some(nb) = nb_opt {
                    if let Some(cell) = nb.cells.iter_mut().find(|c| c.id == cid) {
                        cell.cargo_toml = Some(val);
                    }
                }
            });
            persist_notebook(&state);
            cargo_toml_dirty.set(false);
        });
        let save_fn: js_sys::Function =
            closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
        closure.forget();

        Callback::new(move |val: String| {
            cargo_toml.set(val);
            cargo_toml_dirty.set(true);

            // Mark this cell and all subsequent Code cells as stale.
            if !is_markdown {
                let cid = cell_id_for_stale_toml.clone();
                state.cell_stale.update(|stale| {
                    let cells = state.cells.get_untracked();
                    let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
                    for cell in &cells[my_idx..] {
                        if cell.cell_type == CellType::Code {
                            stale.insert(cell.id.clone(), true);
                        }
                    }
                });
            }

            let win = web_sys::window().unwrap();
            let prev = debounce_handle.get_untracked();
            if prev != 0 {
                win.clear_timeout_with_handle(prev);
            }
            let handle = win
                .set_timeout_with_callback_and_timeout_and_arguments_0(&save_fn, 1_000)
                .unwrap();
            debounce_handle.set(handle);
        })
    };

    #[cfg(not(feature = "hydrate"))]
    let on_cargo_toml_change = Callback::new(move |val: String| {
        cargo_toml.set(val);
    });

    // ── Notebook-level save flush ───────────────────────────────────────
    //
    // When the user triggers a notebook save (Ctrl+S or save button),
    // immediately flush this cell's current source and cargo_toml
    // into the notebook signal (the page-level handler persists to IndexedDB).

    #[cfg(feature = "hydrate")]
    {
        let cid_flush = cell_id_for_flush;
        let prev_save_gen = RwSignal::new(state.save_generation.get_untracked());

        Effect::new(move || {
            let gen = state.save_generation.get();
            if gen == prev_save_gen.get_untracked() {
                return;
            }
            prev_save_gen.set(gen);

            let src = source.get_untracked();
            let toml = cargo_toml.get_untracked();
            let cid = cid_flush.clone();

            state.notebook.update(|nb_opt| {
                if let Some(nb) = nb_opt {
                    if let Some(cell) = nb.cells.iter_mut().find(|c| c.id == cid) {
                        cell.source = src;
                        cell.cargo_toml = Some(toml);
                    }
                }
            });

            source_dirty.set(false);
            cargo_toml_dirty.set(false);
        });
    }

    // ── CSS classes ─────────────────────────────────────────────────────

    let cell_class = Signal::derive(move || {
        let mut class = "ironpad-cell-card".to_string();
        if is_markdown {
            class.push_str(" ironpad-cell-card--markdown");
        }
        if is_active() {
            class.push_str(" ironpad-cell-card--active");
        }
        class
    });

    let collapse_icon = Signal::derive(move || if collapsed.get() { "▸" } else { "▾" });

    let body_class = Signal::derive(move || {
        if collapsed.get() {
            "ironpad-cell-body ironpad-cell-body--collapsed"
        } else {
            "ironpad-cell-body"
        }
    });

    // ── Scroll-to & focus when this cell is newly added ─────────────────

    let cell_wrapper_ref: NodeRef<leptos::html::Div> = NodeRef::new();

    Effect::new(move || {
        let pending = state.pending_focus_cell.get();
        if pending.as_deref() != Some(cell_id_for_focus.as_str()) {
            return;
        }

        // Clear the pending focus to avoid re-triggering.
        state.pending_focus_cell.set(None);

        // Scroll the cell card into view.
        #[cfg(feature = "hydrate")]
        if let Some(el) = cell_wrapper_ref.get_untracked() {
            let html_el: &web_sys::Element = &el;
            html_el.scroll_into_view();
        }

        // Focus the source editor after a short delay to allow Monaco to
        // initialise asynchronously via the AMD loader.
        #[cfg(feature = "hydrate")]
        {
            use wasm_bindgen::prelude::*;

            let handle = source_handle;
            let closure = Closure::<dyn Fn()>::new(move || {
                if let Some(h) = handle.get_untracked() {
                    h.focus();
                }
            });
            let focus_fn: js_sys::Function =
                closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
            closure.forget();

            let _ = web_sys::window()
                .unwrap()
                .set_timeout_with_callback_and_timeout_and_arguments_0(&focus_fn, 300);
        }
    });

    // ── Render ──────────────────────────────────────────────────────────

    view! {
        <div node_ref=cell_wrapper_ref>
        <Card
            class=cell_class
            on:click=on_click
        >
            <CardHeader>
                <div class="ironpad-cell-header">
                    <button
                        class="ironpad-cell-collapse-btn"
                        on:click=move |ev| {
                            ev.stop_propagation();
                            collapsed.update(|c| *c = !*c);
                        }
                    >
                        {collapse_icon}
                    </button>

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

                    {move || {
                        if is_markdown {
                            return view! { <span /> }.into_any();
                        }
                        let is_stale = state.cell_stale.get()
                            .get(&cell_id_for_stale_header)
                            .copied()
                            .unwrap_or(false);
                        if is_stale {
                            view! { <span class="ironpad-stale-indicator" title="Cell output is stale">"⟳"</span> }.into_any()
                        } else {
                            view! { <span /> }.into_any()
                        }
                    }}

                    {if !is_markdown {
                        view! {
                            <Tag
                                size=TagSize::ExtraSmall
                                class=Signal::derive(move || {
                                    let suffix = match cell_status.get() {
                                        CellStatus::Idle => "idle",
                                        CellStatus::Queued => "queued",
                                        CellStatus::Compiling => "compiling",
                                        CellStatus::Running => "running",
                                        CellStatus::Success => "success",
                                        CellStatus::Error => "error",
                                    };
                                    format!("ironpad-cell-status ironpad-cell-status--{suffix}")
                                })
                            >
                                {move || {
                                    match cell_status.get() {
                                        CellStatus::Idle => "● idle".to_string(),
                                        CellStatus::Queued => "◎ queued".to_string(),
                                        CellStatus::Compiling => "◐ compiling…".to_string(),
                                        CellStatus::Running => "◐ running…".to_string(),
                                        CellStatus::Success => {
                                            match compile_time_ms.get() {
                                                Some(ms) => format!("✓ {ms:.0}ms"),
                                                None => "✓ done".to_string(),
                                            }
                                        }
                                        CellStatus::Error => "✕ error".to_string(),
                                    }
                                }}
                            </Tag>
                        }.into_any()
                    } else {
                        view! {
                            <Tag size=TagSize::ExtraSmall class="ironpad-cell-type-badge ironpad-cell-type-badge--markdown">
                                "📝 markdown"
                            </Tag>
                        }.into_any()
                    }}

                    <div class="ironpad-cell-actions">
                        {if !is_markdown {
                            view! {
                                <Button
                                    appearance=ButtonAppearance::Subtle
                                    on_click=move |ev: leptos::ev::MouseEvent| {
                                        ev.stop_propagation();
                                        run_trigger.update(|g| *g += 1);
                                    }
                                >
                                    {move || {
                                        if matches!(cell_status.get(), CellStatus::Compiling | CellStatus::Running) {
                                            "⏳"
                                        } else {
                                            "▶"
                                        }
                                    }}
                                </Button>
                            }.into_any()
                        } else {
                            view! { <span /> }.into_any()
                        }}

                        <div class="ironpad-cell-menu-wrapper">
                            <Button
                                appearance=ButtonAppearance::Subtle
                                on_click=move |ev: leptos::ev::MouseEvent| {
                                    ev.stop_propagation();
                                    menu_open.update(|v| *v = !*v);
                                }
                            >
                                "⋯"
                            </Button>

                            {move || {
                                if !menu_open.get() {
                                    return view! { <div /> }.into_any();
                                }

                                let first = is_first.get();
                                let last = is_last.get();

                                view! {
                                    <div
                                        class="ironpad-cell-menu-backdrop"
                                        on:click=move |_| menu_open.set(false)
                                    />
                                    <div class="ironpad-cell-menu">
                                        <button
                                            class="ironpad-cell-menu-item"
                                            disabled=first
                                            on:click=on_move_up
                                        >
                                            "↑ Move Up"
                                        </button>
                                        <button
                                            class="ironpad-cell-menu-item"
                                            disabled=last
                                            on:click=on_move_down
                                        >
                                            "↓ Move Down"
                                        </button>
                                        <button
                                            class="ironpad-cell-menu-item"
                                            on:click=on_duplicate
                                        >
                                            "⧉ Duplicate"
                                        </button>
                                        {if !is_markdown {
                                            view! {
                                                <button
                                                    class="ironpad-cell-menu-item"
                                                    on:click=on_run_all_below
                                                >
                                                    "▶▶ Run All Below"
                                                </button>
                                            }.into_any()
                                        } else {
                                            view! { <span /> }.into_any()
                                        }}
                                        <div class="ironpad-cell-menu-divider" />
                                        <button
                                            class="ironpad-cell-menu-item ironpad-cell-menu-item--danger"
                                            on:click=on_delete_confirmed
                                        >
                                            "🗑 Delete"
                                        </button>
                                    </div>
                                }.into_any()
                            }}
                        </div>
                    </div>
                </div>
            </CardHeader>

            {if is_markdown {
                // ── Markdown cell body ──────────────────────────────────
                view! {
                    <div class=body_class>
                        <MarkdownCell
                            source=source.get_untracked()
                            on_change=on_source_change
                            cell_id=cell_id_for_markdown.get_value()
                        />
                    </div>
                }.into_any()
            } else {
                // ── Code cell body + panels ─────────────────────────────
                view! {
                    <div class=body_class>
                        <div class="ironpad-cell-tabs">
                            <TabList selected_value=selected_tab>
                                <Tab value="code">
                                    {move || if source_dirty.get() { "Code ●" } else { "Code" }}
                                </Tab>
                                <Tab value="cargo-toml">
                                    {move || if cargo_toml_dirty.get() { "Cargo.toml ●" } else { "Cargo.toml" }}
                                </Tab>
                            </TabList>
                        </div>

                        <div
                            class="ironpad-cell-editor-pane"
                            style:display=move || {
                                if selected_tab.get() == "code" { "block" } else { "none" }
                            }
                        >
                            <MonacoEditor
                                initial_value=source.get_untracked()
                                language="rust"
                                on_change=on_source_change
                                handle=source_handle
                            />
                        </div>
                        <div
                            class="ironpad-cell-editor-pane"
                            style:display=move || {
                                if selected_tab.get() == "cargo-toml" { "block" } else { "none" }
                            }
                        >
                            <MonacoEditor
                                initial_value=cargo_toml.get_untracked()
                                language="toml"
                                on_change=on_cargo_toml_change
                                handle=cargo_toml_handle
                            />
                        </div>
                    </div>

                    // ── Compile result panel ────────────────────────────────────
                    <CompileResultPanel
                        cell_status=cell_status
                        last_compile=last_compile
                        compile_time_ms=compile_time_ms
                    />

                    // ── Execution output panel ──────────────────────────────────
                    <CellOutputPanel
                        execution_result=execution_result
                    />
                }.into_any()
            }}
        </Card>
        </div>
    }
}

// ── Compile result panel ─────────────────────────────────────────────────────

/// Displays compilation results below a cell: success info or error diagnostics.
///
/// Hidden when the cell has not been compiled yet (Idle state).
/// On success, shows a summary line with optional warnings.
/// On error, delegates to the dedicated [`ErrorPanel`] component (T-031).
#[component]
fn CompileResultPanel(
    cell_status: RwSignal<CellStatus>,
    last_compile: RwSignal<Option<CompileResponse>>,
    compile_time_ms: RwSignal<Option<f64>>,
) -> impl IntoView {
    view! {
        {move || {
            let status = cell_status.get();

            // Hide panel when idle, queued, or compiling (spinner is shown in the header).
            if matches!(status, CellStatus::Idle | CellStatus::Queued | CellStatus::Compiling | CellStatus::Running) {
                return view! { <div /> }.into_any();
            }

            let Some(response) = last_compile.get() else {
                return view! { <div /> }.into_any();
            };

            match status {
                CellStatus::Success => {
                    let blob_size = response.wasm_blob.len();
                    let cached = response.cached;
                    let time = compile_time_ms.get().unwrap_or(0.0);
                    let warnings: Vec<Diagnostic> = response
                        .diagnostics
                        .into_iter()
                        .filter(|d| d.severity == Severity::Warning)
                        .collect();

                    view! {
                        <div class="ironpad-compile-result ironpad-compile-result--success">
                            <div class="ironpad-compile-result-summary">
                                {format!(
                                    "✓ Compiled ({:.1} KB, {time:.0}ms{})",
                                    blob_size as f64 / 1024.0,
                                    if cached { ", cached" } else { "" },
                                )}
                            </div>
                            {if !warnings.is_empty() {
                                view! {
                                    <ErrorPanel diagnostics=warnings />
                                }.into_any()
                            } else {
                                view! { <div /> }.into_any()
                            }}
                        </div>
                    }.into_any()
                }

                CellStatus::Error => {
                    let diagnostics = response.diagnostics.clone();

                    view! {
                        <ErrorPanel diagnostics=diagnostics />
                    }.into_any()
                }

                _ => view! { <div /> }.into_any(),
            }
        }}
    }
}

// ── Cell output panel ────────────────────────────────────────────────────────

/// Displays execution output below a cell.
///
/// Shows the human-readable display text, a hex dump of raw output bytes with
/// byte count, and execution timing.  The panel is collapsible and hidden when
/// the cell has not been executed yet.
#[component]
fn CellOutputPanel(execution_result: RwSignal<Option<ExecutionResult>>) -> impl IntoView {
    let output_collapsed = RwSignal::new(false);

    view! {
        {move || {
            let Some(result) = execution_result.get() else {
                return view! { <div /> }.into_any();
            };

            let collapse_icon = if output_collapsed.get() { "▸" } else { "▾" };

            let panel_class = if output_collapsed.get() {
                "ironpad-output-panel ironpad-output-panel--collapsed"
            } else {
                "ironpad-output-panel"
            };

            let time_ms = result.execution_time_ms;
            let byte_count = result.output_bytes.len();
            let output_bytes = result.output_bytes.clone();

            // Parse display panels from JSON, with backward-compat fallback.
            let panels: Vec<DisplayPanel> = match &result.display_text {
                Some(json) => serde_json::from_str(json).unwrap_or_else(|_| {
                    vec![DisplayPanel::Text(json.clone())]
                }),
                None => vec![],
            };

            view! {
                <div class=panel_class>
                    <div
                        class="ironpad-output-header"
                        on:click=move |_| output_collapsed.update(|c| *c = !*c)
                    >
                        <span class="ironpad-output-toggle">{collapse_icon}</span>
                        <span class="ironpad-output-title">"Output"</span>
                        <span class="ironpad-output-meta">
                            {format!("{byte_count} bytes · {time_ms:.1}ms")}
                        </span>
                    </div>

                    {if !output_collapsed.get_untracked() {
                        let output_bytes = output_bytes.clone();

                        view! {
                            <div class="ironpad-output-body">
                                // Display panels section.
                                {panels.into_iter().map(|panel| {
                                    match panel {
                                        DisplayPanel::Text(text) => view! {
                                            <div class="ironpad-output-display">
                                                <pre class="ironpad-output-display-text">{text}</pre>
                                            </div>
                                        }.into_any(),
                                        DisplayPanel::Html(html) => view! {
                                            <div class="ironpad-output-display ironpad-output-html"
                                                 inner_html=html>
                                            </div>
                                        }.into_any(),
                                        DisplayPanel::Svg(svg) => view! {
                                            <div class="ironpad-output-display ironpad-output-svg"
                                                 inner_html=svg>
                                            </div>
                                        }.into_any(),
                                    }
                                }).collect::<Vec<_>>()}

                                // Raw bytes hex dump section.
                                {if !output_bytes.is_empty() {
                                    let hex = format_hex_dump(&output_bytes);
                                    view! {
                                        <div class="ironpad-output-bytes">
                                            <div class="ironpad-output-bytes-header">
                                                {format!("Raw output ({byte_count} bytes)")}
                                            </div>
                                            <pre class="ironpad-output-hex-dump">{hex}</pre>
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div /> }.into_any()
                                }}
                            </div>
                        }.into_any()
                    } else {
                        view! { <div /> }.into_any()
                    }}
                </div>
            }.into_any()
        }}
    }
}

/// Formats bytes as a hex dump with 16 bytes per row.
///
/// Each row shows: offset (hex)  │  hex bytes (space-separated)  │  ASCII repr
/// Non-printable bytes render as `.` in the ASCII column.
fn format_hex_dump(data: &[u8]) -> String {
    const BYTES_PER_ROW: usize = 16;

    let mut lines = Vec::new();

    for (i, chunk) in data.chunks(BYTES_PER_ROW).enumerate() {
        let offset = i * BYTES_PER_ROW;

        let hex_part: String = chunk
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<Vec<_>>()
            .join(" ");

        let ascii_part: String = chunk
            .iter()
            .map(|&b| {
                if b.is_ascii_graphic() || b == b' ' {
                    b as char
                } else {
                    '.'
                }
            })
            .collect();

        // Pad hex part to a fixed width so ASCII column aligns.
        lines.push(format!("{offset:08x}  {hex_part:<48}  {ascii_part}"));
    }

    lines.join("\n")
}

// ── Add cell button ─────────────────────────────────────────────────────────

/// "Add Cell" buttons (Code / Markdown), rendered between cells and at the end
/// of the list.
#[component]
fn AddCellButton(
    after_cell_id: Option<String>,
    on_add: Callback<(Option<String>, CellType)>,
) -> impl IntoView {
    let after_code = after_cell_id.clone();
    let after_md = after_cell_id.clone();
    let on_add_code = move |_| {
        on_add.run((after_code.clone(), CellType::Code));
    };
    let on_add_markdown = move |_| {
        on_add.run((after_md.clone(), CellType::Markdown));
    };

    view! {
        <div class="ironpad-add-cell-row">
            <button class="ironpad-add-cell-btn" on:click=on_add_code>
                "+ Code"
            </button>
            <button class="ironpad-add-cell-btn ironpad-add-cell-btn--markdown" on:click=on_add_markdown>
                "+ Markdown"
            </button>
        </div>
    }
}

// ── Notebook editor skeleton ────────────────────────────────────────────────

/// Skeleton placeholder shown while the notebook is loading.
#[component]
fn NotebookEditorSkeleton() -> impl IntoView {
    view! {
        <div class="ironpad-cell-list">
            <CellSkeleton />
            <CellSkeleton />
        </div>
    }
}

/// Skeleton placeholder for a single cell card.
#[component]
fn CellSkeleton() -> impl IntoView {
    view! {
        <Skeleton class="ironpad-cell-skeleton">
            <div class="ironpad-cell-skeleton-header">
                <SkeletonItem class="ironpad-skeleton-badge" />
                <SkeletonItem class="ironpad-skeleton-label" />
                <SkeletonItem class="ironpad-skeleton-status" />
            </div>
            <SkeletonItem class="ironpad-skeleton-editor" />
        </Skeleton>
    }
}
