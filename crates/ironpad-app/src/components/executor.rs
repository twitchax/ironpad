// ── WASM executor bindings ──────────────────────────────────────────────────
//
// The JS-side executor (`public/executor.js`) manages WASM module loading,
// caching, and execution.  These bindings provide a type-safe Rust API over
// the JS bridge for use from Leptos components.

// ── JS interop (client-side only) ───────────────────────────────────────────

#[cfg(feature = "hydrate")]
mod js {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        /// Load a compiled WASM blob for a cell.  Returns a `Promise<void>`.
        #[wasm_bindgen(js_namespace = IronpadExecutor, js_name = "loadBlob", catch)]
        pub fn load_blob(
            cell_id: &str,
            hash: &str,
            wasm_bytes: &js_sys::Uint8Array,
        ) -> Result<js_sys::Promise, JsValue>;

        /// Execute a loaded cell with input bytes.
        /// Returns `{ outputBytes: Uint8Array, displayText: string | null }`.
        #[wasm_bindgen(js_namespace = IronpadExecutor, catch)]
        pub fn execute(cell_id: &str, input_bytes: &js_sys::Uint8Array)
            -> Result<JsValue, JsValue>;

        /// Remove a loaded cell module, freeing browser-side resources.
        #[wasm_bindgen(js_namespace = IronpadExecutor)]
        pub fn unload(cell_id: &str);

        /// Check whether a cell has a module loaded with the given hash.
        #[wasm_bindgen(js_namespace = IronpadExecutor, js_name = "isLoaded")]
        pub fn is_loaded(cell_id: &str, hash: &str) -> bool;
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Compute a lightweight hash of a WASM blob for executor caching.
///
/// Uses FNV-1a (64-bit) to avoid pulling in a heavy hashing dependency on the
/// WASM client side.  The hash is only used to detect same-blob cache hits.
#[cfg(feature = "hydrate")]
pub fn hash_wasm_blob(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

/// Verify that the executor JS module is available on `window`.
///
/// The executor auto-initialises when `executor.js` loads, so this is
/// primarily a diagnostic check.  Returns `Err` if the global is missing.
#[cfg(feature = "hydrate")]
pub fn init_executor() -> Result<(), String> {
    let window = web_sys::window().ok_or("no window object")?;
    let val =
        js_sys::Reflect::get(&window, &"IronpadExecutor".into()).map_err(|e| format!("{e:?}"))?;

    if val.is_undefined() || val.is_null() {
        return Err("IronpadExecutor not found on window".into());
    }

    Ok(())
}

/// Load a compiled WASM blob into the executor's cache.
///
/// If a blob with the same `hash` is already loaded for the cell, this is a
/// no-op (cache hit).  The function is async because `WebAssembly.instantiate`
/// is async on the browser.
#[cfg(feature = "hydrate")]
pub async fn load_blob(cell_id: &str, hash: &str, bytes: &[u8]) -> Result<(), String> {
    let uint8 = js_sys::Uint8Array::from(bytes);
    let promise = js::load_blob(cell_id, hash, &uint8).map_err(|e| format!("{e:?}"))?;

    wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| format!("{e:?}"))?;

    Ok(())
}

/// Execute a previously-loaded cell with the given input bytes.
///
/// Returns `(output_bytes, display_text)`.  The cell must have been loaded via
/// [`load_blob`] first; otherwise the executor throws.
#[cfg(feature = "hydrate")]
pub fn execute_cell(
    cell_id: &str,
    input_bytes: &[u8],
) -> Result<(Vec<u8>, Option<String>), String> {
    let input = js_sys::Uint8Array::from(input_bytes);
    let result = js::execute(cell_id, &input).map_err(|e| format!("{e:?}"))?;

    // Extract `outputBytes` (Uint8Array) from the result object.
    let output_val =
        js_sys::Reflect::get(&result, &"outputBytes".into()).map_err(|e| format!("{e:?}"))?;

    let output_bytes = if wasm_bindgen::JsCast::is_instance_of::<js_sys::Uint8Array>(&output_val) {
        js_sys::Uint8Array::from(output_val).to_vec()
    } else {
        vec![]
    };

    // Extract `displayText` (string | null).
    let display_val =
        js_sys::Reflect::get(&result, &"displayText".into()).map_err(|e| format!("{e:?}"))?;

    let display_text = if display_val.is_null() || display_val.is_undefined() {
        None
    } else {
        display_val.as_string()
    };

    Ok((output_bytes, display_text))
}
