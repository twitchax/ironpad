/// View-only notebook component — renders a notebook in read-only mode.
///
/// Users can view source code, run cells, and fork the notebook, but cannot
/// edit, add, delete, or reorder cells.
use std::collections::HashMap;

use leptos::prelude::*;

use ironpad_common::{CellType, CompileRequest, ExecutionResult, IronpadCell, IronpadNotebook};

use crate::components::markdown_cell::render_markdown;
use crate::components::monaco_editor::MonacoEditor;
use crate::server_fns::compile_cell;

// ── Display panels ──────────────────────────────────────────────────────────

/// Display panel types matching ironpad-cell's DisplayPanel enum.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
enum DisplayPanel {
    Text(String),
    Html(String),
    Svg(String),
}

// ── Per-cell output data ────────────────────────────────────────────────────

/// Cached output from a cell execution, used for piping data to downstream cells.
#[derive(Clone)]
struct CellOutputData {
    bytes: Vec<u8>,
    type_tag: Option<String>,
}

// ── ViewOnlyNotebook ────────────────────────────────────────────────────────

/// Read-only notebook view. Displays cells (code + markdown), supports
/// execution, and provides a fork button to clone the notebook.
#[component]
pub fn ViewOnlyNotebook(
    notebook: IronpadNotebook,
    /// Label shown on the fork button (e.g., "Fork" for public, "Fork to Private" for shared).
    #[prop(default = "Fork".to_string())]
    fork_label: String,
) -> impl IntoView {
    let notebook = StoredValue::new(notebook);

    // Shared state: execution outputs keyed by cell ID (for piping between cells).
    let cell_outputs: RwSignal<HashMap<String, CellOutputData>> = RwSignal::new(HashMap::new());

    // Fork handler — clones the notebook with a new ID and navigates to it.
    let fork_label_clone = fork_label.clone();
    let fork_action = move |_| {
        #[cfg(feature = "hydrate")]
        {
            leptos::task::spawn_local(async move {
                let mut nb = notebook.get_value();
                nb.id = uuid::Uuid::new_v4();
                nb.title = format!("{} (fork)", nb.title);
                nb.created_at = chrono::Utc::now();
                nb.updated_at = chrono::Utc::now();

                crate::storage::client::save_notebook(&nb).await;

                let navigate = leptos_router::hooks::use_navigate();
                navigate(
                    &format!("/notebook/{}", nb.id),
                    leptos_router::NavigateOptions::default(),
                );
            });
        }
    };

    // Suppress unused warning during SSR.
    #[cfg(not(feature = "hydrate"))]
    let _ = &fork_action;

    view! {
        <div class="view-only-notebook">
            <div class="view-only-toolbar">
                <h1 class="view-only-title">{notebook.with_value(|nb| nb.title.clone())}</h1>
                <button class="fork-button" on:click=fork_action>
                    {"🍴 "}{fork_label_clone}
                </button>
            </div>
            <div class="view-only-cells">
                {notebook.with_value(|nb| {
                    let cells = nb.cells.clone();
                    let shared_cargo_toml = nb.shared_cargo_toml.clone();
                    let notebook_id = nb.id.to_string();

                    cells.iter().map(|cell| {
                        let cell = cell.clone();
                        let all_cells = cells.clone();
                        let shared = shared_cargo_toml.clone();
                        let nid = notebook_id.clone();

                        view! {
                            <ViewOnlyCell
                                cell=cell
                                all_cells=all_cells
                                shared_cargo_toml=shared
                                notebook_id=nid
                                cell_outputs=cell_outputs
                            />
                        }
                    }).collect_view()
                })}
            </div>
        </div>
    }
}

// ── ViewOnlyCell ────────────────────────────────────────────────────────────

