//! Shared cell-output rendering.
//!
//! Both the notebook editor (`pages/notebook_editor/cell_output.rs`) and the
//! read-only viewer (`components/view_only_notebook.rs`) render the same
//! execution output: a list of [`DisplayPanel`]s plus interactive widgets. This
//! module holds the single copy of that pipeline so a rendering or sanitization
//! fix reaches both paths.
//!
//! The two consumers differ in only two ways, both parameterized here:
//!   * their CSS class names — captured by [`PanelClasses`];
//!   * whether a widget value change marks downstream cells stale to drive
//!     reactive re-execution — captured by [`WidgetSink`] (`Some` `cell_stale`
//!     in the editor, `None` in the viewer).

use std::collections::HashMap;
use std::fmt::Write;

use ironpad_common::CellType;
use leptos::prelude::*;

use crate::components::animation_canvas::{AnimationCanvas, SimSliderMeta, SimulationCanvas};
use crate::components::blob_url::{create_blob_url, revoke_blob_url};
use crate::components::copy_button::CopyButton;
use crate::components::live_view_panel::LiveViewPanel;
use crate::components::markdown_cell::render_markdown;

// ── Display panels ──────────────────────────────────────────────────────────

/// App-side mirror of `ironpad_cell::DisplayPanel`.
///
/// Kept separate from the cell crate so the UI doesn't depend on the cell
/// runtime; deserialized from the JSON a cell emits as its display text.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub(crate) enum DisplayPanel {
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
    /// Binary image data (base64-encoded BMP/PNG), rendered via a Blob URL.
    BlobImage {
        mime_type: String,
        base64_data: String,
        width: u32,
        height: u32,
    },
    /// Multi-frame animation (base64-encoded concatenated RGB bytes).
    Animation {
        width: u32,
        height: u32,
        fps: u32,
        frame_count: u32,
        data: String,
    },
    /// Live simulation placeholder (base64-encoded first-frame RGB bytes).
    Simulation {
        width: u32,
        height: u32,
        fps: u32,
        first_frame_data: String,
        sliders: Vec<SimSliderMeta>,
    },
    /// Live view placeholder (initial content + fps + content kind).
    LiveView {
        fps: u32,
        kind: String,
        content: String,
    },
}

// ── Per-cell output data ────────────────────────────────────────────────────

/// Cached output bytes + type tag from a cell execution, keyed by cell ID and
/// used to pipe one cell's output into the next cell's input.
// Fields are accessed only under #[cfg(feature = "hydrate")]; appear dead in SSR.
#[derive(Clone, Default, Debug)]
#[allow(dead_code)]
pub struct CellOutputData {
    pub(crate) bytes: Vec<u8>,
    pub(crate) type_tag: Option<String>,
}

// ── Escaping / table helpers ────────────────────────────────────────────────

/// Minimal HTML entity escaping for text content.
pub(crate) fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render a table as an HTML `<table>` string with the `ironpad-output-table` class.
pub(crate) fn render_table_html(headers: &[String], rows: &[Vec<String>]) -> String {
    let mut html = String::from("<table class=\"ironpad-output-table\"><thead><tr>");
    for h in headers {
        let _ = write!(html, "<th>{}</th>", html_escape(h));
    }
    html.push_str("</tr></thead><tbody>");
    for row in rows {
        html.push_str("<tr>");
        for cell in row {
            let _ = write!(html, "<td>{}</td>", html_escape(cell));
        }
        html.push_str("</tr>");
    }
    html.push_str("</tbody></table>");
    html
}

/// Render a table as tab-separated values for clipboard copy.
pub(crate) fn render_table_tsv(headers: &[String], rows: &[Vec<String>]) -> String {
    let mut tsv = headers.join("\t");
    for row in rows {
        tsv.push('\n');
        tsv.push_str(&row.join("\t"));
    }
    tsv
}

// ── Bincode encoding helpers (hydrate-only) ─────────────────────────────────

