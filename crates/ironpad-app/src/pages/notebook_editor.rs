use std::collections::HashMap;

use ironpad_common::{
    CellManifest, CompileRequest, CompileResponse, Diagnostic, ExecutionResult, NotebookManifest,
    Severity,
};
use leptos::prelude::*;
use leptos_router::hooks::use_params_map;
use thaw::{Button, ButtonAppearance, Card, CardHeader, Spinner, Tab, TabList};

use crate::components::app_layout::LayoutContext;
use crate::components::error_panel::ErrorPanel;
use crate::components::monaco_editor::{MonacoEditor, MonacoEditorHandle};
use crate::server_fns::{
    add_cell, compile_cell, delete_cell, get_cell_content, get_notebook, rename_cell,
    update_cell_cargo_toml, update_cell_source, update_notebook,
};

// ── Cell status ─────────────────────────────────────────────────────────────

/// Reactive cell execution status for the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CellStatus {
    Idle,
    Compiling,
    Success,
    Error,
}

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
    /// Cell ID that should be scrolled to and focused after creation.
    pending_focus_cell: RwSignal<Option<String>>,
    /// Per-cell output bytes from the last execution, keyed by cell ID.
    /// Used to pipe cell N's output as cell N+1's input.
    cell_outputs: RwSignal<HashMap<String, Vec<u8>>>,
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
        pending_focus_cell: RwSignal::new(None),
        cell_outputs: RwSignal::new(HashMap::new()),
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

    // Refresh notebook when a cell is added, and mark the new cell for focus.
    Effect::new(move || {
        if let Some(Ok(new_cell)) = add_cell_action.value().get() {
            state.pending_focus_cell.set(Some(new_cell.id.clone()));
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

    let source = RwSignal::new(String::new());
    let cargo_toml = RwSignal::new(String::new());

    // ── Dirty state (unsaved changes indicator) ─────────────────────────

    let source_dirty = RwSignal::new(false);
    let cargo_toml_dirty = RwSignal::new(false);

    // ── Load cell content ───────────────────────────────────────────────

    let nb_id = state.notebook_id.get_untracked();
    let cid_for_resource = cell.id.clone();
    let content_resource = Resource::new(move || (), {
        let nb_id = nb_id.clone();
        let cid = cid_for_resource.clone();
        move |_| {
            let nb_id = nb_id.clone();
            let cid = cid.clone();
            async move { get_cell_content(nb_id, cid).await }
        }
    });

    // Populate reactive state once content loads.
    Effect::new(move || {
        if let Some(Ok(content)) = content_resource.get() {
            source.set(content.source);
            cargo_toml.set(content.cargo_toml);
        }
    });

    // ── Delete action ───────────────────────────────────────────────────

    let nb_id = state.notebook_id.get_untracked();
    let delete_action = Action::new(move |_: &()| {
        let nb_id = nb_id.clone();
        let cid = cell_id_for_delete.clone();
        async move { delete_cell(nb_id, cid).await }
    });

    Effect::new(move || {
        if let Some(Ok(())) = delete_action.value().get() {
            // Remove deleted cell's cached output.
            state.cell_outputs.update(|map| {
                map.remove(&cell_id_for_delete_cleanup);
            });
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

    // ── Run cell action (compile flow) ──────────────────────────────────

    // Trigger signal: incrementing this dispatches a compile.
    let run_trigger = RwSignal::new(0u64);
    let cell_id_for_run = StoredValue::new(cell.id.clone());

    // The actual compile flow, driven by `run_trigger`.
    Effect::new(move || {
        let gen = run_trigger.get();
        if gen == 0 {
            return;
        }

        // Avoid double-dispatch while already compiling.
        if cell_status.get_untracked() == CellStatus::Compiling {
            return;
        }

        let cid = cell_id_for_run.get_value();
        let current_source = source.get_untracked();
        let current_cargo_toml = cargo_toml.get_untracked();

        // Resolve the previous cell's output bytes for the I/O pipeline.
        // Cell 0 receives empty input; all others receive the prior cell's output.
        let input_bytes = {
            let cells = state.cells.get_untracked();
            let my_idx = cells.iter().position(|c| c.id == cid);
            match my_idx {
                Some(idx) if idx > 0 => {
                    let prev_id = &cells[idx - 1].id;
                    state
                        .cell_outputs
                        .get_untracked()
                        .get(prev_id)
                        .cloned()
                        .unwrap_or_default()
                }
                _ => vec![],
            }
        };

        // Invalidate downstream cells' cached outputs (this cell and all after it).
        {
            let cells = state.cells.get_untracked();
            let my_idx = cells.iter().position(|c| c.id == cid).unwrap_or(0);
            let downstream_ids: Vec<String> =
                cells[my_idx..].iter().map(|c| c.id.clone()).collect();
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
                cell_id: cid,
                source: current_source,
                cargo_toml: current_cargo_toml,
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

                            let blob = response.wasm_blob.clone();
                            last_compile.set(Some(response));

                            let hash = executor::hash_wasm_blob(&blob);

                            let exec_err =
                                match executor::load_blob(&cell_id_for_exec, &hash, &blob).await {
                                    Ok(()) => {
                                        let exec_start = js_sys::Date::now();
                                        match executor::execute_cell(
                                            &cell_id_for_exec,
                                            &input_bytes,
                                        ) {
                                            Ok((output_bytes, display_text)) => {
                                                // Store output for downstream cells.
                                                state.cell_outputs.update(|map| {
                                                    map.insert(
                                                        cell_id_for_exec.clone(),
                                                        output_bytes.clone(),
                                                    );
                                                });

                                                execution_result.set(Some(ExecutionResult {
                                                    display_text,
                                                    output_bytes,
                                                    execution_time_ms: js_sys::Date::now()
                                                        - exec_start,
                                                }));
                                                cell_status.set(CellStatus::Success);
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
                                }));
                                cell_status.set(CellStatus::Error);
                            }
                        }

                        #[cfg(not(feature = "hydrate"))]
                        {
                            cell_status.set(CellStatus::Success);
                            last_compile.set(Some(response));
                        }
                    } else {
                        cell_status.set(CellStatus::Error);
                        last_compile.set(Some(response));
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
                    }));
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

        let nb_id_save = state.notebook_id.get_untracked();
        let cid_save = cell.id.clone();
        let debounce_handle: RwSignal<i32> = RwSignal::new(0);

        // Build a reusable JS function that reads the *current* source from
        // the signal and persists it.  Created once per cell (the `forget`
        // is a one-time, bounded leak).
        let closure = Closure::<dyn Fn()>::new(move || {
            let val = source.get_untracked();
            let nb_id = nb_id_save.clone();
            let cid = cid_save.clone();
            leptos::task::spawn_local(async move {
                if update_cell_source(nb_id, cid, val.clone()).await.is_ok() {
                    // Only clear dirty if the source hasn't changed while the
                    // save was in flight (avoids swallowing new edits).
                    if source.get_untracked() == val {
                        source_dirty.set(false);
                    }
                }
            });
        });
        let save_fn: js_sys::Function =
            closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
        closure.forget();

        Callback::new(move |val: String| {
            source.set(val);
            source_dirty.set(true);

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

        let nb_id_save = state.notebook_id.get_untracked();
        let cid_save = cell.id.clone();
        let debounce_handle: RwSignal<i32> = RwSignal::new(0);

        let closure = Closure::<dyn Fn()>::new(move || {
            let val = cargo_toml.get_untracked();
            let nb_id = nb_id_save.clone();
            let cid = cid_save.clone();
            leptos::task::spawn_local(async move {
                if update_cell_cargo_toml(nb_id, cid, val.clone())
                    .await
                    .is_ok()
                    && cargo_toml.get_untracked() == val
                {
                    cargo_toml_dirty.set(false);
                }
            });
        });
        let save_fn: js_sys::Function =
            closure.as_ref().unchecked_ref::<js_sys::Function>().clone();
        closure.forget();

        Callback::new(move |val: String| {
            cargo_toml.set(val);
            cargo_toml_dirty.set(true);

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

    // ── CSS classes ─────────────────────────────────────────────────────

    let cell_class = Signal::derive(move || {
        if is_active() {
            "ironpad-cell-card ironpad-cell-card--active".to_string()
        } else {
            "ironpad-cell-card".to_string()
        }
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

                    <span class=move || {
                        let suffix = match cell_status.get() {
                            CellStatus::Idle => "idle",
                            CellStatus::Compiling => "compiling",
                            CellStatus::Success => "success",
                            CellStatus::Error => "error",
                        };
                        format!("ironpad-cell-status ironpad-cell-status--{suffix}")
                    }>
                        {move || {
                            match cell_status.get() {
                                CellStatus::Idle => "idle".to_string(),
                                CellStatus::Compiling => "compiling…".to_string(),
                                CellStatus::Success => {
                                    match compile_time_ms.get() {
                                        Some(ms) => format!("✓ {ms:.0}ms"),
                                        None => "✓ done".to_string(),
                                    }
                                }
                                CellStatus::Error => "✕ error".to_string(),
                            }
                        }}
                    </span>

                    <div class="ironpad-cell-actions">
                        <Button
                            appearance=ButtonAppearance::Subtle
                            on_click=move |ev: leptos::ev::MouseEvent| {
                                ev.stop_propagation();
                                run_trigger.update(|g| *g += 1);
                            }
                        >
                            {move || {
                                if cell_status.get() == CellStatus::Compiling {
                                    "⏳"
                                } else {
                                    "▶"
                                }
                            }}
                        </Button>
                        <Button
                            appearance=ButtonAppearance::Subtle
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

                <Suspense fallback=move || view! {
                    <div class="ironpad-cell-loading">
                        <Spinner />
                    </div>
                }>
                    {move || Suspend::new(async move {
                        let _ = content_resource.await;

                        view! {
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
                        }
                    })}
                </Suspense>
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

            // Hide panel when idle or compiling (spinner is shown in the header).
            if status == CellStatus::Idle || status == CellStatus::Compiling {
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
            let display_text = result.display_text.clone();
            let output_bytes = result.output_bytes.clone();

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
                        let display_text = display_text.clone();
                        let output_bytes = output_bytes.clone();

                        view! {
                            <div class="ironpad-output-body">
                                // Display text section.
                                {if let Some(ref text) = display_text {
                                    view! {
                                        <div class="ironpad-output-display">
                                            <pre class="ironpad-output-display-text">{text.clone()}</pre>
                                        </div>
                                    }.into_any()
                                } else {
                                    view! { <div /> }.into_any()
                                }}

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