/// Dispatches rendering to the correct sub-component based on cell type.
#[component]
fn ViewOnlyCell(
    cell: IronpadCell,
    all_cells: Vec<IronpadCell>,
    shared_cargo_toml: Option<String>,
    notebook_id: String,
    cell_outputs: RwSignal<HashMap<String, CellOutputData>>,
) -> impl IntoView {
    match cell.cell_type {
        CellType::Code => view! {
            <ViewOnlyCodeCell
                cell=cell
                all_cells=all_cells
                shared_cargo_toml=shared_cargo_toml
                notebook_id=notebook_id
                cell_outputs=cell_outputs
            />
        }
        .into_any(),
        CellType::Markdown => view! {
            <ViewOnlyMarkdownCell source=cell.source.clone() />
        }
        .into_any(),
    }
}

// ── ViewOnlyCodeCell ────────────────────────────────────────────────────────

/// Renders a code cell with a read-only Monaco editor, run button, and output area.
#[component]
fn ViewOnlyCodeCell(
    cell: IronpadCell,
    all_cells: Vec<IronpadCell>,
    shared_cargo_toml: Option<String>,
    notebook_id: String,
    cell_outputs: RwSignal<HashMap<String, CellOutputData>>,
) -> impl IntoView {
    let cell = StoredValue::new(cell);
    let all_cells = StoredValue::new(all_cells);
    let shared_cargo_toml = StoredValue::new(shared_cargo_toml);
    let notebook_id = StoredValue::new(notebook_id);

    let compiling = RwSignal::new(false);
    let execution_result: RwSignal<Option<ExecutionResult>> = RwSignal::new(None);
    let error_message: RwSignal<Option<String>> = RwSignal::new(None);

    // Run cell: compile → load WASM → execute → display output.
    let run_cell = move |_| {
        #[cfg(feature = "hydrate")]
        {
            if compiling.get_untracked() {
                return;
            }
            compiling.set(true);
            error_message.set(None);

            leptos::task::spawn_local(async move {
                let cell_data = cell.get_value();
                let cells = all_cells.get_value();
                let my_idx = cells.iter().position(|c| c.id == cell_data.id).unwrap_or(0);

                // Compute previous cell types and serialized input bytes.
                let outputs = cell_outputs.get_untracked();
                let prev_code_cells: Vec<&IronpadCell> = cells[..my_idx]
                    .iter()
                    .filter(|c| c.cell_type == CellType::Code)
                    .collect();

                let mut all_output_bytes: Vec<&[u8]> = Vec::new();
                let mut types: Vec<Option<String>> = Vec::new();

                for c in &prev_code_cells {
                    if let Some(data) = outputs.get(&c.id) {
                        all_output_bytes.push(&data.bytes);
                        types.push(data.type_tag.clone());
                    } else {
                        all_output_bytes.push(&[]);
                        types.push(None);
                    }
                }

                // CellInputs wire format: [count: u32 LE][len0: u32 LE][bytes0]...
                let mut input_buf = Vec::new();
                input_buf.extend_from_slice(&(all_output_bytes.len() as u32).to_le_bytes());
                for output in &all_output_bytes {
                    input_buf.extend_from_slice(&(output.len() as u32).to_le_bytes());
                    input_buf.extend_from_slice(output);
                }

                let request = CompileRequest {
                    notebook_id: notebook_id.get_value(),
                    cell_id: cell_data.id.clone(),
                    source: cell_data.source.clone(),
                    cargo_toml: cell_data.cargo_toml.clone().unwrap_or_default(),
                    previous_cell_types: types,
                    shared_cargo_toml: shared_cargo_toml.get_value(),
                };

                match compile_cell(request).await {
                    Ok(response) => {
                        if response.wasm_blob.is_empty() {
                            let errors: Vec<String> = response
                                .diagnostics
                                .iter()
                                .map(|d| d.message.clone())
                                .collect();
                            error_message.set(Some(errors.join("\n")));
                            compiling.set(false);
                            return;
                        }

                        let hash = crate::components::executor::hash_wasm_blob(&response.wasm_blob);
                        match crate::components::executor::load_blob(
                            &cell_data.id,
                            &hash,
                            &response.wasm_blob,
                            response.js_glue,
                        )
                        .await
                        {
                            Ok(()) => {
                                let exec_start = js_sys::Date::now();
                                match crate::components::executor::execute_cell(
                                    &cell_data.id,
                                    &input_buf,
                                )
                                .await
                                {
                                    Ok((output_bytes, display_text, type_tag)) => {
                                        cell_outputs.update(|map| {
                                            map.insert(
                                                cell_data.id.clone(),
                                                CellOutputData {
                                                    bytes: output_bytes.clone(),
                                                    type_tag: type_tag.clone(),
                                                },
                                            );
                                        });

                                        execution_result.set(Some(ExecutionResult {
                                            display_text,
                                            output_bytes,
                                            execution_time_ms: js_sys::Date::now() - exec_start,
                                            type_tag,
                                        }));
                                    }
                                    Err(e) => {
                                        error_message.set(Some(format!("Execution error: {e}")));
                                    }
                                }
                            }
                            Err(e) => {
                                error_message.set(Some(format!("WASM load error: {e}")));
                            }
                        }
                    }
                    Err(e) => {
                        error_message.set(Some(format!("Compile error: {e}")));
                    }
                }

                compiling.set(false);
            });
        }
    };

    // Suppress unused warning during SSR.
    #[cfg(not(feature = "hydrate"))]
    let _ = &run_cell;

    view! {
        <div class="view-only-cell">
            <div class="view-only-cell-header">
                <span class="view-only-cell-label">{cell.with_value(|c| c.label.clone())}</span>
                <button
                    class="view-only-run-button"
                    on:click=run_cell
                    disabled=move || compiling.get()
                >
                    {move || if compiling.get() { "⏳ Compiling…" } else { "▶ Run" }}
                </button>
            </div>
            <MonacoEditor
                initial_value=cell.with_value(|c| c.source.clone())
                language="rust"
                read_only=true
            />
            {move || error_message.get().map(|err| view! {
                <div class="view-only-error">
                    <pre>{err}</pre>
                </div>
            })}
            {move || execution_result.get().map(|result| {
                view! { <ViewOnlyOutput result=result /> }
            })}
        </div>
    }
}

