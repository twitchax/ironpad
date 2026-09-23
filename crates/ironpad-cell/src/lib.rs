//! ironpad-cell — injected into every user cell as a dependency.
//!
//! Provides [`CellInput`], [`CellOutput`], [`CellResult`], and memory FFI
//! helpers (`ironpad_alloc` / `ironpad_dealloc`).  The [`prelude`] module
//! re-exports the essential items so user cells can simply write:
//!
//! ```ignore
//! use ironpad_cell::prelude::*;
//! ```

// FFI reclaim helpers intentionally use Vec::from_raw_parts(ptr, len, len)
// because every buffer we allocate has capacity == length.
#![allow(unknown_lints)]
#![allow(clippy::same_length_and_capacity)]

// ── Host messaging FFI ───────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    /// Send a JSON-encoded message to the host runtime.
    fn ironpad_host_message(ptr: *const u8, len: u32);
}

/// Send a JSON message to the host runtime during cell execution.
///
/// This is the generic channel for cell-to-host communication. Messages
/// are JSON strings dispatched by the executor based on the `"type"` field.
pub fn host_message(msg: &str) {
    #[cfg(target_arch = "wasm32")]
    {
        let bytes = msg.as_bytes();
        // `usize` is `u32` on wasm32 (the only target this block compiles for), so the cast is lossless.
        #[allow(clippy::cast_possible_truncation)]
        unsafe {
            ironpad_host_message(bytes.as_ptr(), bytes.len() as u32);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = msg;
        // No-op outside WASM — host messaging is only available in the browser runtime.
    }
}

/// Send a structured message to the host runtime.
///
/// Serializes `value` to JSON and sends it via [`host_message`].
pub fn host_message_json<T: serde::Serialize>(value: &T) {
    if let Ok(json) = serde_json::to_string(value) {
        host_message(&json);
    }
}

// ── Prelude ──────────────────────────────────────────────────────────────────

pub mod prelude {
    pub use bincode;
    pub use serde::{Deserialize, Serialize};

    #[cfg(target_arch = "wasm32")]
    pub use js_sys;
    #[cfg(all(target_arch = "wasm32", feature = "rayon"))]
    pub use rayon;
    #[cfg(all(target_arch = "wasm32", feature = "rayon"))]
    pub use rayon::prelude::*;
    #[cfg(target_arch = "wasm32")]
    pub use reqwest;
    #[cfg(target_arch = "wasm32")]
    pub use wasm_bindgen::prelude::*;
    #[cfg(target_arch = "wasm32")]
    pub use wasm_bindgen_futures;
    #[cfg(all(target_arch = "wasm32", feature = "rayon"))]
    pub use wasm_bindgen_rayon::init_thread_pool;

    pub use console_error_panic_hook;

    pub use crate::blocking;
    pub use crate::canvas::{Animation, Canvas};
    pub use crate::gpu::{gpu_available, GpuCanvas, GpuSimulation};
    pub use crate::plot::Plot;
    pub use crate::sim;
    pub use crate::timing::Stopwatch;
    pub use crate::ui;
    pub use crate::ui::{sim_slider, ProgressHandle, SimSlider};
    pub use crate::{host_message, host_message_json};
    pub use crate::{
        CellInput, CellInputs, CellOutput, CellResult, DisplayPanel, Html, IntoPanels, Json,
        LiveContent, LiveTickResult, LiveView, LiveViewMeta, Md, SimSliderMeta, Simulation,
        SimulationMeta, Svg, Table, TickResult, TypeTag,
    };

    #[cfg(target_arch = "wasm32")]
    pub use super::http;
}

pub mod blocking;
pub mod canvas;
pub mod gpu;
pub mod plot;
pub mod sim;
pub mod timing;
pub mod ui;

#[cfg(target_arch = "wasm32")]
pub mod enzyme_shims;
#[cfg(target_arch = "wasm32")]
pub mod http;

// ── CellInput ────────────────────────────────────────────────────────────────

/// Read-only wrapper around the raw bytes produced by the previous cell.
///
/// Cell 0 in a notebook always receives an empty slice.
pub struct CellInput<'a> {
    bytes: &'a [u8],
}

impl<'a> CellInput<'a> {
    /// Create a `CellInput` from a raw byte slice.
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    /// Deserialize the input bytes into `T` using bincode + serde.
    pub fn deserialize<T: serde::de::DeserializeOwned>(
        &self,
    ) -> Result<T, bincode::error::DecodeError> {
        let (value, _) =
            bincode::serde::decode_from_slice(self.bytes, bincode::config::standard())?;
        Ok(value)
    }

    /// Returns `true` when no data was passed from a preceding cell.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Access the underlying byte slice directly.
    pub fn raw(&self) -> &[u8] {
        self.bytes
    }
}

// ── CellInputs ───────────────────────────────────────────────────────────────

/// Container for all previous cell outputs, passed to a cell's `cell_main` function.
///
/// Uses a simple length-prefixed binary wire format:
/// `[u32 LE: count][u32 LE: len0][bytes0...][u32 LE: len1][bytes1...]...`
pub struct CellInputs {
    data: Vec<Vec<u8>>,
}

impl CellInputs {
    /// Decode from the length-prefixed wire format.
    /// If `bytes` is empty, returns an empty `CellInputs`.
    pub fn from_raw(bytes: &[u8]) -> Self {
        // Bounds-checked wire-format parse: `[count:u32]([len:u32][data:len])*`.
        // Every read goes through `slice::get` and a checked add, so a
        // truncated or malformed buffer (or an out-of-range count/len) yields
        // whatever parsed cleanly instead of panicking on an out-of-bounds index
        // or over-allocating from an attacker-controlled count.
        let mut data = Vec::new();

        let Some(count_bytes) = bytes.get(0..4) else {
            return Self { data };
        };
        let count = u32::from_le_bytes(count_bytes.try_into().unwrap()) as usize;
        let mut offset = 4usize;

        for _ in 0..count {
            // Length prefix.
            let Some(end) = offset.checked_add(4) else {
                break;
            };
            let Some(len_bytes) = bytes.get(offset..end) else {
                break;
            };
            let len = u32::from_le_bytes(len_bytes.try_into().unwrap()) as usize;
            offset = end;

            // Segment payload.
            let Some(seg_end) = offset.checked_add(len) else {
                break;
            };
            let Some(segment) = bytes.get(offset..seg_end) else {
                break;
            };
            data.push(segment.to_vec());
            offset = seg_end;
        }

        Self { data }
    }

    /// Encode a list of output byte slices into the wire format.
    /// This is used by the frontend to package all previous cell outputs.
    #[allow(clippy::cast_possible_truncation)]
    pub fn serialize(outputs: &[&[u8]]) -> Vec<u8> {
        let total_len = 4 + outputs.iter().map(|o| 4 + o.len()).sum::<usize>();
        let mut buf = Vec::with_capacity(total_len);

        buf.extend_from_slice(&(outputs.len() as u32).to_le_bytes());
        for output in outputs {
            buf.extend_from_slice(&(output.len() as u32).to_le_bytes());
            buf.extend_from_slice(output);
        }

        buf
    }

    /// Get the output at `index` as a `CellInput`.
    /// Returns an empty `CellInput` if `index` is out of bounds.
    pub fn get(&self, index: usize) -> CellInput<'_> {
        match self.data.get(index) {
            Some(bytes) => CellInput::new(bytes),
            None => CellInput::new(&[]),
        }
    }

    /// Get the last output as a `CellInput`.
    /// Returns an empty `CellInput` if there are no outputs.
    pub fn last(&self) -> CellInput<'_> {
        match self.data.last() {
            Some(bytes) => CellInput::new(bytes),
            None => CellInput::new(&[]),
        }
    }

    /// Number of cell outputs.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether there are no cell outputs.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
}

// ── DisplayPanel ─────────────────────────────────────────────────────────────

/// A single display panel in cell output. Multiple panels can be shown simultaneously.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
pub enum DisplayPanel {
    /// Plain text, rendered in a `<pre>` tag.
    Text(String),
    /// Raw HTML, rendered via `inner_html`.
    Html(String),
    /// SVG markup, rendered inline.
    Svg(String),
    /// Raw markdown, rendered client-side.
    Markdown(String),
    /// Structured table data, rendered as an HTML table.
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Interactive UI widget (slider, dropdown, checkbox, etc.).
    Interactive { kind: String, config: String },
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

// ── Svg / Html / Md newtypes ─────────────────────────────────────────────────

/// SVG content for rich display output. Use in tuples: `(data, Svg(svg_string))`
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Svg(pub String);

/// HTML content for rich display output. Use in tuples: `(data, Html(html_string))`
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Html(pub String);

/// Structured table for rich display output. Use in tuples: `(data, Table::new(headers, rows))`
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl Table {
    pub fn new(headers: Vec<impl Into<String>>, rows: Vec<Vec<impl Into<String>>>) -> Self {
        Self {
            headers: headers.into_iter().map(Into::into).collect(),
            rows: rows
                .into_iter()
                .map(|r| r.into_iter().map(Into::into).collect())
                .collect(),
        }
    }
}

/// Markdown content for rich display output. Use in tuples: `(data, Md(md_string))`
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Md(pub String);

/// JSON content for syntax-highlighted display output.
///
/// Custom `Serialize`/`Deserialize` impls encode the inner `serde_json::Value`
/// as a JSON string. This is necessary because `serde_json::Value::Deserialize`
/// calls `deserialize_any`, which is incompatible with non-self-describing
/// formats like bincode. Encoding as a JSON string ensures correct round-trip
/// through any serde-compatible format.
#[derive(Clone, Debug)]
pub struct Json(pub serde_json::Value);

impl serde::Serialize for Json {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let json_str = serde_json::to_string(&self.0).map_err(serde::ser::Error::custom)?;
        serializer.serialize_str(&json_str)
    }
}

impl<'de> serde::Deserialize<'de> for Json {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let json_str = <String as serde::Deserialize>::deserialize(deserializer)?;
        let value = serde_json::from_str(&json_str).map_err(serde::de::Error::custom)?;
        Ok(Json(value))
    }
}

impl std::str::FromStr for Json {
    type Err = serde_json::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(serde_json::from_str(s)?))
    }
}

// ── CellOutput ───────────────────────────────────────────────────────────────

/// Output produced by a cell.
///
/// Contains optional binary data (forwarded to the next cell via bincode),
/// a list of display panels shown in the UI output panel,
/// and an optional type tag describing the Rust type that was serialized.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct CellOutput {
    bytes: Vec<u8>,
    panels: Vec<DisplayPanel>,
    type_tag: Option<String>,
}

impl CellOutput {
    /// Serialize `value` with bincode and store the bytes as output.
    pub fn new<T: serde::Serialize>(value: &T) -> Result<Self, bincode::error::EncodeError> {
        let bytes = bincode::serde::encode_to_vec(value, bincode::config::standard())?;
        Ok(Self {
            bytes,
            panels: vec![],
            type_tag: None,
        })
    }

    /// Append a text panel to this output.
    pub fn with_display(mut self, text: String) -> Self {
        self.panels.push(DisplayPanel::Text(text));
        self
    }

    /// Append a text panel.
    pub fn with_text(mut self, s: impl Into<String>) -> Self {
        self.panels.push(DisplayPanel::Text(s.into()));
        self
    }

    /// Append an HTML panel.
    pub fn with_html(mut self, s: impl Into<String>) -> Self {
        self.panels.push(DisplayPanel::Html(s.into()));
        self
    }

