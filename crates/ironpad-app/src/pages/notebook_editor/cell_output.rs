use std::collections::HashMap;

use ironpad_common::{
    CellManifest, CellType, CompileResponse, Diagnostic, ExecutionResult, Severity,
};
use leptos::prelude::*;

use crate::components::copy_button::CopyButton;
use crate::components::error_panel::ErrorPanel;
use crate::components::markdown_cell::render_markdown;

use super::export::{render_table_html, render_table_tsv, DisplayPanel};
use super::state::{CellOutputData, CellStatus};

// ── Widget context ───────────────────────────────────────────────────────────

/// Bundles the reactive signals needed by interactive widgets to update cell
/// outputs and trigger downstream re-execution.
#[derive(Clone, Copy)]
#[allow(dead_code)]
struct WidgetContext {
    cell_outputs: RwSignal<HashMap<String, CellOutputData>>,
    cell_stale: RwSignal<HashMap<String, bool>>,
    cells: RwSignal<Vec<CellManifest>>,
    run_all_queue: RwSignal<Vec<String>>,
}

// ── Compile result panel ─────────────────────────────────────────────────────

/// Displays compilation results below a cell: success info or error diagnostics.
///
/// Hidden when the cell has not been compiled yet (Idle state).
/// On success, shows a summary line with optional warnings.
/// On error, delegates to the dedicated [`ErrorPanel`] component (T-031).
#[component]
pub(super) fn CompileResultPanel(
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
pub(super) fn CellOutputPanel(
    execution_result: RwSignal<Option<ExecutionResult>>,
    /// Cell ID for this cell (needed to update outputs on widget change).
    #[prop(optional, into)]
    cell_id: Option<String>,
    /// Notebook-level cell outputs signal (for updating bytes on widget change).
    #[prop(optional)]
    cell_outputs: Option<RwSignal<HashMap<String, CellOutputData>>>,
    /// Notebook-level cell stale signal (for marking downstream cells stale).
    #[prop(optional)]
    cell_stale: Option<RwSignal<HashMap<String, bool>>>,
    /// Ordered cell list (for finding downstream cells).
    #[prop(optional)]
    cells: Option<RwSignal<Vec<CellManifest>>>,
    /// Run-all queue (downstream cell IDs are pushed here for execution).
    #[prop(optional)]
    run_all_queue: Option<RwSignal<Vec<String>>>,
) -> impl IntoView {
    // Build widget context if all required signals are present.
    let widget_ctx = match (cell_outputs, cell_stale, cells, run_all_queue) {
        (Some(cell_outputs), Some(cell_stale), Some(cells), Some(run_all_queue)) => {
            Some(WidgetContext {
                cell_outputs,
                cell_stale,
                cells,
                run_all_queue,
            })
        }
        _ => None,
    };

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
                                        DisplayPanel::Text(text) => {
                                            let copy_text = text.clone();
                                            view! {
                                                <div class="ironpad-output-display">
                                                    <CopyButton text=copy_text />
                                                    <pre class="ironpad-output-display-text">{text}</pre>
                                                </div>
                                            }.into_any()
                                        },
                                        DisplayPanel::Html(html) => {
                                            let copy_text = html.clone();
                                            view! {
                                                <div class="ironpad-output-display ironpad-output-html">
                                                    <CopyButton text=copy_text />
                                                    <div inner_html=html></div>
                                                </div>
                                            }.into_any()
                                        },
                                        DisplayPanel::Svg(svg) => {
                                            let copy_text = svg.clone();
                                            view! {
                                                <div class="ironpad-output-display ironpad-output-svg">
                                                    <CopyButton text=copy_text />
                                                    <div inner_html=svg></div>
                                                </div>
                                            }.into_any()
                                        },
                                        DisplayPanel::Markdown(md) => {
                                            let copy_text = md.clone();
                                            let rendered = render_markdown(&md);
                                            view! {
                                                <div class="ironpad-output-display">
                                                    <CopyButton text=copy_text />
                                                    <div class="ironpad-markdown-cell-preview" inner_html=rendered></div>
                                                </div>
                                            }.into_any()
                                        },
                                        DisplayPanel::Table { headers, rows } => {
                                            let copy_text = render_table_tsv(&headers, &rows);
                                            let table_html = render_table_html(&headers, &rows);
                                            view! {
                                                <div class="ironpad-output-display">
                                                    <CopyButton text=copy_text />
                                                    <div inner_html=table_html></div>
                                                </div>
                                            }.into_any()
                                        },
                                        DisplayPanel::Interactive { kind, config } => {
                                            let cid = cell_id.clone();
                                            view! {
                                                <InteractiveWidget
                                                    kind=kind
                                                    config=config
                                                    cell_id=cid
                                                    widget_ctx=widget_ctx
                                                />
                                            }.into_any()
                                        },
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

// ── Bincode encoding helpers (hydrate-only) ──────────────────────────────────

/// Encode an `f64` value to bincode v2 standard-config bytes.
#[cfg(feature = "hydrate")]
fn bincode_encode_f64(value: f64) -> Vec<u8> {
    bincode::encode_to_vec(value, bincode::config::standard()).expect("f64 encoding cannot fail")
}

/// Encode a `bool` value to bincode v2 standard-config bytes.
#[cfg(feature = "hydrate")]
fn bincode_encode_bool(value: bool) -> Vec<u8> {
    bincode::encode_to_vec(value, bincode::config::standard()).expect("bool encoding cannot fail")
}

/// Encode a `String` value to bincode v2 standard-config bytes.
#[cfg(feature = "hydrate")]
fn bincode_encode_string(value: &str) -> Vec<u8> {
    bincode::encode_to_vec(value, bincode::config::standard()).expect("String encoding cannot fail")
}

/// Update cell outputs and mark downstream cells stale after a widget value
/// change.
#[cfg(feature = "hydrate")]
fn update_cell_output(
    new_bytes: Vec<u8>,
    cell_id: &Option<String>,
    widget_ctx: Option<WidgetContext>,
) {
    let Some(cid) = cell_id else { return };
    let Some(ctx) = widget_ctx else { return };

    ctx.cell_outputs.update(|map| {
        if let Some(data) = map.get_mut(cid) {
            data.bytes = new_bytes;
        }
    });

    ctx.cell_stale.update(|map| {
        for val in map.values_mut() {
            *val = true;
        }
    });
}

// ── Interactive widget component ─────────────────────────────────────────────

/// Renders an interactive UI widget (slider, dropdown, checkbox, etc.) with
/// live value change callbacks that update cell outputs.
#[component]
fn InteractiveWidget(
    #[prop(into)] kind: String,
    #[prop(into)] config: String,
    cell_id: Option<String>,
    widget_ctx: Option<WidgetContext>,
) -> impl IntoView {
    let cfg: serde_json::Value = serde_json::from_str(&config).unwrap_or_default();
    let label = cfg
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();

    match kind.as_str() {
        "slider" => render_slider(&cfg, &label, cell_id, widget_ctx).into_any(),
        "dropdown" => render_dropdown(&cfg, &label, cell_id, widget_ctx).into_any(),
        "checkbox" => render_checkbox(&cfg, &label, cell_id, widget_ctx).into_any(),
        "text_input" => render_text_input(&cfg, &label, cell_id, widget_ctx).into_any(),
        "number" => render_number(&cfg, &label, cell_id, widget_ctx).into_any(),
        "switch" => render_switch(&cfg, &label, cell_id, widget_ctx).into_any(),
        "button" => render_button(&cfg, &label, cell_id, widget_ctx).into_any(),
        _ => view! {
            <div class="ironpad-interactive-widget">
                <span class="ironpad-widget-label">{format!("[unknown widget: {kind}]")}</span>
            </div>
        }
        .into_any(),
    }
}

fn render_slider(
    cfg: &serde_json::Value,
    label: &str,
    cell_id: Option<String>,
    widget_ctx: Option<WidgetContext>,
) -> impl IntoView {
    let min = cfg.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let max = cfg.get("max").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let step = cfg.get("step").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let default = cfg.get("default").and_then(|v| v.as_f64()).unwrap_or(min);
    let label_text = if label.is_empty() {
        String::new()
    } else {
        label.to_owned()
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
                update_cell_output(bytes, &cell_id, widget_ctx);
            }
        }
    };

    // Suppress unused warnings during SSR.
    #[cfg(not(feature = "hydrate"))]
    let on_input = move |_: leptos::ev::Event| {};
    let _ = (&cell_id, &widget_ctx);

    view! {
        <div class="ironpad-interactive-widget">
            {if !label_text.is_empty() {
                view! { <span class="ironpad-widget-label">{label_text}</span> }.into_any()
            } else {
                view! { <span /> }.into_any()
            }}
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
}

fn render_dropdown(
    cfg: &serde_json::Value,
    label: &str,
    cell_id: Option<String>,
    widget_ctx: Option<WidgetContext>,
) -> impl IntoView {
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
        String::new()
    } else {
        label.to_owned()
    };

    let value = RwSignal::new(default);

    #[cfg(feature = "hydrate")]
    let on_change = {
        let cell_id = cell_id.clone();
        move |ev: web_sys::Event| {
            let new_val = leptos::prelude::event_target_value(&ev);
            value.set(new_val.clone());
            let bytes = bincode_encode_string(&new_val);
            update_cell_output(bytes, &cell_id, widget_ctx);
        }
    };

    #[cfg(not(feature = "hydrate"))]
    let on_change = move |_: leptos::ev::Event| {};
    let _ = (&cell_id, &widget_ctx);

    view! {
        <div class="ironpad-interactive-widget">
            {if !label_text.is_empty() {
                view! { <span class="ironpad-widget-label">{label_text}</span> }.into_any()
            } else {
                view! { <span /> }.into_any()
            }}
            <select
                prop:value=move || value.get()
                on:change=on_change
            >
                {options.into_iter().map(|opt| {
                    let opt_val = opt.clone();
                    let opt_selected = opt.clone();
                    view! {
                        <option value=opt_val selected=move || value.get() == opt_selected>
                            {opt}
                        </option>
                    }
                }).collect_view()}
            </select>
        </div>
    }
}

