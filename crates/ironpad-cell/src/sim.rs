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

/// Decode a length-prefixed JSON buffer into `T`.
///
/// Wire format: `[4 bytes u32-LE length][length bytes UTF-8 JSON]`.  This is
/// the pure, host-testable core of [`read_from_ffi`]: given the raw buffer it
/// parses the length prefix, bounds-checks the payload, and deserialises —
/// with no raw-pointer or allocation concerns, so the same bounds discipline
/// as [`crate::CellInputs::from_raw`] can be exercised on native targets.
///
/// Returns `None` if the buffer is shorter than the 4-byte header, if the
/// declared length runs past the buffer (or `4 + length` would overflow a
/// `usize`, which can happen on wasm32 where `usize` is 32-bit), or if the
/// JSON fails to deserialise.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
fn decode_length_prefixed<T: serde::de::DeserializeOwned>(buf: &[u8]) -> Option<T> {
    let len_bytes: [u8; 4] = buf.get(0..4)?.try_into().ok()?;
    let json_len = u32::from_le_bytes(len_bytes) as usize;
    // `4 + json_len` can wrap on wasm32 (usize == u32); reject rather than wrap.
    let end = 4usize.checked_add(json_len)?;
    let json = buf.get(4..end)?;
    serde_json::from_slice(json).ok()
}

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
    // `usize` is `u32` on wasm32 (the only target this block compiles for), so the cast is lossless.
    #[allow(clippy::cast_possible_truncation)]
    let ptr = ffi_fn(key_bytes.as_ptr(), key_bytes.len() as u32);

    if ptr == 0 {
        return None;
    }

    // SAFETY: `ptr` is a valid WASM linear-memory address written by the JS
    // executor via `ironpad_alloc`, holding a `[u32-LE length][N bytes JSON]`
    // buffer.  We read the 4-byte length prefix to size the full allocation,
    // run the pure decode over it, then free the exact allocation.  If
    // `4 + length` would wrap `u32` we bail before any wide read or free
    // (leaking the buffer is preferable to unsound pointer arithmetic).
    unsafe {
        let len_bytes: [u8; 4] = std::slice::from_raw_parts(ptr as *const u8, 4)
            .try_into()
            .ok()?;
        let json_len = u32::from_le_bytes(len_bytes) as usize;
        let total_len = 4usize.checked_add(json_len)?;

        let buffer = std::slice::from_raw_parts(ptr as *const u8, total_len);
        let result = decode_length_prefixed(buffer);

        crate::ironpad_dealloc(ptr as *mut u8, total_len);

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

    // ── decode_length_prefixed (pure FFI decode core) ────────────────────────

    /// Frame `json` the way the JS executor does: `[u32-LE len][json bytes]`.
    fn framed(json: &[u8]) -> Vec<u8> {
        let mut buf = u32::try_from(json.len()).unwrap().to_le_bytes().to_vec();
        buf.extend_from_slice(json);
        buf
    }

    #[test]
    fn decode_round_trips_a_valid_buffer() {
        let decoded: Option<serde_json::Value> =
            decode_length_prefixed(&framed(br#"{"a":1,"b":[2,3]}"#));
        assert_eq!(decoded, Some(serde_json::json!({"a": 1, "b": [2, 3]})));

        // A scalar payload round-trips too.
        let decoded: Option<i32> = decode_length_prefixed(&framed(b"42"));
        assert_eq!(decoded, Some(42));
    }

    #[test]
    fn decode_rejects_truncated_header() {
        // Fewer than the 4 header bytes → None, never a panic or OOB read.
        assert!(decode_length_prefixed::<i32>(&[]).is_none());
        assert!(decode_length_prefixed::<i32>(&[1, 2, 3]).is_none());
    }

    #[test]
    fn decode_rejects_length_past_buffer() {
        // Header claims 100 payload bytes but only 3 follow.
        let mut buf = 100u32.to_le_bytes().to_vec();
        buf.extend_from_slice(b"abc");
        let decoded: Option<serde_json::Value> = decode_length_prefixed(&buf);
        assert!(decoded.is_none());
    }

    #[test]
    fn decode_rejects_near_u32_max_length_without_wrapping() {
        // A length prefix at u32::MAX: `4 + len` would wrap on wasm32
        // (usize == u32).  Must return None, never panic or read OOB.
        let mut buf = u32::MAX.to_le_bytes().to_vec();
        buf.extend_from_slice(b"whatever");
        let decoded: Option<serde_json::Value> = decode_length_prefixed(&buf);
        assert!(decoded.is_none());
    }
}