/// Encode an `f64` value to bincode v2 standard-config bytes.
#[cfg(feature = "hydrate")]
pub(crate) fn bincode_encode_f64(value: f64) -> Vec<u8> {
    bincode::encode_to_vec(value, bincode::config::standard()).expect("f64 encoding cannot fail")
}

/// Encode a `bool` value to bincode v2 standard-config bytes.
#[cfg(feature = "hydrate")]
pub(crate) fn bincode_encode_bool(value: bool) -> Vec<u8> {
    bincode::encode_to_vec(value, bincode::config::standard()).expect("bool encoding cannot fail")
}

/// Encode a `String` value to bincode v2 standard-config bytes.
#[cfg(feature = "hydrate")]
pub(crate) fn bincode_encode_string(value: &str) -> Vec<u8> {
    bincode::encode_to_vec(value, bincode::config::standard()).expect("String encoding cannot fail")
}

// ── Sim bus JS bridge (hydrate-only) ─────────────────────────────────────────

#[cfg(feature = "hydrate")]
mod sim_bus_js {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(inline_js = "
        export function sim_bus_write_f64(key, value) {
            if (window.IronpadExecutor && window.IronpadExecutor.simBusWrite) {
                window.IronpadExecutor.simBusWrite(key, value);
            }
        }
        export function sim_bus_write_bool(key, value) {
            if (window.IronpadExecutor && window.IronpadExecutor.simBusWrite) {
                window.IronpadExecutor.simBusWrite(key, value ? 1.0 : 0.0);
            }
        }
    ")]
    extern "C" {
        pub fn sim_bus_write_f64(key: &str, value: f64);
        pub fn sim_bus_write_bool(key: &str, value: bool);
    }
}

// ── Widget side-effect sink ──────────────────────────────────────────────────

/// Bundles the reactive signals an interactive widget writes to on value
/// change. Parameterizes the one behavioral difference between the editor and
/// read-only widget paths: `cell_stale` is `Some` in the editor (a value change
/// marks downstream cells stale to drive reactive re-execution) and `None` in
/// the viewer (which has no reactive runner and updates outputs directly).
// Fields are accessed only under #[cfg(feature = "hydrate")]; appear dead in SSR.
#[derive(Clone, Copy)]
#[allow(dead_code)]
pub(crate) struct WidgetSink {
    /// Cell outputs map, updated with the new bytes on a value change.
    pub(crate) cell_outputs: RwSignal<HashMap<String, CellOutputData>>,
    /// Run-all queue; the button widget pushes downstream cell IDs here.
    pub(crate) run_all_queue: RwSignal<Vec<String>>,
    /// Ordered `(id, cell_type)` for every cell, used to find strictly-downstream
    /// Code cells (button widget) and to mark them stale.
    pub(crate) cells: Signal<Vec<(String, CellType)>>,
    /// When `Some`, a value change marks downstream Code cells stale (editor
    /// reactive mode). `None` in the read-only viewer.
    pub(crate) cell_stale: Option<RwSignal<HashMap<String, bool>>>,
}

/// Update cell outputs and — in the editor — mark downstream cells stale after
/// a widget value change.
#[cfg(feature = "hydrate")]
fn update_cell_output(new_bytes: Vec<u8>, cell_id: Option<&str>, sink: Option<WidgetSink>) {
    let Some(cid) = cell_id else { return };
    let Some(sink) = sink else { return };

    sink.cell_outputs.update(|map| {
        if let Some(data) = map.get_mut(cid) {
            data.bytes = new_bytes;
        }
    });

    // Mark strictly-downstream Code cells stale so the reactive runner
    // re-executes the cells that consume this widget's value. We skip the
    // widget's own cell: its output is set directly above.
    let Some(cell_stale) = sink.cell_stale else {
        return;
    };
    let cells = sink.cells.get_untracked();
    let my_idx = cells.iter().position(|(id, _)| id == cid).unwrap_or(0);
    cell_stale.update(|map| {
        for (id, cell_type) in cells.iter().skip(my_idx + 1) {
            if *cell_type == CellType::Code {
                map.insert(id.clone(), true);
            }
        }
    });
}

// ── Display panel rendering ──────────────────────────────────────────────────

/// CSS class names for the per-panel wrappers. The editor and the read-only
/// viewer use different class prefixes; panel rendering is otherwise identical.
#[derive(Clone, Copy)]
pub(crate) struct PanelClasses {
    /// Base wrapper class for text/markdown/table/animation/simulation/liveview.
    pub(crate) display: &'static str,
    /// Class for the `<pre>` of a `Text` panel.
    pub(crate) text: &'static str,
    /// Wrapper class for an `Html` panel.
    pub(crate) html: &'static str,
    /// Wrapper class for an `Svg` panel.
    pub(crate) svg: &'static str,
    /// Wrapper class for a `BlobImage` panel.
    pub(crate) visual: &'static str,
}

/// Panel classes for the notebook editor's output panel.
pub(crate) const EDITOR_PANEL_CLASSES: PanelClasses = PanelClasses {
    display: "ironpad-output-display",
    text: "ironpad-output-display-text",
    html: "ironpad-output-display ironpad-output-html",
    svg: "ironpad-output-display ironpad-output-svg",
    visual: "ironpad-output-display ironpad-output-visual",
};

/// Panel classes for the read-only notebook viewer.
pub(crate) const VIEW_ONLY_PANEL_CLASSES: PanelClasses = PanelClasses {
    display: "view-only-output-display",
    text: "view-only-output-text",
    html: "view-only-output-display view-only-output-html",
    svg: "view-only-output-display view-only-output-svg",
    visual: "view-only-output-display view-only-output-visual",
};

/// Render a single [`DisplayPanel`] into a view, using the given class set and
/// widget side-effect sink. Shared by the editor and the read-only viewer.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn render_display_panel(
    panel: DisplayPanel,
    classes: PanelClasses,
    cell_id: Option<String>,
    sink: Option<WidgetSink>,
) -> AnyView {
    match panel {
        DisplayPanel::Text(text) => {
            let copy_text = text.clone();
            view! {
                <div class=classes.display>
                    <CopyButton text=copy_text />
                    <pre class=classes.text>{text}</pre>
                </div>
            }
            .into_any()
        }
        DisplayPanel::Html(html) => {
            let copy_text = html.clone();
            // Sanitize before inner_html: cell output is untrusted and
            // shared/public notebooks are viewed by others.
            let safe = crate::sanitize::sanitize_html(&html);
            view! {
                <div class=classes.html>
                    <CopyButton text=copy_text />
                    <div inner_html=safe></div>
                </div>
            }
            .into_any()
        }
        DisplayPanel::Svg(svg) => {
            let copy_text = svg.clone();
            let safe = crate::sanitize::sanitize_svg(&svg);
            view! {
                <div class=classes.svg>
                    <CopyButton text=copy_text />
                    <div inner_html=safe></div>
                </div>
            }
            .into_any()
        }
        DisplayPanel::Markdown(md) => {
            let copy_text = md.clone();
            let rendered = render_markdown(&md);
            view! {
                <div class=classes.display>
                    <CopyButton text=copy_text />
                    <div class="ironpad-markdown-cell-preview" inner_html=rendered></div>
                </div>
            }
            .into_any()
        }
        DisplayPanel::Table { headers, rows } => {
            let copy_text = render_table_tsv(&headers, &rows);
            let table_html = render_table_html(&headers, &rows);
            view! {
                <div class=classes.display>
                    <CopyButton text=copy_text />
                    <div inner_html=table_html></div>
                </div>
            }
            .into_any()
        }
        DisplayPanel::Interactive { kind, config } => view! {
            <InteractiveWidget kind=kind config=config cell_id=cell_id sink=sink />
        }
        .into_any(),
        DisplayPanel::BlobImage {
            mime_type,
            base64_data,
            width,
            height,
        } => {
            let blob_url = create_blob_url(&base64_data, &mime_type).unwrap_or_default();

            // Revoke the Blob URL when this reactive scope is disposed to free
            // browser memory.
            let url_for_cleanup = blob_url.clone();
            on_cleanup(move || {
                revoke_blob_url(&url_for_cleanup);
            });

            view! {
                <div class=classes.visual>
                    <img
                        src=blob_url
                        width=width
                        height=height
                        style="image-rendering: pixelated;"
                    />
                </div>
            }
            .into_any()
        }
        DisplayPanel::Animation {
            width,
            height,
            fps,
            frame_count,
            data,
        } => view! {
            <div class=classes.display>
                <AnimationCanvas width=width height=height fps=fps frame_count=frame_count data=data />
            </div>
        }
        .into_any(),
        DisplayPanel::Simulation {
            width,
            height,
            fps,
            first_frame_data,
            sliders,
        } => {
            let cid = cell_id.clone().unwrap_or_default();
            view! {
                <div class=classes.display>
                    <SimulationCanvas width=width height=height fps=fps first_frame_data=first_frame_data cell_id=cid sliders=sliders />
                </div>
            }
            .into_any()
        }
        DisplayPanel::LiveView { fps, kind, content } => {
            let cid = cell_id.clone().unwrap_or_default();
            view! {
                <div class=classes.display>
                    <LiveViewPanel fps=fps kind=kind content=content cell_id=cid />
                </div>
            }
            .into_any()
        }
    }
}