fn render_checkbox(
    cfg: &serde_json::Value,
    label: &str,
    cell_id: Option<String>,
    widget_ctx: Option<WidgetContext>,
) -> impl IntoView {
    let default = cfg
        .get("default")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let label_text = label.to_owned();

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
                update_cell_output(bytes, &cell_id, widget_ctx);
            }
        }
    };

    #[cfg(not(feature = "hydrate"))]
    let on_change = move |_: leptos::ev::Event| {};
    let _ = (&cell_id, &widget_ctx);

    view! {
        <div class="ironpad-interactive-widget">
            <label class="ironpad-widget-checkbox-label">
                <input
                    type="checkbox"
                    prop:checked=move || checked.get()
                    on:change=on_change
                />
                {" "}{label_text}
            </label>
        </div>
    }
}

fn render_text_input(
    cfg: &serde_json::Value,
    label: &str,
    cell_id: Option<String>,
    widget_ctx: Option<WidgetContext>,
) -> impl IntoView {
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
        String::new()
    } else {
        label.to_owned()
    };

    let value = RwSignal::new(default);

    #[cfg(feature = "hydrate")]
    let on_input = {
        let cell_id = cell_id.clone();
        move |ev: web_sys::Event| {
            let new_val = leptos::prelude::event_target_value(&ev);
            value.set(new_val.clone());
            let bytes = bincode_encode_string(&new_val);
            update_cell_output(bytes, &cell_id, widget_ctx);
        }
    };

    #[cfg(not(feature = "hydrate"))]
    let on_input = move |_: leptos::ev::Event| {};
    let _ = (&cell_id, &widget_ctx);

    view! {
        <div class="ironpad-interactive-widget">
            {if !label_text.is_empty() {
                view! { <span class="ironpad-widget-label">{label_text}</span> }.into_any()
            } else {
                view! { <span /> }.into_any()
            }}
            <input
                type="text"
                placeholder=placeholder
                prop:value=move || value.get()
                on:input=on_input
            />
        </div>
    }
}

