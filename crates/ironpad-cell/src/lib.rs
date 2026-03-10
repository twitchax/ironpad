//! ironpad-cell — injected into every user cell as a dependency.
//!
//! Provides [`CellInput`], [`CellOutput`], [`CellResult`], and memory FFI
//! helpers (`ironpad_alloc` / `ironpad_dealloc`).  The [`prelude`] module
//! re-exports the essential items so user cells can simply write:
//!
//! ```ignore
//! use ironpad_cell::prelude::*;
//! ```

// ── Prelude ──────────────────────────────────────────────────────────────────

pub mod prelude {
    pub use bincode;
    pub use serde::{Deserialize, Serialize};

    #[cfg(target_arch = "wasm32")]
    pub use js_sys;
    #[cfg(target_arch = "wasm32")]
    pub use reqwest;
    #[cfg(target_arch = "wasm32")]
    pub use wasm_bindgen::prelude::*;
    #[cfg(target_arch = "wasm32")]
    pub use wasm_bindgen_futures;

    pub use crate::{
        CellInput, CellInputs, CellOutput, CellResult, DisplayPanel, Html, IntoPanels, Md, Svg,
        Table, TypeTag,
    };

    #[cfg(target_arch = "wasm32")]
    pub use super::http;
}

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
        if bytes.is_empty() {
            return Self { data: Vec::new() };
        }

        let mut offset = 0;

        let count = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;

        let mut data = Vec::with_capacity(count);
        for _ in 0..count {
            let len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            data.push(bytes[offset..offset + len].to_vec());
            offset += len;
        }

        Self { data }
    }

    /// Encode a list of output byte slices into the wire format.
    /// This is used by the frontend to package all previous cell outputs.
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

/// Implement `From<T> for CellOutput` for primitive types that implement both
/// `Serialize` and `Display`.  Each conversion serializes the value with bincode
/// (for piping to the next cell) and sets a human-readable display string.
macro_rules! impl_from_for_cell_output {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl From<$ty> for CellOutput {
                fn from(value: $ty) -> Self {
                    let bytes = bincode::serde::encode_to_vec(&value, bincode::config::standard())
                        .expect("serialization of primitive types cannot fail");
                    Self {
                        bytes,
                        panels: vec![DisplayPanel::Text(value.to_string())],
                        type_tag: Some(stringify!($ty).into()),
                    }
                }
            }
        )+
    };
}

impl_from_for_cell_output!(
    i8, i16, i32, i64, i128, u8, u16, u32, u64, u128, f32, f64, bool, usize, isize,
);

impl From<String> for CellOutput {
    fn from(value: String) -> Self {
        let bytes = bincode::serde::encode_to_vec(&value, bincode::config::standard())
            .expect("serialization of String cannot fail");
        Self {
            bytes,
            panels: vec![DisplayPanel::Text(value)],
            type_tag: Some("String".into()),
        }
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
    fn from(_: ()) -> Self {
        Self::empty()
    }
}

impl<T: serde::Serialize + std::fmt::Debug> From<Vec<T>> for CellOutput {
    fn from(value: Vec<T>) -> Self {
        let type_tag = Some(clean_type_name(std::any::type_name::<Vec<T>>()));
        let display = format!("{value:?}");
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
        Self {
            bytes: vec![],
            panels: vec![DisplayPanel::Svg(value.0)],
            type_tag: Some("Svg".into()),
        }
    }
}

impl From<Html> for CellOutput {
    fn from(value: Html) -> Self {
        Self {
            bytes: vec![],
            panels: vec![DisplayPanel::Html(value.0)],
            type_tag: Some("Html".into()),
        }
    }
}

impl From<Table> for CellOutput {
    fn from(value: Table) -> Self {
        Self {
            bytes: vec![],
            panels: vec![DisplayPanel::Table {
                headers: value.headers,
                rows: value.rows,
            }],
            type_tag: Some("Table".into()),
        }
    }
}

impl From<Md> for CellOutput {
    fn from(value: Md) -> Self {
        Self {
            bytes: vec![],
            panels: vec![DisplayPanel::Markdown(value.0)],
            type_tag: Some("Md".into()),
        }
    }
}

// ── IntoPanels trait ─────────────────────────────────────────────────────────

/// Trait for types that can produce display panels.
pub trait IntoPanels {
    #[allow(clippy::wrong_self_convention)]
    fn into_panels(&self) -> Vec<DisplayPanel>;
}

macro_rules! impl_into_panels_for_primitive {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl IntoPanels for $ty {
                fn into_panels(&self) -> Vec<DisplayPanel> {
                    vec![DisplayPanel::Text(format!("{self}"))]
                }
            }
        )+
    };
}