// ── ViewOnlyMarkdownCell ────────────────────────────────────────────────────

/// Renders markdown as HTML in preview-only mode (no edit toggle).
#[component]
fn ViewOnlyMarkdownCell(#[prop(into)] source: String) -> impl IntoView {
    let html = render_markdown(&source);

    if html.trim().is_empty() {
        view! {
            <div class="view-only-cell view-only-markdown">
                <p style="color: #4a4a6a; font-style: italic;">"(empty markdown cell)"</p>
            </div>
        }
        .into_any()
    } else {
        view! {
            <div class="view-only-cell view-only-markdown ironpad-markdown-cell-preview" inner_html=html></div>
        }
        .into_any()
    }
}

// ── ViewOnlyOutput ──────────────────────────────────────────────────────────

/// Renders execution output panels (text, HTML, SVG) and timing metadata.
#[component]
fn ViewOnlyOutput(result: ExecutionResult) -> impl IntoView {
    let panels: Vec<DisplayPanel> = match &result.display_text {
        Some(json) => {
            serde_json::from_str(json).unwrap_or_else(|_| vec![DisplayPanel::Text(json.clone())])
        }
        None => vec![],
    };

    let output_len = result.output_bytes.len();
    let exec_time = result.execution_time_ms;

    view! {
        <div class="view-only-output">
            <div class="view-only-output-meta">
                <span>{format!("{output_len} bytes")}</span>
                <span>{format!("{exec_time:.1} ms")}</span>
            </div>
            {panels.into_iter().map(|panel| {
                match panel {
                    DisplayPanel::Text(text) => view! {
                        <div class="view-only-output-display">
                            <pre class="view-only-output-text">{text}</pre>
                        </div>
                    }.into_any(),
                    DisplayPanel::Html(html) => view! {
                        <div class="view-only-output-display view-only-output-html" inner_html=html></div>
                    }.into_any(),
                    DisplayPanel::Svg(svg) => view! {
                        <div class="view-only-output-display view-only-output-svg" inner_html=svg></div>
                    }.into_any(),
                }
            }).collect_view()}
        </div>
    }
}