    /// Append an SVG panel.
    pub fn with_svg(mut self, s: impl Into<String>) -> Self {
        self.panels.push(DisplayPanel::Svg(s.into()));
        self
    }

    /// Append a Markdown panel.
    pub fn with_markdown(mut self, s: impl Into<String>) -> Self {
        self.panels.push(DisplayPanel::Markdown(s.into()));
        self
    }

    /// Create an empty output (no data, no display panels).
    pub fn empty() -> Self {
        Self {
            bytes: vec![],
            panels: vec![],
            type_tag: None,
        }
    }

    /// Create a display-only output with a single text panel and no binary payload.
    pub fn text(msg: impl Into<String>) -> Self {
        Self {
            bytes: vec![],
            panels: vec![DisplayPanel::Text(msg.into())],
            type_tag: None,
        }
    }

    /// Create a display-only output with a single HTML panel and no binary payload.
    pub fn html(content: impl Into<String>) -> Self {
        Self {
            bytes: vec![],
            panels: vec![DisplayPanel::Html(content.into())],
            type_tag: None,
        }
    }

    /// Create a display-only output with a single SVG panel and no binary payload.
    pub fn svg(content: impl Into<String>) -> Self {
        Self {
            bytes: vec![],
            panels: vec![DisplayPanel::Svg(content.into())],
            type_tag: None,
        }
    }

    /// Create a display-only output with a single Markdown panel and no binary payload.
    pub fn markdown(content: impl Into<String>) -> Self {
        Self {
            bytes: vec![],
            panels: vec![DisplayPanel::Markdown(content.into())],
            type_tag: None,
        }
    }
}

// ── From<T> for CellOutput ───────────────────────────────────────────────────

/// Encode `value` for piping, then hand it to `panels` for display: the one
/// shape every typed `From<T> for CellOutput` takes, so a bare output carries
/// exactly the bytes and [`TypeTag`] the tuple impls below build for the same
/// value.
///
/// `panels` receives the value by move, so a variant that owns its display
/// payload (`Svg`, `Table`, `String`) hands it over without a clone.
fn typed_output_with<T: serde::Serialize + TypeTag>(
    value: T,
    panels: impl FnOnce(T) -> Vec<DisplayPanel>,
) -> CellOutput {
    let bytes = bincode::serde::encode_to_vec(&value, bincode::config::standard())
        .expect("serialization of a TypeTag type cannot fail");
    CellOutput {
        bytes,
        panels: panels(value),
        type_tag: Some(T::type_tag()),
    }
}

/// `From`, [`IntoPanels`] and [`TypeTag`] for primitives that implement both
/// `Serialize` and `Display`: bincode bytes for piping, a text panel for
/// display, and the type's own name as its tag. One list, three impls.
macro_rules! impl_primitive_output {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl From<$ty> for CellOutput {
                fn from(value: $ty) -> Self {
                    typed_output_with(value, |v| v.into_panels())
                }
            }

            impl IntoPanels for $ty {
                fn into_panels(&self) -> Vec<DisplayPanel> {
                    vec![DisplayPanel::Text(format!("{self}"))]
                }
            }

            impl TypeTag for $ty {
                fn type_tag() -> String {
                    stringify!($ty).into()
                }
            }
        )+
    };
}

impl_primitive_output!(
    i8, i16, i32, i64, i128, u8, u16, u32, u64, u128, f32, f64, bool, usize, isize,
);

impl From<String> for CellOutput {
    fn from(value: String) -> Self {
        typed_output_with(value, |s| vec![DisplayPanel::Text(s)])
    }
}

impl From<&str> for CellOutput {
    fn from(value: &str) -> Self {
        let bytes = bincode::serde::encode_to_vec(value, bincode::config::standard())
            .expect("serialization of &str cannot fail");
        Self {
            bytes,
            panels: vec![DisplayPanel::Text(value.to_string())],
            type_tag: Some("String".into()),
        }
    }
}

impl From<()> for CellOutput {
    fn from((): ()) -> Self {
        Self::empty()
    }
}

impl<T: serde::Serialize + std::fmt::Debug> From<Vec<T>> for CellOutput {
    fn from(value: Vec<T>) -> Self {
        // Deliberately not `typed_output_with`: the bare display names the
        // element type (`Vec<u64>, len = …`), while the tuple path's
        // `IntoPanels` prints the generic `Vec<_>`.
        let type_tag = Some(clean_type_name(std::any::type_name::<Vec<T>>()));
        let display = format_vec_truncated(&value, type_tag.as_deref());
        let bytes = bincode::serde::encode_to_vec(&value, bincode::config::standard())
            .expect("serialization of Vec<T: Serialize> cannot fail");
        Self {
            bytes,
            panels: vec![DisplayPanel::Text(display)],
            type_tag,
        }
    }
}

impl From<Svg> for CellOutput {
    fn from(value: Svg) -> Self {
        typed_output_with(value, |v| vec![DisplayPanel::Svg(v.0)])
    }
}

impl From<Html> for CellOutput {
    fn from(value: Html) -> Self {
        typed_output_with(value, |v| vec![DisplayPanel::Html(v.0)])
    }
}

impl From<Table> for CellOutput {
    fn from(value: Table) -> Self {
        // Moves the rows into the panel: `into_panels(&self)` would clone a
        // potentially large table.
        typed_output_with(value, |t| {
            vec![DisplayPanel::Table {
                headers: t.headers,
                rows: t.rows,
            }]
        })
    }
}

impl From<Md> for CellOutput {
    fn from(value: Md) -> Self {
        typed_output_with(value, |v| vec![DisplayPanel::Markdown(v.0)])
    }
}

impl From<Json> for CellOutput {
    fn from(value: Json) -> Self {
        typed_output_with(value, |v| v.into_panels())
    }
}

impl From<canvas::Canvas> for CellOutput {
    fn from(value: canvas::Canvas) -> Self {
        typed_output_with(value, |c| c.into_panels())
    }
}

impl From<gpu::GpuCanvas> for CellOutput {
    fn from(gpu_canvas: gpu::GpuCanvas) -> Self {
        let canvas = gpu_canvas.render();
        Self::from(canvas)
    }
}

impl From<canvas::Animation> for CellOutput {
    fn from(value: canvas::Animation) -> Self {
        let panels = value.into_panels();
        Self {
            bytes: vec![],
            panels,
            // A display-only value with EMPTY bytes: advertising a type tag
            // makes a downstream cell bind `let cellN: Animation =
            // …deserialize()` and panic at runtime on the zero-byte slot
            // (and `last` binds every tagged slot). No tag means piping skips
            // it — same rationale as SimulationMeta/LiveViewMeta below.
            type_tag: None,
        }
    }
}

impl From<SimulationMeta> for CellOutput {
    fn from(meta: SimulationMeta) -> Self {
        let rgb_data = canvas::base64_encode(meta.first_frame.pixels());
        let panels = vec![DisplayPanel::Simulation {
            width: meta.width,
            height: meta.height,
            fps: meta.fps,
            first_frame_data: rgb_data,
            sliders: meta.sliders,
        }];
        Self {
            bytes: vec![],
            panels,
            // A simulation produces a display panel, not a data value (bytes are
            // empty). Advertising a type tag makes a *downstream* code cell try to
            // declare `let cellN: Simulation` — but `Simulation` is a trait, not a
            // type, so the cell fails to compile ("expected a type, found a
            // trait"). No tag means downstream cells skip it in piping.
            type_tag: None,
        }
    }
}

impl From<LiveViewMeta> for CellOutput {
    fn from(meta: LiveViewMeta) -> Self {
        // By value: the meta is owned, so the initial content moves into the
        // panel rather than being copied.
        let (kind, content) = match meta.initial_content {
            LiveContent::Text(s) => ("text", s),
            LiveContent::Html(s) => ("html", s),
            LiveContent::Markdown(s) => ("markdown", s),
        };
        let panels = vec![DisplayPanel::LiveView {
            fps: meta.fps,
            kind: kind.into(),
            content,
        }];
        Self {
            bytes: vec![],
            panels,
            // Same as `Simulation` above: a live view is a display panel, not a
            // data value, and `LiveView` is a trait. A tag here makes downstream
            // code cells emit `let cellN: LiveView` and fail to compile. None
            // means they skip it in piping.
            type_tag: None,
        }
    }
}

// ── IntoPanels trait ─────────────────────────────────────────────────────────

/// Trait for types that can produce display panels.
pub trait IntoPanels {
    #[allow(clippy::wrong_self_convention)]
    fn into_panels(&self) -> Vec<DisplayPanel>;
}

impl IntoPanels for String {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Text(self.clone())]
    }
}

impl<T: std::fmt::Debug> IntoPanels for Vec<T> {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Text(format_vec_truncated(self, None))]
    }
}

impl IntoPanels for Svg {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Svg(self.0.clone())]
    }
}

impl IntoPanels for Html {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Html(self.0.clone())]
    }
}

impl IntoPanels for Table {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Table {
            headers: self.headers.clone(),
            rows: self.rows.clone(),
        }]
    }
}

impl IntoPanels for Md {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Markdown(self.0.clone())]
    }
}

impl IntoPanels for Json {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Html(render_json_html(&self.0))]
    }
}

impl IntoPanels for canvas::Canvas {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        // The one Canvas rendering: `From<Canvas> for CellOutput` delegates
        // here. A structured BlobImage panel the UI displays directly;
        // emitting an `<img>` inside an Html panel puts the data: URI through
        // the HTML sanitizer, which strips it (ammonia's URL schemes exclude
        // `data:`) — a tuple output like `(Table, canvas)` then renders a
        // correctly-sized empty image.
        let bmp = self.to_bmp();
        vec![DisplayPanel::BlobImage {
            mime_type: "image/bmp".into(),
            base64_data: canvas::base64_encode(&bmp),
            width: self.width(),
            height: self.height(),
        }]
    }
}

impl IntoPanels for gpu::GpuCanvas {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        // GpuCanvas can't render without consuming self, so return placeholder.
        vec![DisplayPanel::Text(
            "[GpuCanvas: call .render() or convert to CellOutput]".into(),
        )]
    }
}

impl IntoPanels for canvas::Animation {
    #[allow(clippy::cast_possible_truncation)]
    fn into_panels(&self) -> Vec<DisplayPanel> {
        let (w, h) = if let Some(f) = self.frames().first() {
            (f.width(), f.height())
        } else {
            return vec![];
        };
        // Encode frame by frame instead of into a concatenated copy first: an
        // animation is the largest output the crate makes (100 frames of
        // 400x400 is 48 MB of RGB), and every frame is `w * h * 3` bytes, a
        // multiple of 3, so the per-frame encodings join into exactly the
        // encoding of the concatenation.
        let frame_b64 = canvas::rgb_byte_count(w, h).div_ceil(3).saturating_mul(4);
        let mut data = String::with_capacity(self.frames().len().saturating_mul(frame_b64));
        for frame in self.frames() {
            canvas::base64_encode_into(&mut data, frame.pixels());
        }
        vec![DisplayPanel::Animation {
            width: w,
            height: h,
            fps: self.fps(),
            frame_count: self.frames().len() as u32,
            data,
        }]
    }
}

impl IntoPanels for CellOutput {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        self.panels.clone()
    }
}

impl IntoPanels for () {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![]
    }
}

// ── TypeTag trait ────────────────────────────────────────────────────────────

/// Trait for types that have a known Rust type tag for scaffold injection.
pub trait TypeTag {
    fn type_tag() -> String;
}

impl TypeTag for String {
    fn type_tag() -> String {
        "String".into()
    }
}

