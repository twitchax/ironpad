//! Simulation bus API — emit named values from WASM cells and read them back.
//!
//! All three functions work on both `wasm32` and native targets. On native the
//! functions are no-ops / stubs; the bus only exists inside the JS executor.

// ── FFI imports (wasm32 only) ────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    /// Read the latest value for `key` from the JS sim bus.
    ///
    /// Returns a pointer to a `[u32-LE length][N bytes UTF-8 JSON]` buffer
    /// allocated in WASM linear memory via `ironpad_alloc`.  Returns `0` if no
    /// value has been emitted for the key.  Caller must free with
    /// `ironpad_dealloc(ptr, 4 + length)`.
    fn ironpad_sim_read(key_ptr: *const u8, key_len: u32) -> u32;

    /// Read all buffered values for `key` (ring buffer, oldest-first).
    ///
    /// Same return protocol as [`ironpad_sim_read`] but the JSON is an array.
    fn ironpad_sim_read_all(key_ptr: *const u8, key_len: u32) -> u32;
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Emit a named value to the simulation bus.
///
/// Serialises `value` as JSON and sends it to the JS executor via the
/// `host_message` channel with `{"type": "sim_emit", "key": key, "value": …}`.
/// The executor maintains a ring buffer of the last 1 000 values per key.
pub fn emit<T: serde::Serialize>(key: &str, value: &T) {
    crate::host_message_json(&serde_json::json!({
        "type": "sim_emit",
        "key": key,
        "value": value,
    }));
}

/// Read the latest value emitted for `key`.
///
/// Returns `None` if no value has been emitted yet, or if deserialisation
/// fails.  On non-`wasm32` targets this always returns `None`.
pub fn read<T: serde::de::DeserializeOwned>(key: &str) -> Option<T> {
    #[cfg(target_arch = "wasm32")]
    {
        read_from_ffi(key, |key_ptr, key_len| unsafe {
            ironpad_sim_read(key_ptr, key_len)
        })
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        None
    }
}

/// Read all buffered values for `key` (ring buffer, oldest-first).
///
/// Returns an empty `Vec` if no values have been emitted, or if
/// deserialisation fails.  On non-`wasm32` targets this always returns an
/// empty `Vec`.
pub fn read_all<T: serde::de::DeserializeOwned>(key: &str) -> Vec<T> {
    #[cfg(target_arch = "wasm32")]
    {
        read_from_ffi(key, |key_ptr, key_len| unsafe {
            ironpad_sim_read_all(key_ptr, key_len)
        })
        .unwrap_or_default()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = key;
        Vec::new()
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Call an FFI read function, parse the returned length-prefixed JSON buffer,
/// deserialise into `T`, and free the buffer.
///
/// Return protocol: `[4 bytes u32-LE length][N bytes UTF-8 JSON]`.
/// A return value of `0` means no data is available.
#[cfg(target_arch = "wasm32")]
fn read_from_ffi<T, F>(key: &str, ffi_fn: F) -> Option<T>
where
    T: serde::de::DeserializeOwned,
    F: FnOnce(*const u8, u32) -> u32,
{
    let key_bytes = key.as_bytes();
    let ptr = ffi_fn(key_bytes.as_ptr(), key_bytes.len() as u32);

    if ptr == 0 {
        return None;
    }

    // SAFETY: `ptr` is a valid WASM linear-memory address written by the JS
    // executor via `ironpad_alloc`.  We read the 4-byte length prefix first,
    // then the JSON payload, then immediately free the buffer.
    unsafe {
        let len_bytes: [u8; 4] = std::slice::from_raw_parts(ptr as *const u8, 4)
            .try_into()
            .ok()?;
        let json_len = u32::from_le_bytes(len_bytes) as usize;

        let json_slice = std::slice::from_raw_parts((ptr + 4) as *const u8, json_len);
        let result = serde_json::from_slice(json_slice).ok();

        crate::ironpad_dealloc(ptr as *mut u8, 4 + json_len);

        result
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emit_is_noop_on_native() {
        // Should not panic; no assertion needed — the function is a no-op.
        emit("temperature", &42.0_f64);
        emit("label", &"hello");
        emit("nested", &serde_json::json!({"a": 1, "b": [2, 3]}));
    }

    #[test]
    fn read_returns_none_on_native() {
        let val: Option<f64> = read("temperature");
        assert!(val.is_none());

        let val: Option<String> = read("missing_key");
        assert!(val.is_none());
    }

    #[test]
    fn read_all_returns_empty_on_native() {
        let vals: Vec<f64> = read_all("temperature");
        assert!(vals.is_empty());

        let vals: Vec<serde_json::Value> = read_all("anything");
        assert!(vals.is_empty());
    }

    #[test]
    fn emit_serialises_various_types() {
        // These should all succeed without panicking.
        emit("int", &42_i32);
        emit("float", &std::f64::consts::PI);
        emit("bool", &true);
        emit("string", &"value");
        emit("vec", &vec![1_i32, 2, 3]);
    }
}
