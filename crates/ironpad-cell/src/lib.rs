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

    pub use crate::{CellInput, CellOutput, CellResult};
}

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

// ── CellOutput ───────────────────────────────────────────────────────────────

/// Output produced by a cell.
///
/// Contains optional binary data (forwarded to the next cell via bincode) and
/// an optional human-readable display string shown in the UI output panel.
pub struct CellOutput {
    bytes: Vec<u8>,
    display: Option<String>,
}

impl CellOutput {
    /// Serialize `value` with bincode and store the bytes as output.
    pub fn new<T: serde::Serialize>(value: &T) -> Result<Self, bincode::error::EncodeError> {
        let bytes = bincode::serde::encode_to_vec(value, bincode::config::standard())?;
        Ok(Self {
            bytes,
            display: None,
        })
    }

    /// Attach a human-readable display string to this output.
    pub fn with_display(mut self, text: String) -> Self {
        self.display = Some(text);
        self
    }

    /// Create an empty output (no data, no display text).
    pub fn empty() -> Self {
        Self {
            bytes: vec![],
            display: None,
        }
    }

    /// Create a display-only output with no binary payload.
    pub fn text(msg: impl Into<String>) -> Self {
        let msg = msg.into();
        Self {
            bytes: vec![],
            display: Some(msg),
        }
    }
}

// ── CellResult (FFI) ────────────────────────────────────────────────────────

/// FFI-compatible result struct returned from `cell_main`.
///
/// The WASM host reads these four fields to extract the output bytes and
/// display text from linear memory.
#[repr(C)]
pub struct CellResult {
    pub output_ptr: *mut u8,
    pub output_len: usize,
    pub display_ptr: *mut u8,
    pub display_len: usize,
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

        let (display_ptr, display_len) = match output.display {
            Some(s) => vec_into_raw(s.into_bytes()),
            None => (std::ptr::null_mut(), 0),
        };

        CellResult {
            output_ptr,
            output_len,
            display_ptr,
            display_len,
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
        assert_eq!(input.raw(), &[]);
    }

    #[test]
    fn input_raw_bytes() {
        let bytes = [1u8, 2, 3, 4];
        let input = CellInput::new(&bytes);
        assert!(!input.is_empty());
        assert_eq!(input.raw(), &[1, 2, 3, 4]);
    }

    // ── CellOutput constructors ──────────────────────────────────────────

    #[test]
    fn output_empty() {
        let output = CellOutput::empty();
        let result: CellResult = output.into();
        assert!(result.output_ptr.is_null());
        assert_eq!(result.output_len, 0);
        assert!(result.display_ptr.is_null());
        assert_eq!(result.display_len, 0);
    }

    #[test]
    fn output_text_only() {
        let output = CellOutput::text("hello world");
        let result: CellResult = output.into();

        assert!(result.output_ptr.is_null());
        assert_eq!(result.output_len, 0);

        assert!(!result.display_ptr.is_null());
        assert!(result.display_len > 0);

        let display = unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                result.display_ptr,
                result.display_len,
            ))
            .to_string()
        };
        assert_eq!(display, "hello world");

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

        let result: CellResult = output.into();

        assert!(!result.output_ptr.is_null());
        assert!(result.output_len > 0);
        assert!(!result.display_ptr.is_null());
        assert!(result.display_len > 0);

        let display = unsafe {
            String::from_utf8_lossy(std::slice::from_raw_parts(
                result.display_ptr,
                result.display_len,
            ))
            .to_string()
        };
        assert_eq!(display, "The answer is 42");

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
        // Verify the size matches 4 pointer-sized fields.
        let expected = 4 * std::mem::size_of::<usize>();
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
}
