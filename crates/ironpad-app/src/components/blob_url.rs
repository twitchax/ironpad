//! Shared blob URL helpers for creating and revoking browser Blob URLs
//! from base64-encoded binary data.

// ── Blob URL helpers (hydrate-only) ──────────────────────────────────────────

/// Decode a base64 string to raw bytes and create a Blob URL via the browser.
///
/// Returns the `blob:` URL string, or `None` if the browser APIs are
/// unavailable (e.g. during SSR).
#[cfg(feature = "hydrate")]
pub fn create_blob_url(base64_data: &str, mime_type: &str) -> Option<String> {
    use js_sys::Function;
    use wasm_bindgen::JsValue;

    // Run the entire base64 → Blob URL pipeline in a single JS call so the
    // decoded bytes never cross the WASM boundary.
    let func = Function::new_with_args(
        "b64,mime",
        "var s=atob(b64);\
         var b=new Uint8Array(s.length);\
         for(var i=0;i<s.length;i++)b[i]=s.charCodeAt(i);\
         return URL.createObjectURL(new Blob([b],{type:mime}))",
    );

    func.call2(
        &JsValue::NULL,
        &JsValue::from_str(base64_data),
        &JsValue::from_str(mime_type),
    )
    .ok()
    .and_then(|v| v.as_string())
}

/// No-op on the server side.
#[cfg(not(feature = "hydrate"))]
pub fn create_blob_url(_base64_data: &str, _mime_type: &str) -> Option<String> {
    None
}

/// Revoke a previously created Blob URL to free browser memory.
#[cfg(feature = "hydrate")]
pub fn revoke_blob_url(url: &str) {
    let _ = web_sys::Url::revoke_object_url(url);
}

/// No-op on the server side.
#[cfg(not(feature = "hydrate"))]
pub fn revoke_blob_url(_url: &str) {}
