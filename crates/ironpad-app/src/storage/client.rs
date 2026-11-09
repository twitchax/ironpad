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

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "listHistory")]
    async fn js_list_history(id: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "getHistorySnapshot")]
    async fn js_get_history_snapshot(id: &str, saved_at: f64) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "snapshotNow")]
    async fn js_snapshot_now(id: &str) -> Result<JsValue, JsValue>;

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

/// Retrieves a single notebook by ID: `Ok(None)` when this browser holds no
/// record with that id, and `Err` when the read failed or the record does not
/// parse as a notebook.
///
/// The three are kept apart because callers act differently on them: the
/// editor tells "not in this browser" from "damaged", and a move to the
/// account must not read a failed read as a successful delete.
pub async fn get_notebook(id: &str) -> Result<Option<IronpadNotebook>, String> {
    let val = match js_get_notebook(id).await {
        Ok(val) => val,
        Err(e) => {
            leptos::logging::warn!("getNotebook failed: {e:?}");
            return Err(format!("the browser store could not be read: {e:?}"));
        }
    };
    if val.is_null() || val.is_undefined() {
        return Ok(None);
    }
    match serde_wasm_bindgen::from_value(val) {
        Ok(nb) => Ok(Some(nb)),
        Err(e) => {
            leptos::logging::warn!(
                "skipping malformed notebook record from getNotebook({id}): {e}"
            );
            Err(format!("the stored record is not a valid notebook: {e}"))
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

/// One row of a notebook's version history (PRD-0058), newest first.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct HistoryEntry {
    /// Snapshot key (epoch ms). `f64` because it crosses the JS boundary.
    #[serde(rename = "savedAt")]
    pub saved_at: f64,
    pub title: String,
    #[serde(rename = "cellCount")]
    pub cell_count: u32,
}

/// Lists a notebook's history snapshots, newest first. Degrades to empty.
pub async fn list_history(id: &str) -> Vec<HistoryEntry> {
    match js_list_history(id).await {
        Ok(val) => serde_wasm_bindgen::from_value(val).unwrap_or_default(),
        Err(e) => {
            leptos::logging::warn!("listHistory failed: {e:?}");
            Vec::new()
        }
    }
}

/// Fetches one history snapshot's JSON, or `None`.
pub async fn get_history_snapshot(id: &str, saved_at: f64) -> Option<String> {
    match js_get_history_snapshot(id, saved_at).await {
        Ok(val) => val.as_string(),
        Err(e) => {
            leptos::logging::warn!("getHistorySnapshot failed: {e:?}");
            None
        }
    }
}

/// Force-snapshots the current stored record (the undoable-restore half of
/// PRD-0058). Logs and swallows failures — a missing pre-restore snapshot
/// must not block the restore the user asked for.
pub async fn snapshot_now(id: &str) {
    if let Err(e) = js_snapshot_now(id).await {
        leptos::logging::warn!("snapshotNow failed: {e:?}");
    }
}