impl<T: 'static> TypeTag for Vec<T> {
    fn type_tag() -> String {
        clean_type_name(std::any::type_name::<Vec<T>>())
    }
}

impl TypeTag for Svg {
    fn type_tag() -> String {
        "Svg".into()
    }
}

impl TypeTag for Html {
    fn type_tag() -> String {
        "Html".into()
    }
}

impl TypeTag for Table {
    fn type_tag() -> String {
        "Table".into()
    }
}

impl TypeTag for Md {
    fn type_tag() -> String {
        "Md".into()
    }
}

impl TypeTag for Json {
    fn type_tag() -> String {
        "Json".into()
    }
}

impl TypeTag for canvas::Canvas {
    fn type_tag() -> String {
        "Canvas".into()
    }
}

impl TypeTag for canvas::Animation {
    fn type_tag() -> String {
        "Animation".into()
    }
}

impl TypeTag for gpu::GpuCanvas {
    fn type_tag() -> String {
        "GpuCanvas".into()
    }
}

impl TypeTag for CellOutput {
    fn type_tag() -> String {
        "CellOutput".into()
    }
}

impl TypeTag for () {
    fn type_tag() -> String {
        "()".into()
    }
}

// ── Simulation ──────────────────────────────────────────────────────────────

/// Trait for live simulations that produce frames on each tick.
///
/// Implement this on your simulation state to enable interactive playback in
/// the notebook.  The runtime calls [`init`](Simulation::init) once, then
/// [`tick`](Simulation::tick) repeatedly at the target [`fps`](Simulation::fps).
pub trait Simulation: Sized + 'static {
    /// Create the initial simulation state.
    fn init() -> Self;
    /// Advance the simulation by one frame and return the canvas to display.
    fn tick(&mut self) -> canvas::Canvas;
    /// Target frames per second (default: 30).
    fn fps() -> u32 {
        30
    }
    /// Slider declarations for this simulation (default: none).
    fn sliders() -> Vec<SimSliderMeta> {
        vec![]
    }
}

/// Metadata for a simulation slider, declaring a bus key and value range.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SimSliderMeta {
    pub key: String,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub label: String,
    pub default: f64,
}

/// Metadata emitted by the scaffold for a [`Simulation`] cell.
pub struct SimulationMeta {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub first_frame: canvas::Canvas,
    pub sliders: Vec<SimSliderMeta>,
}

/// FFI-safe return type for the `cell_tick` export.
#[repr(C)]
pub struct TickResult {
    pub rgb_ptr: *mut u8,
    pub rgb_len: usize,
    pub width: u32,
    pub height: u32,
}

impl From<canvas::Canvas> for TickResult {
    fn from(canvas: canvas::Canvas) -> Self {
        let (width, height) = (canvas.width(), canvas.height());
        let (rgb_ptr, rgb_len) = vec_into_raw(canvas.into_pixels());
        TickResult {
            rgb_ptr,
            rgb_len,
            width,
            height,
        }
    }
}

// ── LiveView ────────────────────────────────────────────────────────────────

/// Trait for live views that produce text/HTML/Markdown on each tick.
///
/// Implement this on your view state to enable live-updating content in
/// the notebook. The runtime calls [`init`](LiveView::init) once, then
/// [`tick`](LiveView::tick) repeatedly at the target [`fps`](LiveView::fps).
pub trait LiveView: Sized + 'static {
    /// Create the initial view state.
    fn init() -> Self;
    /// Produce updated content for this tick.
    fn tick(&mut self) -> LiveContent;
    /// Target frames per second (default: 10).
    fn fps() -> u32 {
        10
    }
}

/// Content produced by a [`LiveView`] tick.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum LiveContent {
    /// Plain text, rendered in a `<pre>` tag.
    Text(String),
    /// Raw HTML, rendered via `inner_html`.
    Html(String),
    /// Markdown, rendered client-side (supports `KaTeX`).
    Markdown(String),
}

/// Metadata emitted by the scaffold for a [`LiveView`] cell.
pub struct LiveViewMeta {
    pub fps: u32,
    pub initial_content: LiveContent,
}

/// FFI-safe return type for the `cell_tick` export of a `LiveView` cell.
///
/// The app decodes `kind` with `live_kind_str` (ironpad-app's
/// `components/live_view_panel.rs`) into the same strings the
/// `From<LiveViewMeta>` panel carries.
///
/// Layout (12 bytes on wasm32):
///   - offset 0: `kind` (`u32`) — 0=Text, 1=Html, 2=Markdown
///   - offset 4: `content_ptr` (`*mut u8`) — pointer to UTF-8 content string
///   - offset 8: `content_len` (`usize`) — length of content string
#[repr(C)]
pub struct LiveTickResult {
    pub kind: u32,
    pub content_ptr: *mut u8,
    pub content_len: usize,
}

impl From<LiveContent> for LiveTickResult {
    fn from(content: LiveContent) -> Self {
        let (kind, s) = match content {
            LiveContent::Text(s) => (0, s),
            LiveContent::Html(s) => (1, s),
            LiveContent::Markdown(s) => (2, s),
        };
        let (content_ptr, content_len) = vec_into_raw(s.into_bytes());
        LiveTickResult {
            kind,
            content_ptr,
            content_len,
        }
    }
}

// ── Tuple From impls ────────────────────────────────────────────────────────

macro_rules! impl_from_tuple_for_cell_output {
    (($first:ident $(, $rest:ident)+)) => {
        impl<$first, $($rest),+> From<($first, $($rest),+)> for CellOutput
        where
            $first: serde::Serialize + IntoPanels + TypeTag,
            $($rest: serde::Serialize + IntoPanels + TypeTag,)+
        {
            #[allow(non_snake_case)]
            fn from(value: ($first, $($rest),+)) -> Self {
                let bytes = bincode::serde::encode_to_vec(&value, bincode::config::standard())
                    .expect("tuple serialization cannot fail");
                let type_tag = {
                    let mut parts = vec![$first::type_tag()];
                    $(parts.push($rest::type_tag());)+
                    format!("({})", parts.join(", "))
                };
                let ($first, $($rest),+) = value;
                let mut panels = $first.into_panels();
                $(panels.extend($rest.into_panels());)+
                Self { bytes, panels, type_tag: Some(type_tag) }
            }
        }
    };
}

impl_from_tuple_for_cell_output!((A, B));
impl_from_tuple_for_cell_output!((A, B, C));
impl_from_tuple_for_cell_output!((A, B, C, D));
impl_from_tuple_for_cell_output!((A, B, C, D, E));
impl_from_tuple_for_cell_output!((A, B, C, D, E, F));
impl_from_tuple_for_cell_output!((A, B, C, D, E, F, G));
impl_from_tuple_for_cell_output!((A, B, C, D, E, F, G, H));
impl_from_tuple_for_cell_output!((A, B, C, D, E, F, G, H, I));
impl_from_tuple_for_cell_output!((A, B, C, D, E, F, G, H, I, J));

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Strip common module prefixes from `std::any::type_name` output so that
/// type tags read like normal Rust source syntax.
///
/// E.g. `alloc::vec::Vec<alloc::string::String>` → `Vec<String>`.
fn clean_type_name(name: &str) -> String {
    name.replace("alloc::vec::Vec", "Vec")
        .replace("alloc::string::String", "String")
        .replace("alloc::boxed::Box", "Box")
        .replace("core::option::Option", "Option")
        .replace("core::result::Result", "Result")
}

/// Maximum number of elements shown when displaying a collection.
const DISPLAY_TRUNCATE_LEN: usize = 20;

/// Maximum characters for a single element's debug representation.
const DISPLAY_ELEMENT_MAX_CHARS: usize = 80;

/// Truncate a single element's debug string if it exceeds [`DISPLAY_ELEMENT_MAX_CHARS`].
fn truncate_debug<T: std::fmt::Debug>(val: &T) -> String {
    let full = format!("{val:?}");
    if full.len() <= DISPLAY_ELEMENT_MAX_CHARS {
        return full;
    }
    // Floor the cut to a char boundary: a naive byte slice at 80 panics when
    // that index lands mid-UTF-8-character (e.g. a Vec of accented or CJK
    // strings), trapping the whole cell during output formatting.
    let mut cut = DISPLAY_ELEMENT_MAX_CHARS;
    while cut > 0 && !full.is_char_boundary(cut) {
        cut -= 1;
    }
    let mut s = full[..cut].to_string();
    s.push('…');
    s
}

/// Format a `Vec<T: Debug>` for display, truncating to [`DISPLAY_TRUNCATE_LEN`]
/// elements when the collection is large, and capping each element's debug
/// representation at [`DISPLAY_ELEMENT_MAX_CHARS`].
///
/// Short vecs: `[1, 2, 3]`
/// Long vecs:  `Vec<u64>, len = 4500, [1, 2, 3, ... 18, 19, 20, ...]`
fn format_vec_truncated<T: std::fmt::Debug>(v: &[T], type_tag: Option<&str>) -> String {
    if v.len() <= DISPLAY_TRUNCATE_LEN {
        let items: Vec<String> = v.iter().map(|x| truncate_debug(x)).collect();
        return format!("[{}]", items.join(", "));
    }

    let items: Vec<String> = v[..DISPLAY_TRUNCATE_LEN]
        .iter()
        .map(|x| truncate_debug(x))
        .collect();

    let tag = type_tag.unwrap_or("Vec<_>");
    format!("{tag}, len = {}, [{}, ...]", v.len(), items.join(", "))
}

/// Append `s` to `out` with HTML special characters escaped (XSS guard).
fn html_escape_into(out: &mut String, s: &str) {
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#x27;"),
            _ => out.push(ch),
        }
    }
}

/// Render a `serde_json::Value` as syntax-highlighted HTML inside a `<pre>` block.
///
/// Every token is written straight into one buffer: a few-hundred-KB response
/// is tens of thousands of tokens, and a throwaway `String` per token was most
/// of the work.
fn render_json_html(value: &serde_json::Value) -> String {
    use std::fmt::Write as _;

    const COLOR_KEY: &str = "#e94560";
    const COLOR_STRING: &str = "#40c080";
    const COLOR_NUMBER: &str = "#40a0f0";
    const COLOR_KEYWORD: &str = "#b080f0";
    const COLOR_PUNCT: &str = "#eaeaea";

    fn open_span(buf: &mut String, color: &str) {
        buf.push_str("<span style=\"color:");
        buf.push_str(color);
        buf.push_str("\">");
    }

    fn span(buf: &mut String, color: &str, text: &str) {
        open_span(buf, color);
        buf.push_str(text);
        buf.push_str("</span>");
    }

    /// A quoted, escaped string token (a value or a key).
    fn quoted_span(buf: &mut String, color: &str, text: &str) {
        open_span(buf, color);
        buf.push_str("&quot;");
        html_escape_into(buf, text);
        buf.push_str("&quot;</span>");
    }

    fn pad(buf: &mut String, indent: usize) {
        buf.extend(std::iter::repeat_n(' ', indent));
    }

    fn write_value(buf: &mut String, value: &serde_json::Value, indent: usize) {
        match value {
            serde_json::Value::Null => span(buf, COLOR_KEYWORD, "null"),
            serde_json::Value::Bool(b) => {
                span(buf, COLOR_KEYWORD, if *b { "true" } else { "false" });
            }
            serde_json::Value::Number(n) => {
                open_span(buf, COLOR_NUMBER);
                let _ = write!(buf, "{n}");
                buf.push_str("</span>");
            }
            serde_json::Value::String(s) => quoted_span(buf, COLOR_STRING, s),
            serde_json::Value::Array(arr) => {
                if arr.is_empty() {
                    span(buf, COLOR_PUNCT, "[]");
                    return;
                }
                span(buf, COLOR_PUNCT, "[");
                buf.push('\n');
                for (i, item) in arr.iter().enumerate() {
                    pad(buf, indent + 2);
                    write_value(buf, item, indent + 2);
                    if i + 1 < arr.len() {
                        span(buf, COLOR_PUNCT, ",");
                    }
                    buf.push('\n');
                }
                pad(buf, indent);
                span(buf, COLOR_PUNCT, "]");
            }
            serde_json::Value::Object(map) => {
                if map.is_empty() {
                    span(buf, COLOR_PUNCT, "{}");
                    return;
                }
                span(buf, COLOR_PUNCT, "{");
                buf.push('\n');
                for (i, (key, val)) in map.iter().enumerate() {
                    pad(buf, indent + 2);
                    quoted_span(buf, COLOR_KEY, key);
                    span(buf, COLOR_PUNCT, ": ");
                    write_value(buf, val, indent + 2);
                    if i + 1 < map.len() {
                        span(buf, COLOR_PUNCT, ",");
                    }
                    buf.push('\n');
                }
                pad(buf, indent);
                span(buf, COLOR_PUNCT, "}");
            }
        }
    }

    let mut buf = String::from("<pre style=\"margin:0; font-family:monospace; line-height:1.5\">");
    write_value(&mut buf, value, 0);
    buf.push_str("</pre>");
    buf
}

