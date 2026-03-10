use ironpad_common::{CompileResponse, Diagnostic, ExecutionResult, Severity};
use leptos::prelude::*;

use crate::components::copy_button::CopyButton;
use crate::components::error_panel::ErrorPanel;
use crate::components::markdown_cell::render_markdown;

use super::export::{render_table_html, render_table_tsv, DisplayPanel};
use super::state::CellStatus;

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
) -> impl IntoView {
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