// ── Interactive widget component ─────────────────────────────────────────────

/// Renders an interactive UI widget (slider, dropdown, checkbox, etc.) with
/// live value-change callbacks that update cell outputs (and, in the editor,
/// mark downstream cells stale via [`WidgetSink`]).
#[component]
fn InteractiveWidget(
    #[prop(into)] kind: String,
    #[prop(into)] config: String,
    cell_id: Option<String>,
    sink: Option<WidgetSink>,
) -> impl IntoView {
    let cfg: serde_json::Value = serde_json::from_str(&config).unwrap_or_default();
    let label = cfg
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();

    match kind.as_str() {
        "slider" => render_slider(&cfg, &label, cell_id, sink).into_any(),
        "dropdown" => render_dropdown(&cfg, &label, cell_id, sink).into_any(),
        "checkbox" => render_checkbox(&cfg, &label, cell_id, sink).into_any(),
        "text_input" => render_text_input(&cfg, &label, cell_id, sink).into_any(),
        "number" => render_number(&cfg, &label, cell_id, sink).into_any(),
        "switch" => render_switch(&cfg, &label, cell_id, sink).into_any(),
        "button" => render_button(&cfg, &label, cell_id, sink).into_any(),
        "progress" => render_progress(&cfg, &label).into_any(),
        _ => view! {
            <div class="ironpad-interactive-widget">
                <span class="ironpad-widget-label">{format!("[unknown widget: {kind}]")}</span>
            </div>
        }
        .into_any(),
    }
}

