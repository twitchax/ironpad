// ── WASM executor bindings ──────────────────────────────────────────────────
//
// The JS-side executor bridge (`public/executor-bridge.js`) delegates WASM
// module loading and execution to a Web Worker.  These bindings provide a
// type-safe Rust API over the bridge for use from Leptos components.

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
            js_glue: Option<String>,
        ) -> Result<js_sys::Promise, JsValue>;

        /// Execute a loaded cell with input bytes.  Returns a
        /// `Promise<{ outputBytes, displayText, typeTag }>`.
        ///
        /// Always async: wasm-bindgen cells may have async `cell_main`.
        #[wasm_bindgen(js_namespace = IronpadExecutor, catch)]
        pub fn execute(
            cell_id: &str,
            input_bytes: &js_sys::Uint8Array,
        ) -> Result<js_sys::Promise, JsValue>;

        /// Remove a loaded cell module, freeing browser-side resources.
        #[wasm_bindgen(js_namespace = IronpadExecutor)]
        pub fn unload(cell_id: &str);

        /// Check whether a cell has a module loaded with the given hash.
        #[wasm_bindgen(js_namespace = IronpadExecutor, js_name = "isLoaded")]
        pub fn is_loaded(cell_id: &str, hash: &str) -> bool;

        /// Terminate the running Web Worker, aborting any in-flight execution.
        /// A fresh Worker is automatically respawned by the bridge.
        #[wasm_bindgen(js_namespace = IronpadExecutor)]
        pub fn terminate();
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Compute a lightweight hash of a WASM blob for executor caching.
///
/// Uses FNV-1a (64-bit) to avoid pulling in a heavy hashing dependency on the
/// WASM client side.  The hash is only used to detect same-blob cache hits.
#[cfg(feature = "hydrate")]
pub fn hash_wasm_blob(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Verify that the executor JS module is available on `window`.
///
/// The executor auto-initialises when `executor-bridge.js` loads, so this is
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

/// Terminate the executor's Web Worker, cancelling any running cell.
///
/// The bridge rejects pending Promises with `AbortError` and respawns a
/// fresh Worker.  Previously-loaded blobs must be re-loaded.
#[cfg(feature = "hydrate")]
pub fn terminate_executor() {
    js::terminate();
}

/// Load a compiled WASM blob into the executor's cache.
///
/// If a blob with the same `hash` is already loaded for the cell, this is a
/// no-op (cache hit).  The function is async because `WebAssembly.instantiate`
/// is async on the browser.
#[cfg(feature = "hydrate")]
pub async fn load_blob(
    cell_id: &str,
    hash: &str,
    bytes: &[u8],
    js_glue: Option<String>,
) -> Result<(), String> {
    let uint8 = js_sys::Uint8Array::from(bytes);
    let promise = js::load_blob(cell_id, hash, &uint8, js_glue).map_err(|e| format!("{e:?}"))?;

    wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| format!("{e:?}"))?;

    Ok(())
}

/// Execution result from running a cell: (`output_bytes`, `display_text`, `type_tag`, `ran_on_main_thread`).
#[cfg(feature = "hydrate")]
pub type CellExecResult = (Vec<u8>, Option<String>, Option<String>, bool);

/// Execute a previously-loaded cell with the given input bytes.
///
/// Returns `(output_bytes, display_text, type_tag)`.  The cell must have been
/// loaded via [`load_blob`] first; otherwise the executor throws.
///
/// Async because the JS executor always returns a Promise (wasm-bindgen cells
/// may have an async `cell_main`).
#[cfg(feature = "hydrate")]
pub async fn execute_cell(cell_id: &str, input_bytes: &[u8]) -> Result<CellExecResult, String> {
    let input = js_sys::Uint8Array::from(input_bytes);
    let promise = js::execute(cell_id, &input).map_err(|e| format!("{e:?}"))?;

    let result = wasm_bindgen_futures::JsFuture::from(promise)
        .await
        .map_err(|e| format!("{e:?}"))?;

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

    // Extract `typeTag` (string | null).
    let type_tag_val =
        js_sys::Reflect::get(&result, &"typeTag".into()).map_err(|e| format!("{e:?}"))?;

    let type_tag = if type_tag_val.is_null() || type_tag_val.is_undefined() {
        None
    } else {
        type_tag_val.as_string()
    };

    // Extract `fallback` (bool) — true when execution fell back to the main thread.
    let fallback_val =
        js_sys::Reflect::get(&result, &"fallback".into()).map_err(|e| format!("{e:?}"))?;

    let ran_on_main_thread = fallback_val.as_bool().unwrap_or(false);

    Ok((output_bytes, display_text, type_tag, ran_on_main_thread))
}