fn render_number(
    cfg: &serde_json::Value,
    label: &str,
    cell_id: Option<String>,
    widget_ctx: Option<WidgetContext>,
) -> impl IntoView {
    let min = cfg.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0);
    let max = cfg.get("max").and_then(|v| v.as_f64()).unwrap_or(100.0);
    let step = cfg.get("step").and_then(|v| v.as_f64()).unwrap_or(1.0);
    let default = cfg.get("default").and_then(|v| v.as_f64()).unwrap_or(min);
    let label_text = if label.is_empty() {
        String::new()
    } else {
        label.to_owned()
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
                update_cell_output(bytes, &cell_id, widget_ctx);
            }
        }
    };

    #[cfg(not(feature = "hydrate"))]
    let on_input = move |_: leptos::ev::Event| {};
    let _ = (&cell_id, &widget_ctx);

    view! {
        <div class="ironpad-interactive-widget">
            {if !label_text.is_empty() {
                view! { <span class="ironpad-widget-label">{label_text}</span> }.into_any()
            } else {
                view! { <span /> }.into_any()
            }}
            <input
                type="number"
                min=min.to_string()
                max=max.to_string()
                step=step.to_string()
                prop:value=move || value.get()
                on:input=on_input
            />
        </div>
    }
}

fn render_switch(
    cfg: &serde_json::Value,
    label: &str,
    cell_id: Option<String>,
    widget_ctx: Option<WidgetContext>,
) -> impl IntoView {
    let default = cfg
        .get("default")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let label_text = label.to_owned();

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
                update_cell_output(bytes, &cell_id, widget_ctx);
            }
        }
    };

    #[cfg(not(feature = "hydrate"))]
    let on_change = move |_: leptos::ev::Event| {};
    let _ = (&cell_id, &widget_ctx);

    view! {
        <div class="ironpad-interactive-widget">
            <label class="ironpad-switch">
                <input
                    type="checkbox"
                    prop:checked=move || checked.get()
                    on:change=on_change
                />
                <span class="ironpad-switch-slider"></span>
                {" "}{label_text}
            </label>
        </div>
    }
}