// NOTE: Identity `From<CellOutput> for CellOutput` is provided by the blanket
// `impl<T> From<T> for T` in core, so no explicit impl is needed.

// ── CellResult (FFI) ────────────────────────────────────────────────────────

/// FFI-compatible result struct returned from `cell_main`.
///
/// The WASM host reads these six fields to extract the output bytes, display
/// text, and type tag from linear memory.
#[repr(C)]
pub struct CellResult {
    pub output_ptr: *mut u8,
    pub output_len: usize,
    pub display_ptr: *mut u8,
    pub display_len: usize,
    pub type_tag_ptr: *mut u8,
    pub type_tag_len: usize,
}

/// Leak a `Vec<u8>` and return its (pointer, length).
///
/// Returns `(null, 0)` for an empty vector.
///
/// The one leak path for all three FFI result types ([`CellResult`],
/// [`TickResult`], [`LiveTickResult`]), so the soundness argument below is
/// written once rather than kept identical across three unsafe blocks.
fn vec_into_raw(v: Vec<u8>) -> (*mut u8, usize) {
    if v.is_empty() {
        return (std::ptr::null_mut(), 0);
    }

    // `into_boxed_slice` *guarantees* capacity == length, which the reclaim path
    // (`Vec::from_raw_parts(ptr, len, len)` in `ironpad_dealloc`) relies on for
    // a sound deallocation.  `shrink_to_fit` is only *allowed* to reach that.
    let mut boxed = v.into_boxed_slice();
    let ptr = boxed.as_mut_ptr();
    let len = boxed.len();
    std::mem::forget(boxed);
    (ptr, len)
}

impl From<CellOutput> for CellResult {
    fn from(output: CellOutput) -> Self {
        let (output_ptr, output_len) = vec_into_raw(output.bytes);

        let (display_ptr, display_len) = if output.panels.is_empty() {
            (std::ptr::null_mut(), 0)
        } else {
            let json =
                serde_json::to_string(&output.panels).expect("panel serialization cannot fail");
            vec_into_raw(json.into_bytes())
        };

        let (type_tag_ptr, type_tag_len) = match output.type_tag {
            Some(s) => vec_into_raw(s.into_bytes()),
            None => (std::ptr::null_mut(), 0),
        };

        CellResult {
            output_ptr,
            output_len,
            display_ptr,
            display_len,
            type_tag_ptr,
            type_tag_len,
        }
    }
}

// ── Memory FFI ───────────────────────────────────────────────────────────────

/// Allocate `len` bytes in linear memory and return a pointer.
///
/// Called by the WASM host to write input data before invoking `cell_main`.
#[no_mangle]
pub extern "C" fn ironpad_alloc(len: usize) -> *mut u8 {
    if len == 0 {
        return std::ptr::null_mut();
    }

    let mut buf: Vec<u8> = Vec::with_capacity(len);
    let ptr = buf.as_mut_ptr();
    std::mem::forget(buf);
    ptr
}

