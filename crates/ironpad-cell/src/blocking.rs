//! Blocking host calls via WebAssembly JSPI — synchronous cell code that
//! suspends the whole WASM stack while the browser awaits a JS promise.
//!
//! Unlike [`crate::http`] (which requires an async cell — any `.await` in the
//! cell body), these functions are plain synchronous Rust: callable from trait
//! impls, iterator adapters, or any helper function, with no function coloring.
//! Under the hood the executor wraps the `ironpad_blocking_*` imports in
//! `WebAssembly.Suspending` and enters `cell_main` via `WebAssembly.promising`
//! (PRD-0043), so a call here parks the entire WASM stack until the promise
//! settles.
//!
//! **Browser support**: requires JSPI, shipped by default in Chrome/Edge 137+.
//! On other browsers the cell fails with a clear error before execution.
//!
//! **Sync cells only**: cells containing `.await` compile to an async
//! `cell_main` wrapper that the executor cannot enter via
//! `WebAssembly.promising`; do not mix `.await` with `blocking::*` calls.
//!
//! On non-`wasm32` targets `sleep_ms` blocks the thread and the fetch
//! functions return `Err` — the JSPI bridge only exists inside the browser
//! executor.

// ── FFI imports (wasm32 only) ────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
extern "C" {
    /// Suspend until `ms` milliseconds have elapsed (JS `setTimeout`).
    fn ironpad_blocking_sleep_ms(ms: f64);

    /// Fetch `url`, suspending until the response settles. Stashes the payload
    /// (response body, or an error message) host-side and returns its byte
    /// length. Retrieve with [`ironpad_blocking_fetch_ok`] +
    /// [`ironpad_blocking_read`]. Two-phase by design: the host never calls
    /// back into a suspended instance.
    fn ironpad_blocking_fetch(url_ptr: *const u8, url_len: u32) -> u32;

    /// Returns `1` if the last [`ironpad_blocking_fetch`] on this cell
    /// succeeded (HTTP 2xx), `0` otherwise (synchronous, no suspension).
    fn ironpad_blocking_fetch_ok() -> u32;

    /// Copy the stashed payload into a cell-allocated buffer, returning the
    /// number of bytes copied (synchronous, no suspension — the same shape as
    /// `ironpad_sim_read`).
    fn ironpad_blocking_read(buf_ptr: *mut u8, buf_len: u32) -> u32;
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Block for `ms` milliseconds.
///
/// On `wasm32` this suspends the WASM stack via JSPI (the browser stays fully
/// responsive); on native targets it parks the thread with
/// [`std::thread::sleep`].
pub fn sleep_ms(ms: f64) {
    #[cfg(target_arch = "wasm32")]
    unsafe {
        ironpad_blocking_sleep_ms(ms);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::sleep(std::time::Duration::from_secs_f64(ms.max(0.0) / 1000.0));
    }
}

/// Perform a blocking GET request, returning the raw response body.
///
/// # Errors
///
/// Returns `Err` with the transport error or `HTTP <status>` text on a
/// non-2xx response; always `Err` on non-`wasm32` targets.
pub fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    #[cfg(target_arch = "wasm32")]
    {
        // Fetch suspends and stashes the payload host-side; `read` then copies
        // it into a buffer this cell owns. Truncation can't happen — the host
        // reported the exact length — but `copied` is honored defensively.
        #[allow(clippy::cast_possible_truncation)]
        let url_len = url.len() as u32;
        let len = unsafe { ironpad_blocking_fetch(url.as_ptr(), url_len) };
        let ok = unsafe { ironpad_blocking_fetch_ok() } == 1;
        let mut buf = vec![0u8; len as usize];
        let copied = unsafe { ironpad_blocking_read(buf.as_mut_ptr(), len) };
        buf.truncate(copied as usize);
        if ok {
            Ok(buf)
        } else {
            Err(String::from_utf8_lossy(&buf).into_owned())
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = url;
        Err("blocking fetch is only available in the browser executor (JSPI)".to_string())
    }
}

/// Perform a blocking GET request, returning the response body as text.
///
/// # Errors
///
/// Same failure modes as [`fetch_bytes`], plus invalid UTF-8 in the body.
pub fn fetch_text(url: &str) -> Result<String, String> {
    let bytes = fetch_bytes(url)?;
    String::from_utf8(bytes).map_err(|e| format!("response body is not valid UTF-8: {e}"))
}

/// Perform a blocking GET request and deserialize the JSON response.
///
/// # Errors
///
/// Same failure modes as [`fetch_text`], plus JSON deserialization errors.
pub fn fetch_json<T: serde::de::DeserializeOwned>(url: &str) -> Result<T, String> {
    let text = fetch_text(url)?;
    serde_json::from_str(&text).map_err(|e| format!("failed to deserialize JSON: {e}"))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_ms_blocks_on_native() {
        let start = std::time::Instant::now();
        sleep_ms(15.0);
        assert!(start.elapsed() >= std::time::Duration::from_millis(14));
    }

    #[test]
    fn sleep_ms_tolerates_negative_durations() {
        sleep_ms(-5.0); // must not panic (Duration::from_secs_f64 would)
    }

    #[test]
    fn fetch_is_unavailable_on_native() {
        let err = fetch_bytes("https://example.com").unwrap_err();
        assert!(
            err.contains("JSPI"),
            "error should name the requirement: {err}"
        );
        assert!(fetch_text("https://example.com").is_err());
        assert!(fetch_json::<serde_json::Value>("https://example.com").is_err());
    }
}