fn render_button(
    _cfg: &serde_json::Value,
    label: &str,
    cell_id: Option<String>,
    widget_ctx: Option<WidgetContext>,
) -> impl IntoView {
    let button_label = if label.is_empty() {
        "Run ▶".to_owned()
    } else {
        label.to_owned()
    };

    #[cfg(feature = "hydrate")]
    let on_click = {
        let cell_id = cell_id.clone();
        move |_: web_sys::MouseEvent| {
            let Some(cid) = &cell_id else { return };
            let Some(ctx) = widget_ctx else { return };

            let all_cells = ctx.cells.get_untracked();
            if let Some(my_idx) = all_cells.iter().position(|c| c.id == *cid) {
                let downstream: Vec<String> = all_cells[my_idx + 1..]
                    .iter()
                    .filter(|c| c.cell_type == CellType::Code)
                    .map(|c| c.id.clone())
                    .collect();
                if !downstream.is_empty() {
                    ctx.run_all_queue.set(downstream);
                }
            }
        }
    };

    #[cfg(not(feature = "hydrate"))]
    let on_click = move |_: web_sys::MouseEvent| {};
    let _ = (&cell_id, &widget_ctx);

    view! {
        <div class="ironpad-interactive-widget">
            <button
                class="ironpad-widget-button"
                on:click=on_click
            >
                {button_label}
            </button>
        </div>
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
