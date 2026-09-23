use std::collections::HashMap;

use crate::components::icon::{Chevron, IconLabel};
use crate::components::icons;
use ironpad_common::{CellManifest, Diagnostic, ExecutionResult, Severity};
use leptos::prelude::*;

use crate::components::error_panel::ErrorPanel;
use crate::components::output_render::{
    render_display_panel, DisplayPanel, PanelMode, WidgetSink, EDITOR_PANEL_CLASSES,
};

use super::pipeline::CompileSummary;
use super::state::{CellOutputData, CellStatus};

// ── Compile result panel ─────────────────────────────────────────────────────

/// Displays compilation results below a cell: success info or error diagnostics.
///
/// Hidden when the cell has not been compiled yet (Idle state).
/// On success, shows a summary line with optional warnings.
/// On error, delegates to the dedicated [`ErrorPanel`] component (T-031).
#[component]
pub(super) fn CompileResultPanel(
    cell_status: RwSignal<CellStatus>,
    last_compile: RwSignal<Option<CompileSummary>>,
    compile_time_ms: RwSignal<Option<f64>>,
    execution_result: RwSignal<Option<ExecutionResult>>,
) -> impl IntoView {
    view! {
        {move || {
            let status = cell_status.get();

            // Hide panel when idle, queued, or compiling (spinner is shown in the header).
            if matches!(status, CellStatus::Idle | CellStatus::Queued | CellStatus::Compiling | CellStatus::Running) {
                return view! { <div /> }.into_any();
            }

            match status {
                CellStatus::Success => {
                    // Borrowed: only the scalars and the warnings are needed,
                    // never a clone of the whole summary or execution result.
                    let Some((blob_size, cached, warnings)) = last_compile.with(|compile| {
                        compile.as_ref().map(|summary| {
                            let warnings: Vec<Diagnostic> = summary
                                .diagnostics
                                .iter()
                                .filter(|d| d.severity == Severity::Warning)
                                .cloned()
                                .collect();
                            (summary.blob_len, summary.cached, warnings)
                        })
                    }) else {
                        return view! { <div /> }.into_any();
                    };
                    let time = compile_time_ms.get().unwrap_or(0.0);

                    let runtime_ms = execution_result
                        .with(|result| result.as_ref().map(|r| r.execution_time_ms));

                    // Precision loss acceptable for display sizing.
                    #[allow(clippy::cast_precision_loss)]
                    let summary = format!(
                        "Compiled ({:.1} KB, {time:.0}ms compile{}{})",
                        blob_size as f64 / 1024.0,
                        match runtime_ms {
                            Some(r) => format!(", {r:.0}ms runtime"),
                            None => String::new(),
                        },
                        if cached { ", cached" } else { "" },
                    );

                    view! {
                        <div class="ironpad-compile-result ironpad-compile-result--success">
                            <div class="ironpad-compile-result-summary">
                                {summary}
                            </div>
                            {if warnings.is_empty() {
                                view! { <div /> }.into_any()
                            } else {
                                view! {
                                    <ErrorPanel diagnostics=warnings />
                                }.into_any()
                            }}
                        </div>
                    }.into_any()
                }

                CellStatus::Error => {
                    let Some(diagnostics) = last_compile
                        .with(|compile| compile.as_ref().map(|summary| summary.diagnostics.clone()))
                    else {
                        return view! { <div /> }.into_any();
                    };

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
    /// Live collapse state, owned by the cell so the header's default-collapse
    /// toggle can snap it. Falls back to a panel-local signal when absent
    /// (the read-only viewer's usage).
    #[prop(optional)]
    collapsed: Option<RwSignal<bool>>,
) -> impl IntoView {
    // Build the widget side-effect sink if all required signals are present.
    // The editor supplies `cell_stale` so a widget change marks downstream cells
    // stale for reactive re-execution (the read-only viewer leaves it `None`).
    let widget_sink = match (cell_outputs, cell_stale, cells, run_all_queue) {
        (Some(cell_outputs), Some(cell_stale), Some(cells), Some(run_all_queue)) => {
            Some(WidgetSink {
                cell_outputs,
                run_all_queue,
                cells: Signal::derive(move || {
                    cells.with(|cells| {
                        cells
                            .iter()
                            .map(|c| (c.id.clone(), c.is_runnable()))
                            .collect()
                    })
                }),
                cell_stale: Some(cell_stale),
            })
        }
        _ => None,
    };

    let output_collapsed = collapsed.unwrap_or_else(|| RwSignal::new(false));

    view! {
        {move || {
            let collapsed = output_collapsed.get();
            // Everything the panel shows, read through a borrow: cloning the
            // result (and its output bytes, twice more) on every render,
            // including each collapse toggle, bought nothing. The hex dump
            // is formatted here, and only when the body is showing it.
            let Some((time_ms, byte_count, ran_on_main_thread, panels, hex)) =
                execution_result.with(|result| {
                    result.as_ref().map(|result| {
                        // Parse display panels from JSON, with backward-compat fallback.
                        let panels: Vec<DisplayPanel> = match &result.display_text {
                            Some(json) => serde_json::from_str(json).unwrap_or_else(|_| {
                                vec![DisplayPanel::Text(json.clone())]
                            }),
                            None => vec![],
                        };
                        let hex = (!collapsed && !result.output_bytes.is_empty())
                            .then(|| format_hex_dump(&result.output_bytes));
                        (
                            result.execution_time_ms,
                            result.output_bytes.len(),
                            result.ran_on_main_thread,
                            panels,
                            hex,
                        )
                    })
                })
            else {
                return view! { <div /> }.into_any();
            };

            let panel_class = if collapsed {
                "ironpad-output-panel ironpad-output-panel--collapsed"
            } else {
                "ironpad-output-panel"
            };

            view! {
                <div class=panel_class>
                    <div
                        class="ironpad-output-header"
                        on:click=move |_| output_collapsed.update(|c| *c = !*c)
                    >
                        <span class="ironpad-output-toggle"><Chevron expanded=Signal::derive(move || !output_collapsed.get())/></span>
                        <span class="ironpad-output-title">"Output"</span>
                        <span class="ironpad-output-meta">
                            {format!("{byte_count} byte{} · {time_ms:.1}ms", if byte_count == 1 { "" } else { "s" })}
                        </span>
                        {if ran_on_main_thread {
                            view! {
                                <span
                                    class="ironpad-output-fallback-badge"
                                    title="This cell was re-executed on the main thread because it requires DOM access (e.g. plotters font measurement)"
                                >
                                    <IconLabel icon=icons::WARNING label="main thread"/>
                                </span>
                            }.into_any()
                        } else {
                            view! { <span /> }.into_any()
                        }}
                    </div>

                    {if collapsed {
                        view! { <div /> }.into_any()
                    } else {
                        view! {
                            <div class="ironpad-output-body">
                                // Display panels section (shared with the read-only viewer).
                                {panels.into_iter().map(|panel| {
                                    render_display_panel(panel, EDITOR_PANEL_CLASSES, cell_id.clone(), widget_sink, PanelMode::Live)
                                }).collect::<Vec<_>>()}

                                // Raw bytes hex dump section.
                                {if let Some(hex) = hex {
                                    view! {
                                        <details class="ironpad-output-bytes">
                                            <summary class="ironpad-output-bytes-header">
                                                {format!("Raw output ({byte_count} bytes)")}
                                            </summary>
                                            <pre class="ironpad-output-hex-dump">{hex}</pre>
                                        </details>
                                    }.into_any()
                                } else {
                                    view! { <div /> }.into_any()
                                }}
                            </div>
                        }.into_any()
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
///
/// Only the first `MAX_DUMP_BYTES` bytes are shown; a truncation note is
/// appended when the input exceeds this limit.
fn format_hex_dump(data: &[u8]) -> String {
    const BYTES_PER_ROW: usize = 16;
    const MAX_DUMP_BYTES: usize = 1024;

    let truncated = data.len() > MAX_DUMP_BYTES;
    let display_data = &data[..data.len().min(MAX_DUMP_BYTES)];

    let mut lines = Vec::new();

    for (i, chunk) in display_data.chunks(BYTES_PER_ROW).enumerate() {
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

    if truncated {
        lines.push(format!(
            "\n... truncated ({} of {} bytes shown)",
            MAX_DUMP_BYTES,
            data.len()
        ));
    }

    lines.join("\n")
}
