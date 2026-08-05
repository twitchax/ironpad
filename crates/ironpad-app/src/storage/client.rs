//! wasm-bindgen bindings for the `IronpadStorage` JavaScript API.

use wasm_bindgen::prelude::*;

use ironpad_common::IronpadNotebook;

// ── Raw JS bindings ─────────────────────────────────────────────────────────

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "listNotebooks")]
    async fn js_list_notebooks() -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "getNotebook")]
    async fn js_get_notebook(id: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "saveNotebook")]
    async fn js_save_notebook(notebook: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "deleteNotebook")]
    async fn js_delete_notebook(id: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "searchNotebooks")]
    async fn js_search_notebooks(query: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "importNotebook")]
    async fn js_import_notebook(json_string: &str) -> Result<JsValue, JsValue>;

}

// ── Typed Rust API ──────────────────────────────────────────────────────────

/// Deserialize a JS array of notebooks element-by-element, skipping (and logging)
/// any record that fails to deserialize.
///
/// This is deliberately resilient: deserializing the whole array as
/// `Vec<IronpadNotebook>` in one shot means a single malformed or old-schema
/// record errors the entire batch and hides *every* notebook. Per-element
/// deserialization drops only the bad record.
fn deserialize_notebook_array(val: &JsValue) -> Vec<IronpadNotebook> {
    if !js_sys::Array::is_array(val) {
        // Not an array (null/undefined/unexpected shape) — treat as empty.
        return Vec::new();
    }
    js_sys::Array::from(val)
        .iter()
        .filter_map(
            |item| match serde_wasm_bindgen::from_value::<IronpadNotebook>(item) {
                Ok(nb) => Some(nb),
                Err(e) => {
                    leptos::logging::warn!("skipping malformed notebook record in IndexedDB: {e}");
                    None
                }
            },
        )
        .collect()
}

/// Lists all private notebooks from `IndexedDB`, sorted by `updated_at` descending.
///
/// Degrades to an empty list (logged) if the underlying `IndexedDB` read fails.
pub async fn list_notebooks() -> Vec<IronpadNotebook> {
    match js_list_notebooks().await {
        Ok(val) => deserialize_notebook_array(&val),
        Err(e) => {
            leptos::logging::warn!("listNotebooks failed: {e:?}");
            Vec::new()
        }
    }
}

/// Retrieves a single notebook by ID, or `None` if not found (or the read failed).
pub async fn get_notebook(id: &str) -> Option<IronpadNotebook> {
    let val = match js_get_notebook(id).await {
        Ok(val) => val,
        Err(e) => {
            leptos::logging::warn!("getNotebook failed: {e:?}");
            return None;
        }
    };
    if val.is_null() || val.is_undefined() {
        return None;
    }
    match serde_wasm_bindgen::from_value(val) {
        Ok(nb) => Some(nb),
        Err(e) => {
            leptos::logging::warn!(
                "skipping malformed notebook record from getNotebook({id}): {e}"
            );
            None
        }
    }
}

/// Saves (upserts) a notebook to `IndexedDB`.
///
/// Returns `Err` if serialization fails or the underlying `IndexedDB` write is
/// rejected (e.g. `QuotaExceededError`), so callers can surface the failure
/// instead of the write silently vanishing.
pub async fn save_notebook(notebook: &IronpadNotebook) -> Result<(), JsValue> {
    let val = serde_wasm_bindgen::to_value(notebook)
        .map_err(|e| JsValue::from_str(&format!("serialize notebook: {e}")))?;
    js_save_notebook(val).await?;
    Ok(())
}

/// Deletes a notebook from `IndexedDB` by ID. Logs (and swallows) any failure.
pub async fn delete_notebook(id: &str) {
    if let Err(e) = js_delete_notebook(id).await {
        leptos::logging::warn!("deleteNotebook failed: {e:?}");
    }
}

/// Searches notebooks by title substring.
///
/// Degrades to an empty list (logged) if the underlying `IndexedDB` read fails.
pub async fn search_notebooks(query: &str) -> Vec<IronpadNotebook> {
    match js_search_notebooks(query).await {
        Ok(val) => deserialize_notebook_array(&val),
        Err(e) => {
            leptos::logging::warn!("searchNotebooks failed: {e:?}");
            Vec::new()
        }
    }
}

/// Imports a notebook from a JSON string. Returns the imported notebook with a new UUID,
/// or `None` if the import failed.
pub async fn import_notebook(json_string: &str) -> Option<IronpadNotebook> {
    match js_import_notebook(json_string).await {
        Ok(val) => serde_wasm_bindgen::from_value(val).ok(),
        Err(e) => {
            leptos::logging::warn!("importNotebook failed: {e:?}");
            None
        }
    }
}