/// The optional leading label span every interactive widget renders — one
/// definition instead of four copies in the widget builders.
fn widget_label(label_text: String) -> AnyView {
    if label_text.is_empty() {
        view! { <span /> }.into_any()
    } else {
        view! { <span class="ironpad-widget-label">{label_text}</span> }.into_any()
    }
}

#[allow(clippy::needless_pass_by_value)]
fn render_slider(
    cfg: &serde_json::Value,
    label: &str,
    cell_id: Option<String>,
    sink: Option<WidgetSink>,
) -> impl IntoView {
    let min = cfg
        .get("min")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    let max = cfg
        .get("max")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(100.0);
    let step = cfg
        .get("step")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0);
    let default = cfg
        .get("default")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(min);
    let label_text = if label.is_empty() {
        String::new()
    } else {
        label.to_owned()
    };
    let bus_key = cfg.get("name").and_then(|v| v.as_str()).map(str::to_owned);

    let value = RwSignal::new(default.to_string());

    #[cfg(feature = "hydrate")]
    {
        if let Some(key) = bus_key.clone() {
            Effect::new(move |_| {
                sim_bus_js::sim_bus_write_f64(&key, default);
            });
        }
    }

    #[cfg(feature = "hydrate")]
    let on_input = {
        let cell_id = cell_id.clone();
        let bus_key = bus_key.clone();
        move |ev: web_sys::Event| {
            let new_val = leptos::prelude::event_target_value(&ev);
            value.set(new_val.clone());
            if let Ok(f) = new_val.parse::<f64>() {
                let bytes = bincode_encode_f64(f);
                update_cell_output(bytes, cell_id.as_deref(), sink);
                if let Some(ref key) = bus_key {
                    sim_bus_js::sim_bus_write_f64(key, f);
                }
            }
        }
    };

    // Suppress unused warnings during SSR.
    #[cfg(not(feature = "hydrate"))]
    let on_input = move |_: leptos::ev::Event| {};
    let _ = (&cell_id, &sink, &bus_key);

    view! {
        <div class="ironpad-interactive-widget">
            {widget_label(label_text)}
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

#[allow(clippy::needless_pass_by_value)]
fn render_dropdown(
    cfg: &serde_json::Value,
    label: &str,
    cell_id: Option<String>,
    sink: Option<WidgetSink>,
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
    let bus_key = cfg.get("name").and_then(|v| v.as_str()).map(str::to_owned);

    let options_for_bus = options.clone();

    let value = RwSignal::new(default.clone());

    #[cfg(feature = "hydrate")]
    {
        if let Some(key) = bus_key.clone() {
            #[allow(clippy::cast_precision_loss)]
            let default_idx = options_for_bus
                .iter()
                .position(|o| *o == default)
                .unwrap_or(0) as f64;
            Effect::new(move |_| {
                sim_bus_js::sim_bus_write_f64(&key, default_idx);
            });
        }
    }

    #[cfg(feature = "hydrate")]
    let on_change = {
        let cell_id = cell_id.clone();
        let bus_key = bus_key.clone();
        let options_for_bus = options_for_bus.clone();
        move |ev: web_sys::Event| {
            let new_val = leptos::prelude::event_target_value(&ev);
            value.set(new_val.clone());
            let bytes = bincode_encode_string(&new_val);
            update_cell_output(bytes, cell_id.as_deref(), sink);
            if let Some(ref key) = bus_key {
                #[allow(clippy::cast_precision_loss)]
                let idx = options_for_bus
                    .iter()
                    .position(|o| *o == new_val)
                    .unwrap_or(0) as f64;
                sim_bus_js::sim_bus_write_f64(key, idx);
            }
        }
    };

    #[cfg(not(feature = "hydrate"))]
    let on_change = move |_: leptos::ev::Event| {};
    let _ = (&cell_id, &sink, &bus_key, &options_for_bus);

    view! {
        <div class="ironpad-interactive-widget">
            {widget_label(label_text)}
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

#[allow(clippy::needless_pass_by_value)]
fn render_checkbox(
    cfg: &serde_json::Value,
    label: &str,
    cell_id: Option<String>,
    sink: Option<WidgetSink>,
) -> impl IntoView {
    let default = cfg
        .get("default")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let label_text = label.to_owned();
    let bus_key = cfg.get("name").and_then(|v| v.as_str()).map(str::to_owned);

    let checked = RwSignal::new(default);

    #[cfg(feature = "hydrate")]
    {
        if let Some(key) = bus_key.clone() {
            Effect::new(move |_| {
                sim_bus_js::sim_bus_write_bool(&key, default);
            });
        }
    }

    #[cfg(feature = "hydrate")]
    let on_change = {
        let cell_id = cell_id.clone();
        let bus_key = bus_key.clone();
        move |ev: web_sys::Event| {
            use wasm_bindgen::JsCast;
            if let Some(input) = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
            {
                let new_val = input.checked();
                checked.set(new_val);
                let bytes = bincode_encode_bool(new_val);
                update_cell_output(bytes, cell_id.as_deref(), sink);
                if let Some(ref key) = bus_key {
                    sim_bus_js::sim_bus_write_bool(key, new_val);
                }
            }
        }
    };

    #[cfg(not(feature = "hydrate"))]
    let on_change = move |_: leptos::ev::Event| {};
    let _ = (&cell_id, &sink, &bus_key);

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

#[allow(clippy::needless_pass_by_value)]
fn render_text_input(
    cfg: &serde_json::Value,
    label: &str,
    cell_id: Option<String>,
    sink: Option<WidgetSink>,
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
            update_cell_output(bytes, cell_id.as_deref(), sink);
        }
    };

    #[cfg(not(feature = "hydrate"))]
    let on_input = move |_: leptos::ev::Event| {};
    let _ = (&cell_id, &sink);

    view! {
        <div class="ironpad-interactive-widget">
            {widget_label(label_text)}
            <input
                type="text"
                placeholder=placeholder
                prop:value=move || value.get()
                on:input=on_input
            />
        </div>
    }
}

#[allow(clippy::needless_pass_by_value)]
fn render_number(
    cfg: &serde_json::Value,
    label: &str,
    cell_id: Option<String>,
    sink: Option<WidgetSink>,
) -> impl IntoView {
    let min = cfg
        .get("min")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    let max = cfg
        .get("max")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(100.0);
    let step = cfg
        .get("step")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(1.0);
    let default = cfg
        .get("default")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(min);
    let label_text = if label.is_empty() {
        String::new()
    } else {
        label.to_owned()
    };
    let bus_key = cfg.get("name").and_then(|v| v.as_str()).map(str::to_owned);

    let value = RwSignal::new(default.to_string());

    #[cfg(feature = "hydrate")]
    {
        if let Some(key) = bus_key.clone() {
            Effect::new(move |_| {
                sim_bus_js::sim_bus_write_f64(&key, default);
            });
        }
    }

    #[cfg(feature = "hydrate")]
    let on_input = {
        let cell_id = cell_id.clone();
        let bus_key = bus_key.clone();
        move |ev: web_sys::Event| {
            let new_val = leptos::prelude::event_target_value(&ev);
            value.set(new_val.clone());
            if let Ok(f) = new_val.parse::<f64>() {
                let bytes = bincode_encode_f64(f);
                update_cell_output(bytes, cell_id.as_deref(), sink);
                if let Some(ref key) = bus_key {
                    sim_bus_js::sim_bus_write_f64(key, f);
                }
            }
        }
    };

    #[cfg(not(feature = "hydrate"))]
    let on_input = move |_: leptos::ev::Event| {};
    let _ = (&cell_id, &sink, &bus_key);

    view! {
        <div class="ironpad-interactive-widget">
            {widget_label(label_text)}
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

#[allow(clippy::needless_pass_by_value)]
fn render_switch(
    cfg: &serde_json::Value,
    label: &str,
    cell_id: Option<String>,
    sink: Option<WidgetSink>,
) -> impl IntoView {
    let default = cfg
        .get("default")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let label_text = label.to_owned();
    let bus_key = cfg.get("name").and_then(|v| v.as_str()).map(str::to_owned);

    let checked = RwSignal::new(default);

    #[cfg(feature = "hydrate")]
    {
        if let Some(key) = bus_key.clone() {
            Effect::new(move |_| {
                sim_bus_js::sim_bus_write_bool(&key, default);
            });
        }
    }

    #[cfg(feature = "hydrate")]
    let on_change = {
        let cell_id = cell_id.clone();
        let bus_key = bus_key.clone();
        move |ev: web_sys::Event| {
            use wasm_bindgen::JsCast;
            if let Some(input) = ev
                .target()
                .and_then(|t| t.dyn_into::<web_sys::HtmlInputElement>().ok())
            {
                let new_val = input.checked();
                checked.set(new_val);
                let bytes = bincode_encode_bool(new_val);
                update_cell_output(bytes, cell_id.as_deref(), sink);
                if let Some(ref key) = bus_key {
                    sim_bus_js::sim_bus_write_bool(key, new_val);
                }
            }
        }
    };

    #[cfg(not(feature = "hydrate"))]
    let on_change = move |_: leptos::ev::Event| {};
    let _ = (&cell_id, &sink, &bus_key);

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

#[allow(clippy::needless_pass_by_value)]
fn render_button(
    _cfg: &serde_json::Value,
    label: &str,
    cell_id: Option<String>,
    sink: Option<WidgetSink>,
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
            let Some(sink) = sink else { return };

            let all_cells = sink.cells.get_untracked();
            if let Some(my_idx) = all_cells.iter().position(|(id, _)| id == cid) {
                let downstream: Vec<String> = all_cells[my_idx + 1..]
                    .iter()
                    .filter(|(_, cell_type)| *cell_type == CellType::Code)
                    .map(|(id, _)| id.clone())
                    .collect();
                if !downstream.is_empty() {
                    sink.run_all_queue.set(downstream);
                }
            }
        }
    };

    #[cfg(not(feature = "hydrate"))]
    let on_click = move |_: leptos::web_sys::MouseEvent| {};
    let _ = (&cell_id, &sink);

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

fn render_progress(cfg: &serde_json::Value, label: &str) -> impl IntoView {
    let id = cfg
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let initial = cfg
        .get("initial")
        .and_then(serde_json::Value::as_f64)
        .unwrap_or(0.0);
    let pct = initial.clamp(0.0, 100.0);
    // Cast is safe: pct is clamped to [0.0, 100.0].
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let pct_int = pct as u32;
    let width_style = format!("width: {pct}%");
    let label_text = if label.is_empty() {
        String::new()
    } else {
        label.to_owned()
    };

    view! {
        <div class="ironpad-interactive-widget">
            {if label_text.is_empty() {
                None
            } else {
                Some(view! { <span class="ironpad-widget-label">{label_text}</span> })
            }}
            <div class="ironpad-progress" data-progress-id={id}>
                <div class="ironpad-progress-bar">
                    <div class="ironpad-progress-fill" style={width_style}></div>
                </div>
                <span class="ironpad-progress-value">{format!("{pct_int}%")}</span>
            </div>
        </div>
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escape_replaces_all_entities() {
        assert_eq!(
            html_escape(r#"<a href="x">&y</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&amp;y&lt;/a&gt;"
        );
    }

    #[test]
    fn html_escape_passes_through_plain_text() {
        assert_eq!(html_escape("hello world"), "hello world");
    }

    #[test]
    fn render_table_html_escapes_and_wraps() {
        let headers = vec!["a".to_string(), "<b>".to_string()];
        let rows = vec![vec!["1".to_string(), "&2".to_string()]];
        let html = render_table_html(&headers, &rows);
        assert_eq!(
            html,
            "<table class=\"ironpad-output-table\"><thead><tr><th>a</th><th>&lt;b&gt;</th></tr></thead><tbody><tr><td>1</td><td>&amp;2</td></tr></tbody></table>"
        );
    }

    #[test]
    fn render_table_tsv_joins_with_tabs_and_newlines() {
        let headers = vec!["a".to_string(), "b".to_string()];
        let rows = vec![
            vec!["1".to_string(), "2".to_string()],
            vec!["3".to_string(), "4".to_string()],
        ];
        assert_eq!(render_table_tsv(&headers, &rows), "a\tb\n1\t2\n3\t4");
    }

    #[test]
    fn render_table_tsv_handles_no_rows() {
        let headers = vec!["a".to_string(), "b".to_string()];
        assert_eq!(render_table_tsv(&headers, &[]), "a\tb");
    }

    #[test]
    fn display_panel_deserializes_from_cell_json() {
        // Round-trips the wire format a cell emits for a text panel.
        let json = r#"[{"Text":"hello"}]"#;
        let panels: Vec<DisplayPanel> = serde_json::from_str(json).unwrap();
        assert_eq!(panels.len(), 1);
        assert!(matches!(panels[0], DisplayPanel::Text(ref t) if t == "hello"));
    }
}