/// Free memory previously allocated by [`ironpad_alloc`] or leaked through
/// [`CellResult`].
///
/// Called by the WASM host after it has copied result data out of linear memory.
///
/// # Safety
///
/// `ptr` must have been allocated by [`ironpad_alloc`] or by a `Vec` leaked
/// through [`CellResult`], and `len` must match the original allocation size.
#[no_mangle]
pub unsafe extern "C" fn ironpad_dealloc(ptr: *mut u8, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }

    drop(Vec::from_raw_parts(ptr, len, len));
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    // ── FFI reclaim helpers ──────────────────────────────────────────────

    /// A [`CellResult`] taken back into owned memory: what the host reads out
    /// of linear memory, decoded, with every leaked buffer reclaimed.
    struct Taken {
        bytes: Vec<u8>,
        panels: Option<Vec<DisplayPanel>>,
        tag: Option<String>,
    }

    /// Reclaim one leaked `(ptr, len)` buffer as an owned `Vec`, empty for a
    /// null pointer.
    ///
    /// Asserts the FFI's null-iff-empty convention, which the JS readers rely
    /// on (they only read, and only free, a buffer whose length is non-zero).
    ///
    /// # Safety
    ///
    /// A non-null `ptr` must come from [`vec_into_raw`] (or a sibling that
    /// guarantees capacity == `len`, the same contract as [`ironpad_dealloc`])
    /// and must not be reclaimed twice.
    unsafe fn reclaim(ptr: *mut u8, len: usize) -> Vec<u8> {
        assert_eq!(ptr.is_null(), len == 0, "a buffer is null iff it is empty");
        if ptr.is_null() {
            return Vec::new();
        }
        Vec::from_raw_parts(ptr, len, len)
    }

    /// Reclaim and decode every buffer a [`CellResult`] leaked.
    ///
    /// By value on purpose: consuming the result is what stops a test from
    /// reclaiming the same buffers twice.
    #[allow(clippy::needless_pass_by_value)]
    fn take(result: CellResult) -> Taken {
        // SAFETY: all three pairs come from `vec_into_raw`, and `result` is
        // consumed here, so each buffer is reclaimed exactly once.
        let (bytes, display, tag) = unsafe {
            (
                reclaim(result.output_ptr, result.output_len),
                reclaim(result.display_ptr, result.display_len),
                reclaim(result.type_tag_ptr, result.type_tag_len),
            )
        };
        Taken {
            bytes,
            panels: (!display.is_empty())
                .then(|| serde_json::from_slice(&display).expect("display JSON parses as panels")),
            tag: (!tag.is_empty()).then(|| String::from_utf8(tag).expect("type tag is UTF-8")),
        }
    }

    // ── Canvas panel rendering ───────────────────────────────────────────

    #[test]
    fn canvas_into_panels_is_a_blob_image_not_html() {
        // Tuple outputs like `(Table, canvas)` render via IntoPanels. An Html
        // panel would route the data: URI through the HTML sanitizer, which
        // strips it — the viewer got a correctly-sized EMPTY image. The panel
        // must be the same structured BlobImage the direct From<Canvas> path
        // produces.
        let canvas = canvas::Canvas::new(4, 2);
        let panels = canvas.into_panels();
        assert_eq!(panels.len(), 1);
        match &panels[0] {
            DisplayPanel::BlobImage {
                mime_type,
                base64_data,
                width,
                height,
            } => {
                assert_eq!(mime_type, "image/bmp");
                assert!(!base64_data.is_empty());
                assert_eq!((*width, *height), (4, 2));
            }
            other => panic!("expected BlobImage, got {other:?}"),
        }
    }

    /// A bare `From<T>` output must equal what the tuple path builds for the
    /// same value: its `IntoPanels` panels, its `TypeTag` tag, and its bincode
    /// bytes. The Canvas pair once drifted (an Html panel on one side, a
    /// `BlobImage` on the other) and cost a `CACHE_EPOCH` bump.
    fn assert_from_matches_into_panels_and_type_tag<T>(value: &T)
    where
        T: Clone + Serialize + IntoPanels + TypeTag,
        CellOutput: From<T>,
    {
        let out = CellOutput::from(value.clone());
        assert_eq!(out.panels, value.into_panels(), "{}", T::type_tag());
        assert_eq!(out.type_tag, Some(T::type_tag()));
        assert_eq!(
            out.bytes,
            bincode::serde::encode_to_vec(value, bincode::config::standard()).unwrap(),
            "{}",
            T::type_tag()
        );
    }

    #[test]
    fn from_matches_into_panels_and_type_tag() {
        assert_from_matches_into_panels_and_type_tag(&-8i8);
        assert_from_matches_into_panels_and_type_tag(&-16i16);
        assert_from_matches_into_panels_and_type_tag(&-32i32);
        assert_from_matches_into_panels_and_type_tag(&-64i64);
        assert_from_matches_into_panels_and_type_tag(&-128i128);
        assert_from_matches_into_panels_and_type_tag(&8u8);
        assert_from_matches_into_panels_and_type_tag(&16u16);
        assert_from_matches_into_panels_and_type_tag(&32u32);
        assert_from_matches_into_panels_and_type_tag(&64u64);
        assert_from_matches_into_panels_and_type_tag(&128u128);
        assert_from_matches_into_panels_and_type_tag(&1.25f32);
        assert_from_matches_into_panels_and_type_tag(&-2.5f64);
        assert_from_matches_into_panels_and_type_tag(&true);
        assert_from_matches_into_panels_and_type_tag(&7usize);
        assert_from_matches_into_panels_and_type_tag(&-7isize);
        assert_from_matches_into_panels_and_type_tag(&"text".to_string());
        assert_from_matches_into_panels_and_type_tag(&Svg("<svg/>".into()));
        assert_from_matches_into_panels_and_type_tag(&Html("<b>b</b>".into()));
        assert_from_matches_into_panels_and_type_tag(&Md("# m".into()));
        assert_from_matches_into_panels_and_type_tag(&Table::new(
            vec!["h1", "h2"],
            vec![vec!["a", "b"], vec!["c", "d"]],
        ));
        assert_from_matches_into_panels_and_type_tag(&Json(serde_json::json!({"k": [1, 2]})));
        let mut canvas = canvas::Canvas::new(3, 2);
        canvas.set_pixel(1, 1, (9, 8, 7));
        assert_from_matches_into_panels_and_type_tag(&canvas);
    }

    #[test]
    fn tuple_with_canvas_renders_the_same_panel_as_direct_canvas_output() {
        let direct = CellOutput::from(canvas::Canvas::new(3, 3));
        let tupled = CellOutput::from((
            Table::new(vec!["h"], vec![vec!["v"]]),
            canvas::Canvas::new(3, 3),
        ));
        let blob_of = |out: &CellOutput| {
            out.panels
                .iter()
                .find_map(|p| match p {
                    DisplayPanel::BlobImage { base64_data, .. } => Some(base64_data.clone()),
                    _ => None,
                })
                .expect("output should carry a BlobImage panel")
        };
        assert_eq!(blob_of(&direct), blob_of(&tupled));
    }

    // ── CellInputs::from_raw bounds checking ─────────────────────────────

    #[test]
    fn cell_inputs_round_trip() {
        let raw = CellInputs::serialize(&[b"first".as_slice(), b"second".as_slice()]);
        let inputs = CellInputs::from_raw(&raw);
        assert_eq!(inputs.data.len(), 2);
        assert_eq!(inputs.data[0], b"first");
        assert_eq!(inputs.data[1], b"second");
    }

    #[test]
    fn cell_inputs_empty_is_empty() {
        assert!(CellInputs::from_raw(&[]).data.is_empty());
        // A count header with fewer than 4 bytes is ignored, not indexed.
        assert!(CellInputs::from_raw(&[1, 2]).data.is_empty());
    }

    #[test]
    fn cell_inputs_truncated_does_not_panic() {
        let mut raw = CellInputs::serialize(&[b"hello".as_slice(), b"world".as_slice()]);
        // Chop the buffer mid-second-segment: parsing must stop cleanly.
        raw.truncate(raw.len() - 3);
        let inputs = CellInputs::from_raw(&raw);
        assert_eq!(
            inputs.data.first().map(Vec::as_slice),
            Some(b"hello".as_slice())
        );
    }

    #[test]
    fn cell_inputs_bogus_count_does_not_over_read() {
        // Claim 1000 segments but provide none: must not panic or allocate wildly.
        let mut raw = 1000u32.to_le_bytes().to_vec();
        raw.extend_from_slice(&5u32.to_le_bytes()); // one length prefix, no payload
        let inputs = CellInputs::from_raw(&raw);
        assert!(inputs.data.is_empty(), "no complete segment should parse");
    }

    // ── vec_into_raw ↔ ironpad_dealloc round-trip ────────────────────────────

    #[test]
    fn vec_into_raw_empty_returns_null() {
        let (ptr, len) = vec_into_raw(Vec::new());
        assert!(ptr.is_null());
        assert_eq!(len, 0);
    }

    #[test]
    fn vec_into_raw_round_trips_with_len_equal_capacity() {
        // Excess capacity: `into_boxed_slice` must reach capacity == len so the
        // `from_raw_parts(ptr, len, len)` reclaim in `ironpad_dealloc` is sound.
        let mut v = Vec::with_capacity(64);
        v.extend_from_slice(b"payload");
        let (ptr, len) = vec_into_raw(v);
        assert_eq!(len, 7);
        assert!(!ptr.is_null());

        // SAFETY: `ptr`/`len` came from `vec_into_raw`, which guarantees
        // capacity == len, so reclaiming with cap == len (as `ironpad_dealloc`
        // does) is sound.
        let reclaimed = unsafe { reclaim(ptr, len) };
        assert_eq!(reclaimed, b"payload");
    }

    #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
    struct Point {
        x: f64,
        y: f64,
    }

    // ── CellInput / CellOutput round-trip ────────────────────────────────

    #[test]
    fn round_trip_struct() {
        let original = Point { x: 1.5, y: -3.0 };
        let output = CellOutput::new(&original).expect("serialize");

        let t = take(output.into());
        assert!(!t.bytes.is_empty());

        // Feed the reclaimed bytes into CellInput.
        let decoded: Point = CellInput::new(&t.bytes).deserialize().expect("deserialize");
        assert_eq!(decoded, original);
    }

    #[test]
    fn round_trip_vec() {
        let data: Vec<i32> = vec![10, 20, 30];
        let output = CellOutput::new(&data).expect("serialize");

        let t = take(output.into());
        let decoded: Vec<i32> = CellInput::new(&t.bytes).deserialize().expect("deserialize");
        assert_eq!(decoded, data);
    }

    #[test]
    fn round_trip_json() {
        let original = Json(serde_json::json!({
            "name": "ironpad",
            "count": 42,
            "active": true,
            "tags": ["wasm", "rust"],
            "nested": { "key": null }
        }));

        let output: CellOutput = original.clone().into();
        let t = take(output.into());
        assert!(!t.bytes.is_empty());

        let decoded: Json = CellInput::new(&t.bytes)
            .deserialize()
            .expect("Json bincode round-trip");
        assert_eq!(decoded.0, original.0);
    }

    // ── CellInput helpers ────────────────────────────────────────────────

    #[test]
    fn input_empty() {
        let input = CellInput::new(&[]);
        assert!(input.is_empty());
        assert_eq!(input.raw(), &[] as &[u8]);
    }

    #[test]
    fn input_raw_bytes() {
        let bytes = [1u8, 2, 3, 4];
        let input = CellInput::new(&bytes);
        assert!(!input.is_empty());
        assert_eq!(input.raw(), &[1u8, 2, 3, 4]);
    }

    // ── CellOutput constructors ──────────────────────────────────────────

    #[test]
    fn output_empty() {
        let output = CellOutput::empty();
        assert!(output.type_tag.is_none());
        // `take` asserts each empty buffer crossed the FFI as a null pointer.
        let t = take(output.into());
        assert!(t.bytes.is_empty());
        assert!(t.panels.is_none());
        assert!(t.tag.is_none());
    }

    #[test]
    fn output_text_only() {
        let output = CellOutput::text("hello world");
        assert!(output.type_tag.is_none());
        assert_eq!(
            output.panels,
            vec![DisplayPanel::Text("hello world".into())]
        );
        let t = take(output.into());

        assert!(t.bytes.is_empty());
        assert!(t.tag.is_none());
        assert_eq!(
            t.panels,
            Some(vec![DisplayPanel::Text("hello world".into())])
        );
    }

    #[test]
    fn output_with_display() {
        let data = 42u64;
        let output = CellOutput::new(&data)
            .expect("serialize")
            .with_display("The answer is 42".to_string());

        assert!(output.type_tag.is_none());
        assert_eq!(
            output.panels,
            vec![DisplayPanel::Text("The answer is 42".into())]
        );
        let t = take(output.into());

        assert!(!t.bytes.is_empty());
        assert!(t.tag.is_none());
        assert_eq!(
            t.panels,
            Some(vec![DisplayPanel::Text("The answer is 42".into())])
        );
    }

    // ── CellResult layout ────────────────────────────────────────────────

    #[test]
    fn cell_result_is_repr_c() {
        // Verify the size matches 6 pointer-sized fields.
        let expected = 6 * std::mem::size_of::<usize>();
        assert_eq!(std::mem::size_of::<CellResult>(), expected);
    }

    // ── FFI alloc / dealloc ──────────────────────────────────────────────

    #[test]
    fn alloc_dealloc_smoke() {
        let ptr = ironpad_alloc(64);
        assert!(!ptr.is_null());

        // Write into the allocation to verify it's valid memory.
        unsafe {
            std::ptr::write_bytes(ptr, 0xAB, 64);
            ironpad_dealloc(ptr, 64);
        }
    }

    #[test]
    fn alloc_zero_returns_null() {
        let ptr = ironpad_alloc(0);
        assert!(ptr.is_null());
    }

    #[test]
    fn dealloc_null_is_noop() {
        // Must not panic or crash.
        unsafe {
            ironpad_dealloc(std::ptr::null_mut(), 0);
            ironpad_dealloc(std::ptr::null_mut(), 10);
        }
    }

    // ── From<T> for CellOutput ──────────────────────────────────────────

    #[test]
    fn from_i32_serializes_and_displays() {
        let output = CellOutput::from(42i32);
        assert_eq!(output.type_tag.as_deref(), Some("i32"));
        assert_eq!(output.panels, vec![DisplayPanel::Text("42".into())]);
        let t = take(output.into());

        // Display JSON carries the panels, and the tag crosses intact.
        assert_eq!(t.panels, Some(vec![DisplayPanel::Text("42".into())]));
        assert_eq!(t.tag.as_deref(), Some("i32"));

        // Round-trip via CellInput.
        let decoded: i32 = CellInput::new(&t.bytes)
            .deserialize()
            .expect("deserialize i32");
        assert_eq!(decoded, 42);
    }

    #[test]
    fn from_f64_serializes_and_displays() {
        let output = CellOutput::from(42.5f64);
        assert_eq!(output.type_tag.as_deref(), Some("f64"));
        assert_eq!(output.panels, vec![DisplayPanel::Text("42.5".into())]);
        let t = take(output.into());

        assert_eq!(t.panels, Some(vec![DisplayPanel::Text("42.5".into())]));
        let decoded: f64 = CellInput::new(&t.bytes)
            .deserialize()
            .expect("deserialize f64");
        assert!((decoded - 42.5).abs() < f64::EPSILON);
    }

    #[test]
    fn from_bool_serializes_and_displays() {
        let output = CellOutput::from(true);
        assert_eq!(output.type_tag.as_deref(), Some("bool"));
        assert_eq!(output.panels, vec![DisplayPanel::Text("true".into())]);
        let t = take(output.into());

        assert_eq!(t.panels, Some(vec![DisplayPanel::Text("true".into())]));
    }

    #[test]
    fn from_string_serializes_and_displays() {
        let output = CellOutput::from("hello world".to_string());
        assert_eq!(output.type_tag.as_deref(), Some("String"));
        assert_eq!(
            output.panels,
            vec![DisplayPanel::Text("hello world".into())]
        );
        let t = take(output.into());

        assert_eq!(
            t.panels,
            Some(vec![DisplayPanel::Text("hello world".into())])
        );
        let decoded: String = CellInput::new(&t.bytes)
            .deserialize()
            .expect("deserialize String");
        assert_eq!(decoded, "hello world");
    }

    #[test]
    fn from_str_ref_serializes_and_displays() {
        let output = CellOutput::from("hello");
        assert_eq!(output.type_tag.as_deref(), Some("String"));
        assert_eq!(output.panels, vec![DisplayPanel::Text("hello".into())]);
        let t = take(output.into());

        assert_eq!(t.panels, Some(vec![DisplayPanel::Text("hello".into())]));
    }

    #[test]
    fn from_unit_produces_empty_output() {
        let output = CellOutput::from(());
        assert!(output.type_tag.is_none());
        // `take` asserts each empty buffer crossed the FFI as a null pointer.
        let t = take(output.into());
        assert!(t.bytes.is_empty());
        assert!(t.panels.is_none());
        assert!(t.tag.is_none());
    }

    #[test]
    fn into_syntax_works_for_primitives() {
        // Verify .into() works for type inference.
        let _output: CellOutput = 42i32.into();
        let _output: CellOutput = "test".into();
        let _output: CellOutput = true.into();
        let _output: CellOutput = 42.5f64.into();
        let _output: CellOutput = ().into();
        let _output: CellOutput = vec![1, 2, 3].into();
    }

    #[test]
    fn from_vec_serializes_and_displays() {
        let data: Vec<i32> = vec![10, 20, 30];
        let output = CellOutput::from(data.clone());
        assert_eq!(output.type_tag.as_deref(), Some("Vec<i32>"));
        assert_eq!(
            output.panels,
            vec![DisplayPanel::Text("[10, 20, 30]".into())]
        );
        let t = take(output.into());

        // Display JSON carries the Debug-formatted panel; the tag crosses intact.
        assert_eq!(
            t.panels,
            Some(vec![DisplayPanel::Text("[10, 20, 30]".into())])
        );
        assert_eq!(t.tag.as_deref(), Some("Vec<i32>"));

        // Round-trip via CellInput.
        let decoded: Vec<i32> = CellInput::new(&t.bytes)
            .deserialize()
            .expect("deserialize Vec<i32>");
        assert_eq!(decoded, data);
    }

    // ── Type tag tests ──────────────────────────────────────────────────

    #[test]
    fn type_tag_clean_type_name() {
        assert_eq!(clean_type_name("alloc::vec::Vec<i32>"), "Vec<i32>");
        assert_eq!(
            clean_type_name("alloc::vec::Vec<alloc::string::String>"),
            "Vec<String>"
        );
        assert_eq!(clean_type_name("alloc::string::String"), "String");
        assert_eq!(clean_type_name("i32"), "i32");
        assert_eq!(clean_type_name("bool"), "bool");
    }

    #[test]
    fn type_tag_vec_string_is_cleaned() {
        let output = CellOutput::from(vec!["a".to_string(), "b".to_string()]);
        assert_eq!(output.type_tag.as_deref(), Some("Vec<String>"));
    }

    #[test]
    fn identity_from_preserves_type_tag() {
        let original = CellOutput::from(42u32);
        assert_eq!(original.type_tag.as_deref(), Some("u32"));
        assert_eq!(original.panels, vec![DisplayPanel::Text("42".into())]);
        #[allow(clippy::useless_conversion)]
        let converted: CellOutput = original.into();
        assert_eq!(converted.type_tag.as_deref(), Some("u32"));
        assert_eq!(converted.panels, vec![DisplayPanel::Text("42".into())]);
    }

    #[test]
    fn new_constructor_has_no_type_tag() {
        let output = CellOutput::new(&42u32).expect("serialize");
        assert!(output.type_tag.is_none());
    }

    // ── CellInputs ──────────────────────────────────────────────────────

    #[test]
    fn cell_inputs_empty() {
        let inputs = CellInputs::from_raw(&[]);
        assert_eq!(inputs.len(), 0);
        assert!(inputs.is_empty());
        assert!(inputs.get(0).is_empty());
    }

    #[test]
    fn cell_inputs_round_trip_single() {
        let value = 42u32;
        let encoded = bincode::serde::encode_to_vec(value, bincode::config::standard()).unwrap();
        let wire = CellInputs::serialize(&[&encoded]);

        let inputs = CellInputs::from_raw(&wire);
        assert_eq!(inputs.len(), 1);
        assert!(!inputs.is_empty());

        let decoded: u32 = inputs.get(0).deserialize().expect("deserialize u32");
        assert_eq!(decoded, 42);
    }

    #[test]
    fn cell_inputs_round_trip_multi() {
        let val_a = 10u32;
        let val_b = "hello".to_string();
        let val_c: Vec<i32> = vec![1, 2, 3];

        let enc_a = bincode::serde::encode_to_vec(val_a, bincode::config::standard()).unwrap();
        let enc_b = bincode::serde::encode_to_vec(&val_b, bincode::config::standard()).unwrap();
        let enc_c = bincode::serde::encode_to_vec(&val_c, bincode::config::standard()).unwrap();

        let wire = CellInputs::serialize(&[&enc_a, &enc_b, &enc_c]);
        let inputs = CellInputs::from_raw(&wire);
        assert_eq!(inputs.len(), 3);

        let dec_a: u32 = inputs.get(0).deserialize().expect("deserialize u32");
        assert_eq!(dec_a, 10);

        let dec_b: String = inputs.get(1).deserialize().expect("deserialize String");
        assert_eq!(dec_b, "hello");

        let dec_c: Vec<i32> = inputs.get(2).deserialize().expect("deserialize Vec<i32>");
        assert_eq!(dec_c, vec![1, 2, 3]);
    }

    #[test]
    fn cell_inputs_oob_graceful() {
        let val = 1u32;
        let enc = bincode::serde::encode_to_vec(val, bincode::config::standard()).unwrap();
        let wire = CellInputs::serialize(&[&enc, &enc]);

        let inputs = CellInputs::from_raw(&wire);
        assert_eq!(inputs.len(), 2);

        // Out of bounds should not panic, returns empty.
        let oob = inputs.get(999);
        assert!(oob.is_empty());
    }

    #[test]
    fn cell_inputs_last() {
        let val_a = 10u32;
        let val_b = 99u32;
        let enc_a = bincode::serde::encode_to_vec(val_a, bincode::config::standard()).unwrap();
        let enc_b = bincode::serde::encode_to_vec(val_b, bincode::config::standard()).unwrap();

        let wire = CellInputs::serialize(&[&enc_a, &enc_b]);
        let inputs = CellInputs::from_raw(&wire);

        let last: u32 = inputs.last().deserialize().expect("deserialize last");
        assert_eq!(last, 99);
    }

    #[test]
    fn cell_inputs_last_empty() {
        let inputs = CellInputs::from_raw(&[]);
        assert!(inputs.last().is_empty());
    }

    #[test]
    fn cell_inputs_serialize_empty() {
        let wire = CellInputs::serialize(&[]);
        // Should be exactly 4 bytes: count = 0.
        assert_eq!(wire.len(), 4);
        assert_eq!(u32::from_le_bytes(wire[..4].try_into().unwrap()), 0);

        let inputs = CellInputs::from_raw(&wire);
        assert!(inputs.is_empty());
    }

    // ── DisplayPanel tests ──────────────────────────────────────────────

    #[test]
    fn cell_output_html_constructor() {
        let output = CellOutput::html("<b>bold</b>");
        assert!(output.bytes.is_empty());
        assert!(output.type_tag.is_none());
        assert_eq!(
            output.panels,
            vec![DisplayPanel::Html("<b>bold</b>".into())]
        );
    }

    #[test]
    fn cell_output_svg_constructor() {
        let svg = r#"<svg><circle r="10"/></svg>"#;
        let output = CellOutput::svg(svg);
        assert!(output.bytes.is_empty());
        assert!(output.type_tag.is_none());
        assert_eq!(output.panels, vec![DisplayPanel::Svg(svg.into())]);
    }

    #[test]
    fn cell_output_markdown_constructor() {
        let output = CellOutput::markdown("# Hello");
        assert!(output.bytes.is_empty());
        assert!(output.type_tag.is_none());
        assert_eq!(
            output.panels,
            vec![DisplayPanel::Markdown("# Hello".into())]
        );
    }

    #[test]
    fn cell_output_builder_chain() {
        let output = CellOutput::empty()
            .with_text("hello")
            .with_svg("<svg></svg>");
        assert_eq!(
            output.panels,
            vec![
                DisplayPanel::Text("hello".into()),
                DisplayPanel::Svg("<svg></svg>".into()),
            ]
        );
    }

    #[test]
    fn display_panel_json_roundtrip() {
        let panels = vec![
            DisplayPanel::Text("hello".into()),
            DisplayPanel::Html("<b>bold</b>".into()),
            DisplayPanel::Svg("<svg/>".into()),
            DisplayPanel::Markdown("# heading".into()),
        ];
        let json = serde_json::to_string(&panels).expect("serialize");
        let decoded: Vec<DisplayPanel> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(panels, decoded);
    }

    #[test]
    fn cell_output_from_preserves_panels() {
        let original = CellOutput::empty()
            .with_text("text")
            .with_html("<p>html</p>");
        let expected_panels = original.panels.clone();
        #[allow(clippy::useless_conversion)]
        let converted: CellOutput = original.into();
        assert_eq!(converted.panels, expected_panels);
    }

    // ── Svg / Html / Md newtype tests ──────────────────────────────────

    #[test]
    fn svg_newtype_into_cell_output() {
        let output = CellOutput::from(Svg("<svg><circle r='10'/></svg>".into()));
        assert_eq!(output.type_tag.as_deref(), Some("Svg"));
        assert_eq!(
            output.panels,
            vec![DisplayPanel::Svg("<svg><circle r='10'/></svg>".into())]
        );
        assert!(!output.bytes.is_empty());

        // Verify round-trip deserialization.
        let input = CellInput::new(&output.bytes);
        let decoded: Svg = input.deserialize().expect("deserialize Svg");
        assert_eq!(decoded.0, "<svg><circle r='10'/></svg>");
    }

    #[test]
    fn html_newtype_into_cell_output() {
        let output = CellOutput::from(Html("<b>bold</b>".into()));
        assert_eq!(output.type_tag.as_deref(), Some("Html"));
        assert_eq!(
            output.panels,
            vec![DisplayPanel::Html("<b>bold</b>".into())]
        );
        assert!(!output.bytes.is_empty());

        // Verify round-trip deserialization.
        let input = CellInput::new(&output.bytes);
        let decoded: Html = input.deserialize().expect("deserialize Html");
        assert_eq!(decoded.0, "<b>bold</b>");
    }

    #[test]
    fn md_newtype_into_cell_output() {
        let output = CellOutput::from(Md("# Hello\n\nworld".into()));
        assert_eq!(output.type_tag.as_deref(), Some("Md"));
        assert_eq!(
            output.panels,
            vec![DisplayPanel::Markdown("# Hello\n\nworld".into())]
        );
        assert!(!output.bytes.is_empty());

        // Verify round-trip deserialization.
        let input = CellInput::new(&output.bytes);
        let decoded: Md = input.deserialize().expect("deserialize Md");
        assert_eq!(decoded.0, "# Hello\n\nworld");
    }

    // ── IntoPanels trait tests ──────────────────────────────────────────

    #[test]
    fn into_panels_primitives() {
        assert_eq!(42i32.into_panels(), vec![DisplayPanel::Text("42".into())]);
        assert_eq!(1.5f64.into_panels(), vec![DisplayPanel::Text("1.5".into())]);
        assert_eq!(true.into_panels(), vec![DisplayPanel::Text("true".into())]);
    }

    #[test]
    fn into_panels_string() {
        let s = "hello".to_string();
        assert_eq!(s.into_panels(), vec![DisplayPanel::Text("hello".into())]);
    }

    #[test]
    fn into_panels_vec() {
        let v = vec![1, 2, 3];
        assert_eq!(
            v.into_panels(),
            vec![DisplayPanel::Text("[1, 2, 3]".into())]
        );
    }

    #[test]
    fn into_panels_vec_truncated() {
        let v: Vec<u64> = (1..=50).collect();
        let panels = v.into_panels();
        assert_eq!(panels.len(), 1);
        let DisplayPanel::Text(text) = &panels[0] else {
            panic!("expected Text panel");
        };
        assert!(
            text.starts_with("Vec<_>, len = 50, [1, 2, 3,"),
            "got: {text}"
        );
        assert!(text.ends_with("...]"), "got: {text}");
        assert!(text.contains("20,"), "should include 20th element: {text}");
        assert!(
            !text.contains("21,"),
            "should not include 21st element: {text}"
        );
    }

    #[test]
    fn truncate_debug_cuts_on_a_char_boundary() {
        // A byte slice at 80 would land mid-character here and panic; the
        // element's Debug repr is well over 80 bytes of multibyte text.
        let v = vec!["é".repeat(60)];
        let panels = v.into_panels();
        let DisplayPanel::Text(text) = &panels[0] else {
            panic!("expected Text panel");
        };
        assert!(text.ends_with("…]"), "got: {text}");
        // Emoji (4-byte) also crosses the boundary.
        let v2 = vec!["🦀".repeat(40)];
        assert!(matches!(&v2.into_panels()[0], DisplayPanel::Text(_)));
    }

    #[test]
    fn into_panels_svg_html() {
        assert_eq!(
            Svg("<svg/>".into()).into_panels(),
            vec![DisplayPanel::Svg("<svg/>".into())]
        );
        assert_eq!(
            Html("<b>hi</b>".into()).into_panels(),
            vec![DisplayPanel::Html("<b>hi</b>".into())]
        );
    }

    #[test]
    fn into_panels_md() {
        assert_eq!(
            Md("**bold**".into()).into_panels(),
            vec![DisplayPanel::Markdown("**bold**".into())]
        );
    }

    #[test]
    fn into_panels_unit() {
        assert_eq!(().into_panels(), vec![]);
    }

    #[test]
    fn into_panels_cell_output() {
        let output = CellOutput::empty().with_text("a").with_svg("<svg/>");
        assert_eq!(
            output.into_panels(),
            vec![
                DisplayPanel::Text("a".into()),
                DisplayPanel::Svg("<svg/>".into()),
            ]
        );
    }

    #[test]
    fn cell_output_from_vec_truncated() {
        let v: Vec<u64> = (1..=100).collect();
        let output = CellOutput::from(v);

        // Display should be truncated.
        let panels = output.into_panels();
        let DisplayPanel::Text(text) = &panels[0] else {
            panic!("expected Text panel");
        };
        assert!(
            text.starts_with("Vec<u64>, len = 100,"),
            "should include type tag: {text}"
        );
        assert!(text.ends_with("...]"), "should end with ...]: {text}");
    }

    // ── TypeTag trait tests ─────────────────────────────────────────────

    #[test]
    fn type_tag_trait_primitives() {
        assert_eq!(i32::type_tag(), "i32");
        assert_eq!(f64::type_tag(), "f64");
        assert_eq!(bool::type_tag(), "bool");
        assert_eq!(usize::type_tag(), "usize");
    }

    #[test]
    fn type_tag_trait_string() {
        assert_eq!(String::type_tag(), "String");
    }

    #[test]
    fn type_tag_trait_vec() {
        assert_eq!(Vec::<i32>::type_tag(), "Vec<i32>");
        assert_eq!(Vec::<String>::type_tag(), "Vec<String>");
    }

    #[test]
    fn type_tag_trait_newtypes() {
        assert_eq!(Svg::type_tag(), "Svg");
        assert_eq!(Html::type_tag(), "Html");
        assert_eq!(Md::type_tag(), "Md");
        assert_eq!(CellOutput::type_tag(), "CellOutput");
        assert_eq!(<()>::type_tag(), "()");
    }

    // ── Tuple From impl tests ───────────────────────────────────────────

    #[test]
    fn tuple_2_from_impl() {
        let output = CellOutput::from((42u32, Svg("<svg>chart</svg>".into())));
        assert_eq!(output.type_tag.as_deref(), Some("(u32, Svg)"));
        assert_eq!(
            output.panels,
            vec![
                DisplayPanel::Text("42".into()),
                DisplayPanel::Svg("<svg>chart</svg>".into()),
            ]
        );
        assert!(!output.bytes.is_empty());

        // Verify round-trip of the tuple bytes.
        let input = CellInput::new(&output.bytes);
        let decoded: (u32, Svg) = input.deserialize().expect("deserialize tuple");
        assert_eq!(decoded.0, 42);
        assert_eq!(decoded.1 .0, "<svg>chart</svg>");
    }

    #[test]
    fn tuple_3_from_impl() {
        let output = CellOutput::from((10i32, "hello".to_string(), Html("<p>world</p>".into())));
        assert_eq!(output.type_tag.as_deref(), Some("(i32, String, Html)"));
        assert_eq!(
            output.panels,
            vec![
                DisplayPanel::Text("10".into()),
                DisplayPanel::Text("hello".into()),
                DisplayPanel::Html("<p>world</p>".into()),
            ]
        );
    }

    #[test]
    fn tuple_4_from_impl() {
        let output = CellOutput::from((1u8, 2u16, 3u32, 4u64));
        assert_eq!(output.type_tag.as_deref(), Some("(u8, u16, u32, u64)"));
        assert_eq!(output.panels.len(), 4);
    }

    #[test]
    fn tuple_5_from_impl() {
        let output = CellOutput::from((
            true,
            42i32,
            "hi".to_string(),
            Svg("<svg/>".into()),
            Html("<b/>".into()),
        ));
        assert_eq!(
            output.type_tag.as_deref(),
            Some("(bool, i32, String, Svg, Html)")
        );
        assert_eq!(output.panels.len(), 5);
        assert_eq!(output.panels[0], DisplayPanel::Text("true".into()));
        assert_eq!(output.panels[3], DisplayPanel::Svg("<svg/>".into()));
        assert_eq!(output.panels[4], DisplayPanel::Html("<b/>".into()));
    }

    #[test]
    fn tuple_panels_merge() {
        let output = CellOutput::from((42u32, Svg("<svg>a</svg>".into()), Html("<b>b</b>".into())));
        // All panels merged in order.
        assert_eq!(
            output.panels,
            vec![
                DisplayPanel::Text("42".into()),
                DisplayPanel::Svg("<svg>a</svg>".into()),
                DisplayPanel::Html("<b>b</b>".into()),
            ]
        );
    }

    // ── Table type tests ────────────────────────────────────────────────

    #[test]
    fn table_new_constructor() {
        let table = Table::new(
            vec!["Name", "Age"],
            vec![vec!["Alice", "30"], vec!["Bob", "25"]],
        );
        assert_eq!(table.headers, vec!["Name", "Age"]);
        assert_eq!(table.rows.len(), 2);
        assert_eq!(table.rows[0], vec!["Alice", "30"]);
        assert_eq!(table.rows[1], vec!["Bob", "25"]);
    }

    #[test]
    fn table_new_with_owned_strings() {
        let table = Table::new(
            vec!["H1".to_string(), "H2".to_string()],
            vec![vec!["a".to_string(), "b".to_string()]],
        );
        assert_eq!(table.headers, vec!["H1", "H2"]);
        assert_eq!(table.rows, vec![vec!["a", "b"]]);
    }

    #[test]
    fn table_into_cell_output() {
        let table = Table::new(vec!["X", "Y"], vec![vec!["1", "2"]]);
        let output = CellOutput::from(table);
        assert_eq!(output.type_tag.as_deref(), Some("Table"));
        assert!(!output.bytes.is_empty());
        assert_eq!(
            output.panels,
            vec![DisplayPanel::Table {
                headers: vec!["X".into(), "Y".into()],
                rows: vec![vec!["1".into(), "2".into()]],
            }]
        );

        // Verify round-trip deserialization.
        let input = CellInput::new(&output.bytes);
        let decoded: Table = input.deserialize().expect("deserialize Table");
        assert_eq!(decoded.headers, vec!["X", "Y"]);
        assert_eq!(decoded.rows, vec![vec!["1".to_string(), "2".to_string()]]);
    }

    #[test]
    fn table_into_panels() {
        let table = Table::new(vec!["A"], vec![vec!["val"]]);
        let panels = table.into_panels();
        assert_eq!(
            panels,
            vec![DisplayPanel::Table {
                headers: vec!["A".into()],
                rows: vec![vec!["val".into()]],
            }]
        );
    }

    #[test]
    fn table_type_tag() {
        assert_eq!(Table::type_tag(), "Table");
    }

    #[test]
    fn table_display_panel_json_roundtrip() {
        let panel = DisplayPanel::Table {
            headers: vec!["Name".into(), "Score".into()],
            rows: vec![
                vec!["Alice".into(), "100".into()],
                vec!["Bob".into(), "85".into()],
            ],
        };
        let json = serde_json::to_string(&panel).expect("serialize");
        let decoded: DisplayPanel = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(panel, decoded);
    }

    #[test]
    fn table_in_tuple() {
        let table = Table::new(vec!["Col"], vec![vec!["val"]]);
        let output = CellOutput::from((42u32, table));
        assert_eq!(output.type_tag.as_deref(), Some("(u32, Table)"));
        assert_eq!(output.panels.len(), 2);
        assert_eq!(output.panels[0], DisplayPanel::Text("42".into()));
        assert_eq!(
            output.panels[1],
            DisplayPanel::Table {
                headers: vec!["Col".into()],
                rows: vec![vec!["val".into()]],
            }
        );
    }

    #[test]
    fn tuple_with_md() {
        let output = CellOutput::from((42u32, Md("# Title".into())));
        assert_eq!(output.type_tag.as_deref(), Some("(u32, Md)"));
        assert_eq!(
            output.panels,
            vec![
                DisplayPanel::Text("42".into()),
                DisplayPanel::Markdown("# Title".into()),
            ]
        );
    }

    // ── Json tests ───────────────────────────────────────────────────────

    #[test]
    fn json_from_str_valid() {
        let json: Json = r#"{"a": 1}"#.parse().expect("valid JSON");
        assert_eq!(json.0, serde_json::json!({"a": 1}));
    }

    #[test]
    fn json_from_str_invalid() {
        let result = "not json {{{".parse::<Json>();
        assert!(result.is_err());
    }

    /// Keys are written in sorted order so the fixture renders the same
    /// whether or not a workspace dependency turns on `serde_json`'s
    /// `preserve_order` (feature unification decides that, not this crate).
    fn json_golden_fixture() -> serde_json::Value {
        serde_json::json!({
            "array": [1, {"b": false}, [], "x"],
            "bool": true,
            "empty_array": [],
            "empty_object": {},
            "nested": {"inner": [2.5, -3]},
            "null": null,
            "number": 42,
            "string": "<a href='x'>&\"",
            "z<k>&": "v"
        })
    }

    /// Byte-exact pin on the renderer. The substring tests below survive a
    /// rewrite that changes spacing, nesting or escaping; this one does not.
    #[test]
    fn json_render_golden() {
        let expected = concat!(
            r##"<pre style="margin:0; font-family:monospace; line-height:1.5"><span style="color:#eaeaea">{</span>"##,
            "\n",
            r##"  <span style="color:#e94560">&quot;array&quot;</span><span style="color:#eaeaea">: </span><span style="color:#eaeaea">[</span>"##,
            "\n",
            r##"    <span style="color:#40a0f0">1</span><span style="color:#eaeaea">,</span>"##,
            "\n",
            r##"    <span style="color:#eaeaea">{</span>"##,
            "\n",
            r##"      <span style="color:#e94560">&quot;b&quot;</span><span style="color:#eaeaea">: </span><span style="color:#b080f0">false</span>"##,
            "\n",
            r##"    <span style="color:#eaeaea">}</span><span style="color:#eaeaea">,</span>"##,
            "\n",
            r##"    <span style="color:#eaeaea">[]</span><span style="color:#eaeaea">,</span>"##,
            "\n",
            r##"    <span style="color:#40c080">&quot;x&quot;</span>"##,
            "\n",
            r##"  <span style="color:#eaeaea">]</span><span style="color:#eaeaea">,</span>"##,
            "\n",
            r##"  <span style="color:#e94560">&quot;bool&quot;</span><span style="color:#eaeaea">: </span><span style="color:#b080f0">true</span><span style="color:#eaeaea">,</span>"##,
            "\n",
            r##"  <span style="color:#e94560">&quot;empty_array&quot;</span><span style="color:#eaeaea">: </span><span style="color:#eaeaea">[]</span><span style="color:#eaeaea">,</span>"##,
            "\n",
            r##"  <span style="color:#e94560">&quot;empty_object&quot;</span><span style="color:#eaeaea">: </span><span style="color:#eaeaea">{}</span><span style="color:#eaeaea">,</span>"##,
            "\n",
            r##"  <span style="color:#e94560">&quot;nested&quot;</span><span style="color:#eaeaea">: </span><span style="color:#eaeaea">{</span>"##,
            "\n",
            r##"    <span style="color:#e94560">&quot;inner&quot;</span><span style="color:#eaeaea">: </span><span style="color:#eaeaea">[</span>"##,
            "\n",
            r##"      <span style="color:#40a0f0">2.5</span><span style="color:#eaeaea">,</span>"##,
            "\n",
            r##"      <span style="color:#40a0f0">-3</span>"##,
            "\n",
            r##"    <span style="color:#eaeaea">]</span>"##,
            "\n",
            r##"  <span style="color:#eaeaea">}</span><span style="color:#eaeaea">,</span>"##,
            "\n",
            r##"  <span style="color:#e94560">&quot;null&quot;</span><span style="color:#eaeaea">: </span><span style="color:#b080f0">null</span><span style="color:#eaeaea">,</span>"##,
            "\n",
            r##"  <span style="color:#e94560">&quot;number&quot;</span><span style="color:#eaeaea">: </span><span style="color:#40a0f0">42</span><span style="color:#eaeaea">,</span>"##,
            "\n",
            r##"  <span style="color:#e94560">&quot;string&quot;</span><span style="color:#eaeaea">: </span><span style="color:#40c080">&quot;&lt;a href=&#x27;x&#x27;&gt;&amp;&quot;&quot;</span><span style="color:#eaeaea">,</span>"##,
            "\n",
            r##"  <span style="color:#e94560">&quot;z&lt;k&gt;&amp;&quot;</span><span style="color:#eaeaea">: </span><span style="color:#40c080">&quot;v&quot;</span>"##,
            "\n",
            r##"<span style="color:#eaeaea">}</span></pre>"##,
        );
        assert_eq!(render_json_html(&json_golden_fixture()), expected);
    }

    #[test]
    fn json_render_contains_pre_tag() {
        let html = render_json_html(&serde_json::json!({"key": "value"}));
        assert!(html.starts_with("<pre "));
        assert!(html.ends_with("</pre>"));
    }

    #[test]
    fn json_render_colors_keys_strings_numbers_booleans() {
        let val = serde_json::json!({
            "name": "alice",
            "age": 30,
            "active": true,
            "extra": null
        });
        let html = render_json_html(&val);

        // Keys colored with accent red.
        assert!(html.contains("#e94560"), "keys should use #e94560");
        // String values colored green.
        assert!(html.contains("#40c080"), "strings should use #40c080");
        // Numbers colored blue.
        assert!(html.contains("#40a0f0"), "numbers should use #40a0f0");
        // Booleans/null colored purple.
        assert!(html.contains("#b080f0"), "bools/null should use #b080f0");
        // Punctuation colored light.
        assert!(html.contains("#eaeaea"), "punctuation should use #eaeaea");
    }

    #[test]
    fn json_into_cell_output_html_panel() {
        let json = Json(serde_json::json!({"x": 1}));
        let output = CellOutput::from(json);
        assert_eq!(output.type_tag.as_deref(), Some("Json"));
        assert_eq!(output.panels.len(), 1);
        match &output.panels[0] {
            DisplayPanel::Html(html) => assert!(html.contains("<pre ")),
            other => panic!("expected Html panel, got {other:?}"),
        }
    }

    #[test]
    fn json_html_escapes_values() {
        let json = Json(serde_json::json!({"key": "<script>alert('xss')</script>"}));
        let output = CellOutput::from(json);
        match &output.panels[0] {
            DisplayPanel::Html(html) => {
                assert!(!html.contains("<script>"), "HTML should be escaped");
                assert!(html.contains("&lt;script&gt;"));
            }
            other => panic!("expected Html panel, got {other:?}"),
        }
    }

    #[test]
    fn json_into_panels_trait() {
        let json = Json(serde_json::json!([1, 2, 3]));
        let panels = json.into_panels();
        assert_eq!(panels.len(), 1);
        match &panels[0] {
            DisplayPanel::Html(html) => {
                assert!(html.contains("<pre "));
                assert!(html.contains("#40a0f0")); // numbers
            }
            other => panic!("expected Html panel, got {other:?}"),
        }
    }

    #[test]
    fn json_type_tag() {
        assert_eq!(Json::type_tag(), "Json");
    }

    // ── Host messaging ──────────────────────────────────────────────────

    #[test]
    fn host_message_noop_on_native() {
        // Should not panic on non-wasm targets.
        host_message("{\"type\":\"test\"}");
        host_message_json(&serde_json::json!({"type": "test", "value": 42}));
    }

    // ── Animation ───────────────────────────────────────────────────────

    #[test]
    fn animation_new_and_accessors() {
        let f1 = canvas::Canvas::new(2, 2);
        let f2 = canvas::Canvas::new(2, 2);
        let anim = canvas::Animation::new(vec![f1, f2], 10);
        assert_eq!(anim.fps(), 10);
        assert_eq!(anim.frames().len(), 2);
        assert_eq!(anim.frames()[0].width(), 2);
    }

    #[test]
    fn animation_empty_frames_yields_empty_panels() {
        let anim = canvas::Animation::new(vec![], 24);
        let panels = anim.into_panels();
        assert!(panels.is_empty());
    }

    #[test]
    fn animation_into_panels_correct_variant() {
        let mut f = canvas::Canvas::new(3, 2);
        f.set_pixel(0, 0, (255, 0, 0));
        let anim = canvas::Animation::new(vec![f.clone(), f], 15);
        let panels = anim.into_panels();
        assert_eq!(panels.len(), 1);
        match &panels[0] {
            DisplayPanel::Animation {
                width,
                height,
                fps,
                frame_count,
                data,
            } => {
                assert_eq!(*width, 3);
                assert_eq!(*height, 2);
                assert_eq!(*fps, 15);
                assert_eq!(*frame_count, 2);
                assert!(!data.is_empty());
            }
            other => panic!("expected Animation panel, got {other:?}"),
        }
    }

    #[test]
    fn animation_per_frame_base64_equals_concatenated() {
        // Odd dimensions, so a frame's byte count (45) is a multiple of 3 but
        // not of 4 or 2: a join that padded per frame would show up here.
        let frames: Vec<canvas::Canvas> = (0..3u8)
            .map(|k| {
                canvas::Canvas::from_fn(5, 3, |x, y| {
                    #[allow(clippy::cast_possible_truncation)]
                    let v = (x * 7 + y * 13) as u8;
                    (v.wrapping_add(k), v ^ k, k.wrapping_mul(31))
                })
            })
            .collect();
        let concatenated: Vec<u8> = frames
            .iter()
            .flat_map(|f| f.pixels().iter().copied())
            .collect();
        let expected = canvas::base64_encode(&concatenated);

        let panels = canvas::Animation::new(frames, 12).into_panels();
        let [DisplayPanel::Animation { data, .. }] = panels.as_slice() else {
            panic!("expected one Animation panel, got {panels:?}");
        };
        assert_eq!(data, &expected);
    }

    #[test]
    fn animation_from_cell_output() {
        let f = canvas::Canvas::new(2, 2);
        let anim = canvas::Animation::new(vec![f], 30);
        let output = CellOutput::from(anim);
        // No type tag: the output is display-only with empty bytes, so a
        // downstream cell must not bind (and then panic deserializing) it.
        assert_eq!(output.type_tag, None);
        assert!(output.bytes.is_empty());
        assert_eq!(output.panels.len(), 1);
    }

    #[test]
    fn animation_type_tag() {
        assert_eq!(canvas::Animation::type_tag(), "Animation");
    }

    // ── Simulation / TickResult ─────────────────────────────────────────

    #[test]
    fn simulation_meta_into_cell_output() {
        let frame = canvas::Canvas::new(4, 3);
        let meta = SimulationMeta {
            width: 4,
            height: 3,
            fps: 60,
            first_frame: frame,
            sliders: vec![],
        };
        let output = CellOutput::from(meta);
        // No type tag: a simulation is display-only, and a `Simulation` tag would
        // make a downstream code cell declare `let cellN: Simulation` (a trait).
        assert!(output.type_tag.is_none());
        assert!(output.bytes.is_empty());
        assert_eq!(output.panels.len(), 1);
        match &output.panels[0] {
            DisplayPanel::Simulation {
                width,
                height,
                fps,
                first_frame_data,
                ..
            } => {
                assert_eq!(*width, 4);
                assert_eq!(*height, 3);
                assert_eq!(*fps, 60);
                assert!(!first_frame_data.is_empty());
            }
            other => panic!("expected Simulation panel, got {other:?}"),
        }
    }

    #[test]
    fn tick_result_from_canvas() {
        let mut c = canvas::Canvas::new(2, 2);
        c.set_pixel(0, 0, (255, 128, 64));
        let tr = TickResult::from(c);
        assert_eq!(tr.width, 2);
        assert_eq!(tr.height, 2);

        // SAFETY: the pixel buffer was leaked by `From<Canvas> for TickResult`.
        let rgb = unsafe { reclaim(tr.rgb_ptr, tr.rgb_len) };
        assert_eq!(rgb.len(), 2 * 2 * 3);
        assert_eq!(&rgb[..3], &[255, 128, 64]);
    }

    #[test]
    fn empty_canvas_tick_result_is_null_zero() {
        // A 0x0 frame crosses as (null, 0), the same empty-case contract as
        // `CellResult`, not as a dangling pointer to a zero-length box.
        let tr = TickResult::from(canvas::Canvas::new(0, 0));
        assert!(tr.rgb_ptr.is_null());
        assert_eq!(tr.rgb_len, 0);
    }

    // ── LiveView / LiveTickResult ──────────────────────────────────────────

    #[test]
    fn live_view_meta_into_cell_output() {
        let meta = LiveViewMeta {
            fps: 10,
            initial_content: LiveContent::Html("<b>hello</b>".into()),
        };
        let output = CellOutput::from(meta);
        // No type tag: a live view is display-only, and a `LiveView` tag would
        // make a downstream code cell declare `let cellN: LiveView` (a trait).
        assert!(output.type_tag.is_none());
        assert!(output.bytes.is_empty());
        assert_eq!(output.panels.len(), 1);
        match &output.panels[0] {
            DisplayPanel::LiveView { fps, kind, content } => {
                assert_eq!(*fps, 10);
                assert_eq!(kind, "html");
                assert_eq!(content, "<b>hello</b>");
            }
            other => panic!("expected LiveView panel, got {other:?}"),
        }
    }

    #[test]
    fn live_tick_result_from_text() {
        let content = LiveContent::Text("hello world".into());
        let result = LiveTickResult::from(content);
        assert_eq!(result.kind, 0);
        // SAFETY: the content was leaked by `From<LiveContent>`.
        let bytes = unsafe { reclaim(result.content_ptr, result.content_len) };
        assert_eq!(bytes, b"hello world");
    }

    #[test]
    fn live_tick_result_from_html() {
        let content = LiveContent::Html("<b>hi</b>".into());
        let result = LiveTickResult::from(content);
        assert_eq!(result.kind, 1);
        // SAFETY: the content was leaked by `From<LiveContent>`.
        let bytes = unsafe { reclaim(result.content_ptr, result.content_len) };
        assert_eq!(bytes, b"<b>hi</b>");
    }

    #[test]
    fn live_tick_result_from_markdown() {
        let content = LiveContent::Markdown("# Title".into());
        let result = LiveTickResult::from(content);
        assert_eq!(result.kind, 2);
        // SAFETY: the content was leaked by `From<LiveContent>`.
        let bytes = unsafe { reclaim(result.content_ptr, result.content_len) };
        assert_eq!(bytes, b"# Title");
    }

    #[test]
    fn empty_live_content_is_null_zero() {
        let result = LiveTickResult::from(LiveContent::Text(String::new()));
        assert_eq!(result.kind, 0);
        assert!(result.content_ptr.is_null());
        assert_eq!(result.content_len, 0);
    }
}
