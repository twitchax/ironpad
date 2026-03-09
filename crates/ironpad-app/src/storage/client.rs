//! wasm-bindgen bindings for the IronpadStorage JavaScript API.

use wasm_bindgen::prelude::*;

use ironpad_common::IronpadNotebook;

// ── Raw JS bindings ─────────────────────────────────────────────────────────

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = ["window", "IronpadStorage"], js_name = "listNotebooks")]
    async fn js_list_notebooks() -> JsValue;

    #[wasm_bindgen(js_namespace = ["window", "IronpadStorage"], js_name = "getNotebook")]
    async fn js_get_notebook(id: &str) -> JsValue;

    #[wasm_bindgen(js_namespace = ["window", "IronpadStorage"], js_name = "saveNotebook")]
    async fn js_save_notebook(notebook: JsValue);

    #[wasm_bindgen(js_namespace = ["window", "IronpadStorage"], js_name = "deleteNotebook")]
    async fn js_delete_notebook(id: &str);

    #[wasm_bindgen(js_namespace = ["window", "IronpadStorage"], js_name = "searchNotebooks")]
    async fn js_search_notebooks(query: &str) -> JsValue;

    #[wasm_bindgen(js_namespace = ["window", "IronpadStorage"], js_name = "exportNotebook")]
    async fn js_export_notebook(id: &str) -> JsValue;

    #[wasm_bindgen(js_namespace = ["window", "IronpadStorage"], js_name = "importNotebook")]
    async fn js_import_notebook(json_string: &str) -> JsValue;
}

// ── Typed Rust API ──────────────────────────────────────────────────────────

/// Lists all private notebooks from IndexedDB, sorted by updated_at descending.
pub async fn list_notebooks() -> Vec<IronpadNotebook> {
    let val = js_list_notebooks().await;
    serde_wasm_bindgen::from_value(val).unwrap_or_default()
}

/// Retrieves a single notebook by ID, or `None` if not found.
pub async fn get_notebook(id: &str) -> Option<IronpadNotebook> {
    let val = js_get_notebook(id).await;
    if val.is_null() || val.is_undefined() {
        return None;
    }
    serde_wasm_bindgen::from_value(val).ok()
}

/// Saves (upserts) a notebook to IndexedDB.
pub async fn save_notebook(notebook: &IronpadNotebook) {
    let val = serde_wasm_bindgen::to_value(notebook).expect("failed to serialize notebook");
    js_save_notebook(val).await;
}

/// Deletes a notebook from IndexedDB by ID.
pub async fn delete_notebook(id: &str) {
    js_delete_notebook(id).await;
}

/// Searches notebooks by title substring.
pub async fn search_notebooks(query: &str) -> Vec<IronpadNotebook> {
    let val = js_search_notebooks(query).await;
    serde_wasm_bindgen::from_value(val).unwrap_or_default()
}

/// Exports a notebook as a JSON string, or `None` if not found.
pub async fn export_notebook(id: &str) -> Option<String> {
    let val = js_export_notebook(id).await;
    val.as_string()
}

/// Imports a notebook from a JSON string. Returns the imported notebook with a new UUID.
pub async fn import_notebook(json_string: &str) -> Option<IronpadNotebook> {
    let val = js_import_notebook(json_string).await;
    serde_wasm_bindgen::from_value(val).ok()
}
