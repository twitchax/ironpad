/// View-only notebook component — renders a notebook in read-only mode.
///
/// Users can view source code, run cells, and fork the notebook, but cannot
/// edit, add, delete, or reorder cells.
use std::collections::HashMap;

use leptos::prelude::*;

use ironpad_common::{CellType, CompileRequest, ExecutionResult, IronpadCell, IronpadNotebook};

use crate::components::copy_button::CopyButton;
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
    Markdown(String),
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Interactive UI widget (slider, dropdown, checkbox, etc.).
    Interactive {
        kind: String,
        config: String,
    },
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

    // Run-all sequential execution queue (cell IDs in order).
    let run_all_queue: RwSignal<Vec<String>> = RwSignal::new(Vec::new());
    let running_all: RwSignal<bool> = RwSignal::new(false);

    // Keep running_all in sync with the queue.
    Effect::new(move || {
        let empty = run_all_queue.get().is_empty();
        running_all.set(!empty);
    });

    // Auto-execute all code cells on page load (hydrate-only, fires once).
    #[cfg(feature = "hydrate")]
    {
        let auto_run_done = RwSignal::new(false);
        Effect::new(move || {
            if auto_run_done.get_untracked() {
                return;
            }
            auto_run_done.set(true);

            let cell_ids: Vec<String> = notebook.with_value(|nb| {
                nb.cells
                    .iter()
                    .filter(|c| c.cell_type == CellType::Code)
                    .map(|c| c.id.clone())
                    .collect()
            });
            if !cell_ids.is_empty() {
                run_all_queue.set(cell_ids);
            }
        });
    }

    // Run All handler — collect all code cell IDs and enqueue them.
    let run_all_action = move |_| {
        if running_all.get_untracked() {
            return;
        }
        let cell_ids: Vec<String> = notebook.with_value(|nb| {
            nb.cells
                .iter()
                .filter(|c| c.cell_type == CellType::Code)
                .map(|c| c.id.clone())
                .collect()
        });
        if !cell_ids.is_empty() {
            run_all_queue.set(cell_ids);
        }
    };

    // Fork handler — clones the notebook with a new ID and navigates to it.
    let fork_label_clone = fork_label.clone();
    let navigate = leptos_router::hooks::use_navigate();
    let fork_action = move |_| {
        let _ = &navigate;
        #[cfg(feature = "hydrate")]
        {
            let navigate = navigate.clone();
            leptos::task::spawn_local(async move {
                let mut nb = notebook.get_value();
                nb.id = uuid::Uuid::new_v4();
                nb.title = format!("{} (fork)", nb.title);
                nb.created_at = chrono::Utc::now();
                nb.updated_at = chrono::Utc::now();

                crate::storage::client::save_notebook(&nb).await;

                navigate(
                    &format!("/notebook/{}", nb.id),
                    leptos_router::NavigateOptions::default(),
                );
            });
        }
    };

    // ── Theme toggle state ──────────────────────────────────────────────

    let is_light_theme = RwSignal::new(false);

    #[cfg(feature = "hydrate")]
    {
        if let Some(window) = web_sys::window() {
            let stored = window
                .local_storage()
                .ok()
                .flatten()
                .and_then(|ls| ls.get_item("ironpad-theme").ok().flatten());
            if stored.as_deref() == Some("light") {
                is_light_theme.set(true);
            }
        }
    }

    view! {
        <div class="view-only-notebook">
            <div class="view-only-toolbar">
                <h1 class="view-only-title">{notebook.with_value(|nb| nb.title.clone())}</h1>
                <button
                    class="run-all-button"
                    on:click=run_all_action
                    disabled=move || running_all.get()
                >
                    {move || if running_all.get() { "⏳ Running…" } else { "▶▶ Run All" }}
                </button>
                <button
                    class="ironpad-toolbar-dropdown-toggle"
                    title="Toggle light/dark theme"
                    on:click=move |_| {
                        #[cfg(feature = "hydrate")]
                        {
                            use wasm_bindgen::JsCast as _;

                            let new_light = !is_light_theme.get_untracked();
                            is_light_theme.set(new_light);
                            if let Some(doc) = web_sys::window()
                                .and_then(|w| w.document())
                            {
                                if let Some(html) = doc.document_element() {
                                    if new_light {
                                        let _ = html.set_attribute("data-theme", "light");
                                    } else {
                                        let _ = html.remove_attribute("data-theme");
                                    }
                                }
                            }
                            if let Some(ls) = web_sys::window()
                                .and_then(|w| w.local_storage().ok().flatten())
                            {
                                let _ = ls.set_item(
                                    "ironpad-theme",
                                    if new_light { "light" } else { "dark" },
                                );
                            }
                            if let Some(monaco) = js_sys::Reflect::get(
                                &web_sys::window().unwrap(),
                                &"IronpadMonaco".into(),
                            )
                            .ok()
                            .filter(|v| !v.is_undefined())
                            {
                                if let Ok(set_theme) =
                                    js_sys::Reflect::get(&monaco, &"setTheme".into())
                                {
                                    if set_theme.is_function() {
                                        let f: js_sys::Function = set_theme.unchecked_into();
                                        let theme_name = if new_light {
                                            "ironpad-light"
                                        } else {
                                            "ironpad-dark"
                                        };
                                        let _ = f.call1(
                                            &wasm_bindgen::JsValue::NULL,
                                            &theme_name.into(),
                                        );
                                    }
                                }
                            }
                        }
                    }
                >
                    {move || if is_light_theme.get() { "☀" } else { "🌙" }}
                </button>
                <button class="fork-button" on:click=fork_action>
                    {"🍴 "}{fork_label_clone}
                </button>
            </div>
            <div class="view-only-cells">
                {notebook.with_value(|nb| {
                    let cells = nb.cells.clone();
                    let shared_cargo_toml = nb.shared_cargo_toml.clone();
                    let shared_source = nb.shared_source.clone();
                    let notebook_id = nb.id.to_string();

                    cells.iter().map(|cell| {
                        let cell = cell.clone();
                        let all_cells = cells.clone();
                        let shared = shared_cargo_toml.clone();
                        let shared_src = shared_source.clone();
                        let nid = notebook_id.clone();

                        view! {
                            <ViewOnlyCell
                                cell=cell
                                all_cells=all_cells
                                shared_cargo_toml=shared
                                shared_source=shared_src
                                notebook_id=nid
                                cell_outputs=cell_outputs
                                run_all_queue=run_all_queue
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
    shared_source: Option<String>,
    notebook_id: String,
    cell_outputs: RwSignal<HashMap<String, CellOutputData>>,
    run_all_queue: RwSignal<Vec<String>>,
) -> impl IntoView {
    match cell.cell_type {
        CellType::Code => view! {
            <ViewOnlyCodeCell
                cell=cell
                all_cells=all_cells
                shared_cargo_toml=shared_cargo_toml
                shared_source=shared_source
                notebook_id=notebook_id
                cell_outputs=cell_outputs
                run_all_queue=run_all_queue
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
    shared_source: Option<String>,
    notebook_id: String,
    cell_outputs: RwSignal<HashMap<String, CellOutputData>>,
    run_all_queue: RwSignal<Vec<String>>,
) -> impl IntoView {
    let cell = StoredValue::new(cell);
    let all_cells = StoredValue::new(all_cells);
    let shared_cargo_toml = StoredValue::new(shared_cargo_toml);
    let shared_source = StoredValue::new(shared_source);
    let notebook_id = StoredValue::new(notebook_id);

    let compiling = RwSignal::new(false);
    let execution_result: RwSignal<Option<ExecutionResult>> = RwSignal::new(None);
    let error_message: RwSignal<Option<String>> = RwSignal::new(None);

    // Trigger signal: incrementing this dispatches a compile.
    let run_trigger = RwSignal::new(0u64);

    // Run button click — increment run_trigger to start compile.
    let run_cell = move |_| {
        run_trigger.update(|g| *g += 1);
    };

    // ── Run-all queue watcher ───────────────────────────────────────────
    //
    // When this cell appears at the front of the run-all queue, trigger
    // its compile flow. Non-front cells show a "Queued" status badge.

    let cell_id_for_queue = StoredValue::new(cell.with_value(|c| c.id.clone()));

    let queued = Signal::derive(move || {
        let queue = run_all_queue.get();
        let cid = cell_id_for_queue.get_value();
        queue
            .iter()
            .position(|id| id == &cid)
            .is_some_and(|pos| pos > 0)
    });

    Effect::new(move || {
        let queue = run_all_queue.get();
        let cid = cell_id_for_queue.get_value();

        if queue.first().map(|id| id == &cid).unwrap_or(false) && !compiling.get_untracked() {
            run_trigger.update(|g| *g += 1);
        }
    });

    // ── Compile + execute flow, driven by `run_trigger` ─────────────────

    let cell_id_for_exec = StoredValue::new(cell.with_value(|c| c.id.clone()));

    Effect::new(move || {
        let gen = run_trigger.get();
        if gen == 0 {
            return;
        }

        if compiling.get_untracked() {
            return;
        }

        #[cfg(feature = "hydrate")]
        {
            compiling.set(true);
            error_message.set(None);

            leptos::task::spawn_local(async move {
                let cell_data = cell.get_value();
                let cell_id = cell_id_for_exec.get_value();
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
                    shared_source: shared_source.get_value(),
                };

                let mut had_error = false;

                match compile_cell(request).await {
                    Ok(response) => {
                        if response.wasm_blob.is_empty() {
                            let errors: Vec<String> = response
                                .diagnostics
                                .iter()
                                .map(|d| d.message.clone())
                                .collect();
                            error_message.set(Some(errors.join("\n")));
                            had_error = true;
                        } else {
                            let hash =
                                crate::components::executor::hash_wasm_blob(&response.wasm_blob);
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
                                            error_message
                                                .set(Some(format!("Execution error: {e}")));
                                            had_error = true;
                                        }
                                    }
                                }
                                Err(e) => {
                                    error_message.set(Some(format!("WASM load error: {e}")));
                                    had_error = true;
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error_message.set(Some(format!("Compile error: {e}")));
                        had_error = true;
                    }
                }

                // Advance or clear the run-all queue.
                if had_error {
                    run_all_queue.set(vec![]);
                } else {
                    run_all_queue.update(|q| {
                        if q.first().map(|id| id == &cell_id).unwrap_or(false) {
                            q.remove(0);
                        }
                    });
                }

                compiling.set(false);
            });
        }
    });

    // Suppress unused warning during SSR.
    #[cfg(not(feature = "hydrate"))]
    let _ = &run_cell;

    // Code is collapsed by default to match editor view mode.
    let collapsed = RwSignal::new(true);
    let collapse_icon = Signal::derive(move || if collapsed.get() { "▸" } else { "▾" });
    let body_class = Signal::derive(move || {
        if collapsed.get() {
            "ironpad-cell-body ironpad-cell-body--collapsed"
        } else {
            "ironpad-cell-body"
        }
    });

    // Button text depends on compiling/queued state.
    let button_text = Signal::derive(move || {
        if compiling.get() {
            "⏳ Compiling…"
        } else if queued.get() {
            "⏳ Queued"
        } else {
            "▶ Run"
        }
    });

    view! {
        <div class="view-only-cell ironpad-cell--view-mode">
            <div class="view-only-cell-header">
                <button
                    class="ironpad-cell-collapse-btn"
                    on:click=move |_| collapsed.update(|c| *c = !*c)
                >
                    {collapse_icon}
                </button>
                <span class="view-only-cell-label">{cell.with_value(|c| c.label.clone())}</span>
                <button
                    class="view-only-run-button"
                    on:click=run_cell
                    disabled=move || compiling.get() || queued.get()
                >
                    {button_text}
                </button>
            </div>
            <div class=body_class>
                <MonacoEditor
                    initial_value=cell.with_value(|c| c.source.clone())
                    language="rust"
                    read_only=true
                />
            </div>
            {move || error_message.get().map(|err| view! {
                <div class="view-only-error">
                    <pre>{err}</pre>
                </div>
            })}
            {move || execution_result.get().map(|result| {
                let cell_id = cell.with_value(|c| c.id.clone());
                let all_cells_vec = all_cells.get_value();
                view! { <ViewOnlyOutput result=result cell_id=cell_id all_cells=all_cells_vec run_all_queue=run_all_queue cell_outputs=cell_outputs /> }
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
                <p class="ironpad-placeholder">"(empty markdown cell)"</p>
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
fn ViewOnlyOutput(
    result: ExecutionResult,
    #[prop(into)] cell_id: String,
    all_cells: Vec<IronpadCell>,
    run_all_queue: RwSignal<Vec<String>>,
    cell_outputs: RwSignal<HashMap<String, CellOutputData>>,
) -> impl IntoView {
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
                    DisplayPanel::Text(text) => {
                        let copy_text = text.clone();
                        view! {
                            <div class="view-only-output-display">
                                <CopyButton text=copy_text />
                                <pre class="view-only-output-text">{text}</pre>
                            </div>
                        }.into_any()
                    },
                    DisplayPanel::Html(html) => {
                        let copy_text = html.clone();
                        view! {
                            <div class="view-only-output-display view-only-output-html">
                                <CopyButton text=copy_text />
                                <div inner_html=html></div>
                            </div>
                        }.into_any()
                    },
                    DisplayPanel::Svg(svg) => {
                        let copy_text = svg.clone();
                        view! {
                            <div class="view-only-output-display view-only-output-svg">
                                <CopyButton text=copy_text />
                                <div inner_html=svg></div>
                            </div>
                        }.into_any()
                    },
                    DisplayPanel::Markdown(md) => {
                        let copy_text = md.clone();
                        let rendered = render_markdown(&md);
                        view! {
                            <div class="view-only-output-display">
                                <CopyButton text=copy_text />
                                <div class="ironpad-markdown-cell-preview" inner_html=rendered></div>
                            </div>
                        }.into_any()
                    },
                    DisplayPanel::Table { headers, rows } => {
                        let copy_text = render_table_tsv(&headers, &rows);
                        let table_html = render_table_html(&headers, &rows);
                        view! {
                            <div class="view-only-output-display">
                                <CopyButton text=copy_text />
                                <div inner_html=table_html></div>
                            </div>
                        }.into_any()
                    },
                    DisplayPanel::Interactive { kind, config } => {
                        let cid = cell_id.clone();
                        let cells = all_cells.clone();
                        view! {
                            <ViewOnlyInteractiveWidget kind=kind config=config cell_id=cid all_cells=cells run_all_queue=run_all_queue cell_outputs=cell_outputs />
                        }.into_any()
                    },
                }
            }).collect_view()}
        </div>
    }
}

/// Render a table as an HTML `<table>` string with the `ironpad-output-table` class.
fn render_table_html(headers: &[String], rows: &[Vec<String>]) -> String {
    let mut html = String::from("<table class=\"ironpad-output-table\"><thead><tr>");
    for h in headers {
        html.push_str(&format!("<th>{}</th>", html_escape(h)));
    }
    html.push_str("</tr></thead><tbody>");
    for row in rows {
        html.push_str("<tr>");
        for cell in row {
            html.push_str(&format!("<td>{}</td>", html_escape(cell)));
        }
        html.push_str("</tr>");
    }
    html.push_str("</tbody></table>");
    html
}

/// Render a table as tab-separated values for clipboard copy.
fn render_table_tsv(headers: &[String], rows: &[Vec<String>]) -> String {
    let mut tsv = headers.join("\t");
    for row in rows {
        tsv.push('\n');
        tsv.push_str(&row.join("\t"));
    }
    tsv
}

// ── ViewOnlyInteractiveWidget ────────────────────────────────────────────────

/// Renders an interactive widget in read-only mode (shows current/default value).
#[component]
fn ViewOnlyInteractiveWidget(
    #[prop(into)] kind: String,
    #[prop(into)] config: String,
    #[prop(into)] cell_id: String,
    all_cells: Vec<IronpadCell>,
    run_all_queue: RwSignal<Vec<String>>,
    cell_outputs: RwSignal<HashMap<String, CellOutputData>>,
) -> impl IntoView {
    let cfg: serde_json::Value = serde_json::from_str(&config).unwrap_or_default();
    let label = cfg
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();

    let content = match kind.as_str() {
        "slider" | "number" => {
            let default = cfg.get("default").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let min = cfg.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let max = cfg.get("max").and_then(|v| v.as_f64()).unwrap_or(100.0);
            let step = cfg.get("step").and_then(|v| v.as_f64()).unwrap_or(1.0);
            let label_text = if label.is_empty() {
                kind.clone()
            } else {
                label.clone()
            };

            let value = RwSignal::new(default.to_string());

            #[cfg(feature = "hydrate")]
            let on_input = {
                let cell_id = cell_id.clone();
                move |ev: web_sys::Event| {
                    let new_val = leptos::prelude::event_target_value(&ev);
                    value.set(new_val.clone());
                    if let Ok(f) = new_val.parse::<f64>() {
                        let bytes = bincode_encode_f64(f);
                        update_view_cell_output(bytes, &cell_id, cell_outputs);
                    }
                }
            };

            #[cfg(not(feature = "hydrate"))]
            let on_input = move |_: leptos::ev::Event| {};
            let _ = (&cell_id, &cell_outputs);

            view! {
                <div class="ironpad-interactive-widget">
                    <span class="ironpad-widget-label">{label_text}</span>
                    <input
                        type="range"
                        min=min.to_string()
                        max=max.to_string()
                        step=step.to_string()
                        prop:value=move || value.get()
                        on:input=on_input
                    />
                    <span class="ironpad-widget-value">{move || value.get()}</span>
                </div>
            }
            .into_any()
        }
        "dropdown" => {
            let options: Vec<String> = cfg
                .get("options")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default();
            let default = cfg
                .get("default")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let label_text = if label.is_empty() {
                "dropdown".to_owned()
            } else {
                label.clone()
            };

            let selected = RwSignal::new(default);

            #[cfg(feature = "hydrate")]
            let on_change = {
                let cell_id = cell_id.clone();
                move |ev: web_sys::Event| {
                    let new_val = leptos::prelude::event_target_value(&ev);
                    selected.set(new_val.clone());
                    let bytes = bincode_encode_string(&new_val);
                    update_view_cell_output(bytes, &cell_id, cell_outputs);
                }
            };

            #[cfg(not(feature = "hydrate"))]
            let on_change = move |_: leptos::ev::Event| {};
            let _ = (&cell_id, &cell_outputs);

            view! {
                <div class="ironpad-interactive-widget">
                    <span class="ironpad-widget-label">{label_text}</span>
                    <select
                        prop:value=move || selected.get()
                        on:change=on_change
                    >
                        {options.into_iter().map(|opt| {
                            let opt_val = opt.clone();
                            let opt_selected = opt.clone();
                            view! {
                                <option value=opt_val selected=move || selected.get() == opt_selected>
                                    {opt}
                                </option>
                            }
                        }).collect_view()}
                    </select>
                </div>
            }
            .into_any()
        }
        "checkbox" | "switch" => {
            let default = cfg
                .get("default")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let checked = RwSignal::new(default);

            #[cfg(feature = "hydrate")]
            let on_change = {
                let cell_id = cell_id.clone();
                move |ev: web_sys::Event| {
                    use wasm_bindgen::JsCast;
                    if let Some(input) = ev
                        .target()
                        .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
                    {
                        let new_val = input.checked();
                        checked.set(new_val);
                        let bytes = bincode_encode_bool(new_val);
                        update_view_cell_output(bytes, &cell_id, cell_outputs);
                    }
                }
            };

            #[cfg(not(feature = "hydrate"))]
            let on_change = move |_: leptos::ev::Event| {};
            let _ = (&cell_id, &cell_outputs);

            view! {
                <div class="ironpad-interactive-widget">
                    <label>
                        <input
                            type="checkbox"
                            prop:checked=move || checked.get()
                            on:change=on_change
                        />
                        {" "}{label.clone()}
                    </label>
                </div>
            }
            .into_any()
        }
        "text_input" => {
            let placeholder = cfg
                .get("placeholder")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let default = cfg
                .get("default")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let label_text = if label.is_empty() {
                "text".to_owned()
            } else {
                label.clone()
            };

            let text = RwSignal::new(default);

            #[cfg(feature = "hydrate")]
            let on_input = {
                let cell_id = cell_id.clone();
                move |ev: web_sys::Event| {
                    let new_val = leptos::prelude::event_target_value(&ev);
                    text.set(new_val.clone());
                    let bytes = bincode_encode_string(&new_val);
                    update_view_cell_output(bytes, &cell_id, cell_outputs);
                }
            };

            #[cfg(not(feature = "hydrate"))]
            let on_input = move |_: leptos::ev::Event| {};
            let _ = (&cell_id, &cell_outputs);

            view! {
                <div class="ironpad-interactive-widget">
                    <span class="ironpad-widget-label">{label_text}</span>
                    <input
                        type="text"
                        placeholder=placeholder
                        prop:value=move || text.get()
                        on:input=on_input
                    />
                </div>
            }
            .into_any()
        }
        "button" => {
            let button_label = if label.is_empty() {
                "Run ▶".to_owned()
            } else {
                label.clone()
            };

            #[cfg(feature = "hydrate")]
            let on_click = {
                let cell_id = cell_id.clone();
                move |_: web_sys::MouseEvent| {
                    if let Some(my_idx) = all_cells.iter().position(|c| c.id == cell_id) {
                        let downstream: Vec<String> = all_cells[my_idx + 1..]
                            .iter()
                            .filter(|c| c.cell_type == CellType::Code)
                            .map(|c| c.id.clone())
                            .collect();
                        if !downstream.is_empty() {
                            run_all_queue.set(downstream);
                        }
                    }
                }
            };

            #[cfg(not(feature = "hydrate"))]
            let on_click = move |_: leptos::web_sys::MouseEvent| {};
            let _ = (&cell_id, &run_all_queue);

            view! {
                <div class="ironpad-interactive-widget">
                    <button class="ironpad-widget-button" on:click=on_click>
                        {button_label}
                    </button>
                </div>
            }
            .into_any()
        }
        "progress" => {
            let id = cfg
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_owned();
            let initial = cfg.get("initial").and_then(|v| v.as_f64()).unwrap_or(0.0);
            let pct = initial.clamp(0.0, 100.0);
            let width_style = format!("width: {pct}%");
            let label_text = if label.is_empty() {
                String::new()
            } else {
                label.to_owned()
            };

            view! {
                <div class="ironpad-interactive-widget">
                    {if !label_text.is_empty() {
                        Some(view! { <span class="ironpad-widget-label">{label_text}</span> })
                    } else {
                        None
                    }}
                    <div class="ironpad-progress" data-progress-id={id}>
                        <div class="ironpad-progress-bar">
                            <div class="ironpad-progress-fill" style={width_style}></div>
                        </div>
                        <span class="ironpad-progress-value">{format!("{}%", pct as u32)}</span>
                    </div>
                </div>
            }
            .into_any()
        }
        _ => view! {
            <div class="ironpad-interactive-widget ironpad-interactive-widget--readonly">
                <span class="ironpad-widget-label">{format!("[{kind} widget]")}</span>
            </div>
        }
        .into_any(),
    };

    content
}

// ── Bincode encoding helpers (hydrate-only) ─────────────────────────────────

#[cfg(feature = "hydrate")]
fn bincode_encode_f64(value: f64) -> Vec<u8> {
    bincode::encode_to_vec(value, bincode::config::standard()).expect("f64 encoding cannot fail")
}

#[cfg(feature = "hydrate")]
fn bincode_encode_bool(value: bool) -> Vec<u8> {
    bincode::encode_to_vec(value, bincode::config::standard()).expect("bool encoding cannot fail")
}

#[cfg(feature = "hydrate")]
fn bincode_encode_string(value: &str) -> Vec<u8> {
    bincode::encode_to_vec(value, bincode::config::standard()).expect("String encoding cannot fail")
}

#[cfg(feature = "hydrate")]
fn update_view_cell_output(
    new_bytes: Vec<u8>,
    cell_id: &str,
    cell_outputs: RwSignal<HashMap<String, CellOutputData>>,
) {
    cell_outputs.update(|map| {
        if let Some(data) = map.get_mut(cell_id) {
            data.bytes = new_bytes;
        }
    });
}

/// Minimal HTML entity escaping for text content.
fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
