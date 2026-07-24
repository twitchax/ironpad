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

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "exportNotebook")]
    async fn js_export_notebook(id: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "importNotebook")]
    async fn js_import_notebook(json_string: &str) -> Result<JsValue, JsValue>;

    // ── Mutable shares + user key (PRD-0049) ────────────────────────────────

    #[wasm_bindgen(js_namespace = ["window", "IronpadStorage"], js_name = "mintKey")]
    fn js_mint_key() -> String;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "getUserKey")]
    async fn js_get_user_key() -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "setUserKey")]
    async fn js_set_user_key(value: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "convertToMutable")]
    async fn js_convert_to_mutable(
        id: &str,
        share_id: &str,
        edit_key: &str,
    ) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "convertToPrivate")]
    async fn js_convert_to_private(id: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "saveMutable")]
    async fn js_save_mutable(record: JsValue) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "getMutable")]
    async fn js_get_mutable(id: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "listMutable")]
    async fn js_list_mutable() -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "deleteMutable")]
    async fn js_delete_mutable(id: &str) -> Result<JsValue, JsValue>;
}

/// A mutable-share working copy stored locally (PRD-0049): the notebook plus
/// its server share id and the key that authorizes pushes on this device.
#[derive(Clone, Debug, serde::Deserialize)]
pub struct MutableLocalRecord {
    pub id: String,
    pub share_id: String,
    pub edit_key: String,
    pub notebook: IronpadNotebook,
}

/// Serialized shape written to the `mutable` object store.
#[derive(serde::Serialize)]
struct MutableLocalRecordOut<'a> {
    id: String,
    share_id: &'a str,
    edit_key: &'a str,
    notebook: &'a IronpadNotebook,
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

/// Exports a notebook as a JSON string, or `None` if not found (or the read failed).
pub async fn export_notebook(id: &str) -> Option<String> {
    match js_export_notebook(id).await {
        Ok(val) => val.as_string(),
        Err(e) => {
            leptos::logging::warn!("exportNotebook failed: {e:?}");
            None
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

// ── Mutable shares + user key (PRD-0049) ────────────────────────────────────

/// Mint a fresh random 64-hex key (a new share's per-notebook key).
pub fn mint_key() -> String {
    js_mint_key()
}

/// This device's user key, generating and persisting one on first use.
/// Degrades to an empty string (logged) if `IndexedDB` is unavailable.
pub async fn get_user_key() -> String {
    match js_get_user_key().await {
        Ok(val) => val.as_string().unwrap_or_default(),
        Err(e) => {
            leptos::logging::warn!("getUserKey failed: {e:?}");
            String::new()
        }
    }
}

/// Overwrite this device's user key (local clobber; does not revoke the old
/// one server-side).
pub async fn set_user_key(value: &str) {
    if let Err(e) = js_set_user_key(value).await {
        leptos::logging::warn!("setUserKey failed: {e:?}");
    }
}

/// Convert a private notebook into a mutable-share working copy (move it into
/// the mutable store under `share_id`/`edit_key`).
pub async fn convert_to_mutable(id: &str, share_id: &str, edit_key: &str) -> Result<(), JsValue> {
    js_convert_to_mutable(id, share_id, edit_key).await?;
    Ok(())
}

/// Move a mutable-backed notebook back to the private store (unpublish).
pub async fn convert_to_private(id: &str) {
    if let Err(e) = js_convert_to_private(id).await {
        leptos::logging::warn!("convertToPrivate failed: {e:?}");
    }
}

/// Insert or replace a local mutable-share record (rebind onto this device).
pub async fn save_mutable(
    notebook: &IronpadNotebook,
    share_id: &str,
    edit_key: &str,
) -> Result<(), JsValue> {
    let record = MutableLocalRecordOut {
        id: notebook.id.to_string(),
        share_id,
        edit_key,
        notebook,
    };
    let val = serde_wasm_bindgen::to_value(&record)
        .map_err(|e| JsValue::from_str(&format!("serialize mutable record: {e}")))?;
    js_save_mutable(val).await?;
    Ok(())
}

/// Get a local mutable-share record by uuid, or `None` if the notebook is not
/// mutable-backed on this device.
pub async fn get_mutable(id: &str) -> Option<MutableLocalRecord> {
    let val = match js_get_mutable(id).await {
        Ok(val) => val,
        Err(e) => {
            leptos::logging::warn!("getMutable failed: {e:?}");
            return None;
        }
    };
    if val.is_null() || val.is_undefined() {
        return None;
    }
    match serde_wasm_bindgen::from_value(val) {
        Ok(record) => Some(record),
        Err(e) => {
            leptos::logging::warn!("skipping malformed mutable record from getMutable({id}): {e}");
            None
        }
    }
}

/// List all mutable-share records bound on this device.
pub async fn list_mutable() -> Vec<MutableLocalRecord> {
    let val = match js_list_mutable().await {
        Ok(val) => val,
        Err(e) => {
            leptos::logging::warn!("listMutable failed: {e:?}");
            return Vec::new();
        }
    };
    if !js_sys::Array::is_array(&val) {
        return Vec::new();
    }
    js_sys::Array::from(&val)
        .iter()
        .filter_map(
            |item| match serde_wasm_bindgen::from_value::<MutableLocalRecord>(item) {
                Ok(record) => Some(record),
                Err(e) => {
                    leptos::logging::warn!("skipping malformed mutable record: {e}");
                    None
                }
            },
        )
        .collect()
}

/// Delete a local mutable-share working copy by uuid (does not touch the server).
pub async fn delete_mutable(id: &str) {
    if let Err(e) = js_delete_mutable(id).await {
        leptos::logging::warn!("deleteMutable failed: {e:?}");
    }
}