impl_into_panels_for_primitive!(
    i8, i16, i32, i64, i128, u8, u16, u32, u64, u128, f32, f64, bool, usize, isize,
);

impl IntoPanels for String {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Text(self.clone())]
    }
}

impl<T: std::fmt::Debug> IntoPanels for Vec<T> {
    fn into_panels(&self) -> Vec<DisplayPanel> {
        vec![DisplayPanel::Text(format!("{self:?}"))]
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

macro_rules! impl_type_tag_for_primitive {
    ($($ty:ty),+ $(,)?) => {
        $(
            impl TypeTag for $ty {
                fn type_tag() -> String {
                    stringify!($ty).into()
                }
            }
        )+
    };
}

impl_type_tag_for_primitive!(
    i8, i16, i32, i64, i128, u8, u16, u32, u64, u128, f32, f64, bool, usize, isize,
);

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
fn vec_into_raw(mut v: Vec<u8>) -> (*mut u8, usize) {
    if v.is_empty() {
        return (std::ptr::null_mut(), 0);
    }

    v.shrink_to_fit();
    let ptr = v.as_mut_ptr();
    let len = v.len();
    std::mem::forget(v);
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

        let result: CellResult = output.into();
        assert!(!result.output_ptr.is_null());
        assert!(result.output_len > 0);

        // Reconstruct the bytes to feed into CellInput.
        let bytes = unsafe { std::slice::from_raw_parts(result.output_ptr, result.output_len) };
        let input = CellInput::new(bytes);
        let decoded: Point = input.deserialize().expect("deserialize");
        assert_eq!(decoded, original);

        // Clean up leaked memory.
        unsafe {
            drop(Vec::from_raw_parts(
                result.output_ptr,
                result.output_len,
                result.output_len,
            ));
        }
    }

    #[test]
    fn round_trip_vec() {
        let data: Vec<i32> = vec![10, 20, 30];
        let output = CellOutput::new(&data).expect("serialize");

        let result: CellResult = output.into();
        let bytes = unsafe { std::slice::from_raw_parts(result.output_ptr, result.output_len) };
        let input = CellInput::new(bytes);
        let decoded: Vec<i32> = input.deserialize().expect("deserialize");
        assert_eq!(decoded, data);

        unsafe {
            drop(Vec::from_raw_parts(
                result.output_ptr,
                result.output_len,
                result.output_len,
            ));
        }
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
        let result: CellResult = output.into();
        assert!(result.output_ptr.is_null());
        assert_eq!(result.output_len, 0);
        assert!(result.display_ptr.is_null());
        assert_eq!(result.display_len, 0);
        assert!(result.type_tag_ptr.is_null());
        assert_eq!(result.type_tag_len, 0);
    }

    #[test]
    fn output_text_only() {
        let output = CellOutput::text("hello world");
        assert!(output.type_tag.is_none());
        assert_eq!(
            output.panels,
            vec![DisplayPanel::Text("hello world".into())]
        );
        let result: CellResult = output.into();

        assert!(result.output_ptr.is_null());
        assert_eq!(result.output_len, 0);

        assert!(!result.display_ptr.is_null());
        assert!(result.display_len > 0);
        assert!(result.type_tag_ptr.is_null());
        assert_eq!(result.type_tag_len, 0);

        let display = unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                result.display_ptr,
                result.display_len,
            ))
            .to_string()
        };
        let panels: Vec<DisplayPanel> = serde_json::from_str(&display).expect("parse JSON panels");
        assert_eq!(panels, vec![DisplayPanel::Text("hello world".into())]);

        unsafe {
            drop(Vec::from_raw_parts(
                result.display_ptr,
                result.display_len,
                result.display_len,
            ));
        }
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
        let result: CellResult = output.into();

        assert!(!result.output_ptr.is_null());
        assert!(result.output_len > 0);
        assert!(!result.display_ptr.is_null());
        assert!(result.display_len > 0);
        assert!(result.type_tag_ptr.is_null());
        assert_eq!(result.type_tag_len, 0);

        let display = unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                result.display_ptr,
                result.display_len,
            ))
            .to_string()
        };
        let panels: Vec<DisplayPanel> = serde_json::from_str(&display).expect("parse JSON panels");
        assert_eq!(panels, vec![DisplayPanel::Text("The answer is 42".into())]);

        // Clean up.
        unsafe {
            drop(Vec::from_raw_parts(
                result.output_ptr,
                result.output_len,
                result.output_len,
            ));
            drop(Vec::from_raw_parts(
                result.display_ptr,
                result.display_len,
                result.display_len,
            ));
        }
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
        let result: CellResult = output.into();

        // Should have serialized bytes.
        assert!(!result.output_ptr.is_null());
        assert!(result.output_len > 0);

        // Verify display JSON contains panels.
        let display = unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                result.display_ptr,
                result.display_len,
            ))
            .to_string()
        };
        let panels: Vec<DisplayPanel> = serde_json::from_str(&display).expect("parse JSON panels");
        assert_eq!(panels, vec![DisplayPanel::Text("42".into())]);

        // Verify type tag.
        let tag = unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                result.type_tag_ptr,
                result.type_tag_len,
            ))
            .to_string()
        };
        assert_eq!(tag, "i32");

        // Verify round-trip via CellInput.
        let bytes = unsafe { std::slice::from_raw_parts(result.output_ptr, result.output_len) };
        let input = CellInput::new(bytes);
        let decoded: i32 = input.deserialize().expect("deserialize i32");
        assert_eq!(decoded, 42);

        unsafe {
            drop(Vec::from_raw_parts(
                result.output_ptr,
                result.output_len,
                result.output_len,
            ));
            drop(Vec::from_raw_parts(
                result.display_ptr,
                result.display_len,
                result.display_len,
            ));
            drop(Vec::from_raw_parts(
                result.type_tag_ptr,
                result.type_tag_len,
                result.type_tag_len,
            ));
        }
    }

    #[test]
    fn from_f64_serializes_and_displays() {
        let output = CellOutput::from(42.5f64);
        assert_eq!(output.type_tag.as_deref(), Some("f64"));
        assert_eq!(output.panels, vec![DisplayPanel::Text("42.5".into())]);
        let result: CellResult = output.into();

        let display = unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                result.display_ptr,
                result.display_len,
            ))
            .to_string()
        };
        let panels: Vec<DisplayPanel> = serde_json::from_str(&display).expect("parse JSON panels");
        assert_eq!(panels, vec![DisplayPanel::Text("42.5".into())]);

        let bytes = unsafe { std::slice::from_raw_parts(result.output_ptr, result.output_len) };
        let input = CellInput::new(bytes);
        let decoded: f64 = input.deserialize().expect("deserialize f64");
        assert!((decoded - 42.5).abs() < f64::EPSILON);

        unsafe {
            drop(Vec::from_raw_parts(
                result.output_ptr,
                result.output_len,
                result.output_len,
            ));
            drop(Vec::from_raw_parts(
                result.display_ptr,
                result.display_len,
                result.display_len,
            ));
            drop(Vec::from_raw_parts(
                result.type_tag_ptr,
                result.type_tag_len,
                result.type_tag_len,
            ));
        }
    }

    #[test]
    fn from_bool_serializes_and_displays() {
        let output = CellOutput::from(true);
        assert_eq!(output.type_tag.as_deref(), Some("bool"));
        assert_eq!(output.panels, vec![DisplayPanel::Text("true".into())]);
        let result: CellResult = output.into();

        let display = unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                result.display_ptr,
                result.display_len,
            ))
            .to_string()
        };
        let panels: Vec<DisplayPanel> = serde_json::from_str(&display).expect("parse JSON panels");
        assert_eq!(panels, vec![DisplayPanel::Text("true".into())]);

        unsafe {
            drop(Vec::from_raw_parts(
                result.output_ptr,
                result.output_len,
                result.output_len,
            ));
            drop(Vec::from_raw_parts(
                result.display_ptr,
                result.display_len,
                result.display_len,
            ));
            drop(Vec::from_raw_parts(
                result.type_tag_ptr,
                result.type_tag_len,
                result.type_tag_len,
            ));
        }
    }

    #[test]
    fn from_string_serializes_and_displays() {
        let output = CellOutput::from("hello world".to_string());
        assert_eq!(output.type_tag.as_deref(), Some("String"));
        assert_eq!(
            output.panels,
            vec![DisplayPanel::Text("hello world".into())]
        );
        let result: CellResult = output.into();

        let display = unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                result.display_ptr,
                result.display_len,
            ))
            .to_string()
        };
        let panels: Vec<DisplayPanel> = serde_json::from_str(&display).expect("parse JSON panels");
        assert_eq!(panels, vec![DisplayPanel::Text("hello world".into())]);

        let bytes = unsafe { std::slice::from_raw_parts(result.output_ptr, result.output_len) };
        let input = CellInput::new(bytes);
        let decoded: String = input.deserialize().expect("deserialize String");
        assert_eq!(decoded, "hello world");

        unsafe {
            drop(Vec::from_raw_parts(
                result.output_ptr,
                result.output_len,
                result.output_len,
            ));
            drop(Vec::from_raw_parts(
                result.display_ptr,
                result.display_len,
                result.display_len,
            ));
            drop(Vec::from_raw_parts(
                result.type_tag_ptr,
                result.type_tag_len,
                result.type_tag_len,
            ));
        }
    }

    #[test]
    fn from_str_ref_serializes_and_displays() {
        let output = CellOutput::from("hello");
        assert_eq!(output.type_tag.as_deref(), Some("String"));
        assert_eq!(output.panels, vec![DisplayPanel::Text("hello".into())]);
        let result: CellResult = output.into();

        let display = unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                result.display_ptr,
                result.display_len,
            ))
            .to_string()
        };
        let panels: Vec<DisplayPanel> = serde_json::from_str(&display).expect("parse JSON panels");
        assert_eq!(panels, vec![DisplayPanel::Text("hello".into())]);

        unsafe {
            drop(Vec::from_raw_parts(
                result.output_ptr,
                result.output_len,
                result.output_len,
            ));
            drop(Vec::from_raw_parts(
                result.display_ptr,
                result.display_len,
                result.display_len,
            ));
            drop(Vec::from_raw_parts(
                result.type_tag_ptr,
                result.type_tag_len,
                result.type_tag_len,
            ));
        }
    }

    #[test]
    fn from_unit_produces_empty_output() {
        let output = CellOutput::from(());
        assert!(output.type_tag.is_none());
        let result: CellResult = output.into();

        assert!(result.output_ptr.is_null());
        assert_eq!(result.output_len, 0);
        assert!(result.display_ptr.is_null());
        assert_eq!(result.display_len, 0);
        assert!(result.type_tag_ptr.is_null());
        assert_eq!(result.type_tag_len, 0);
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
        let result: CellResult = output.into();

        // Verify display JSON contains panels with Debug format.
        let display = unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                result.display_ptr,
                result.display_len,
            ))
            .to_string()
        };
        let panels: Vec<DisplayPanel> = serde_json::from_str(&display).expect("parse JSON panels");
        assert_eq!(panels, vec![DisplayPanel::Text("[10, 20, 30]".into())]);

        // Verify type tag through CellResult.
        let tag = unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                result.type_tag_ptr,
                result.type_tag_len,
            ))
            .to_string()
        };
        assert_eq!(tag, "Vec<i32>");

        // Verify round-trip via CellInput.
        let bytes = unsafe { std::slice::from_raw_parts(result.output_ptr, result.output_len) };
        let input = CellInput::new(bytes);
        let decoded: Vec<i32> = input.deserialize().expect("deserialize Vec<i32>");
        assert_eq!(decoded, data);

        unsafe {
            drop(Vec::from_raw_parts(
                result.output_ptr,
                result.output_len,
                result.output_len,
            ));
            drop(Vec::from_raw_parts(
                result.display_ptr,
                result.display_len,
                result.display_len,
            ));
            drop(Vec::from_raw_parts(
                result.type_tag_ptr,
                result.type_tag_len,
                result.type_tag_len,
            ));
        }
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
        assert!(output.bytes.is_empty());
    }

    #[test]
    fn html_newtype_into_cell_output() {
        let output = CellOutput::from(Html("<b>bold</b>".into()));
        assert_eq!(output.type_tag.as_deref(), Some("Html"));
        assert_eq!(
            output.panels,
            vec![DisplayPanel::Html("<b>bold</b>".into())]
        );
        assert!(output.bytes.is_empty());
    }

    #[test]
    fn md_newtype_into_cell_output() {
        let output = CellOutput::from(Md("# Hello\n\nworld".into()));
        assert_eq!(output.type_tag.as_deref(), Some("Md"));
        assert_eq!(
            output.panels,
            vec![DisplayPanel::Markdown("# Hello\n\nworld".into())]
        );
        assert!(output.bytes.is_empty());
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
        assert!(output.bytes.is_empty());
        assert_eq!(
            output.panels,
            vec![DisplayPanel::Table {
                headers: vec!["X".into(), "Y".into()],
                rows: vec![vec!["1".into(), "2".into()]],
            }]
        );
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
}
