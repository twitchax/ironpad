//! Local compiled-blob cache (PRD-0047).
//!
//! A content-addressed `IndexedDB` store of compiled cell WASM, keyed by the
//! SAME cache recipe the server uses (`ironpad_common::cache_key`) bound to
//! the server's toolchain fingerprint (fetched once per session and memoized).
//! Cache mode probes here before paying the `compile_cell` round trip; Fresh
//! mode skips the probe and overwrites the entry with the server's fresh
//! result, so flipping back to cache mode serves the fresh blob. A deploy or
//! toolchain bump changes the fingerprint, which changes every key — the
//! stale entries simply stop being found and age out of the LRU.

use std::cell::RefCell;

use wasm_bindgen::prelude::*;

use ironpad_common::{cache_key, CompileRequest, CompileResponse};

use crate::server_fns::get_toolchain_fingerprint;

// ── Raw JS bindings (public/storage.js) ─────────────────────────────────────

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "getBlob")]
    async fn js_get_blob(hash: &str) -> Result<JsValue, JsValue>;

    #[wasm_bindgen(catch, js_namespace = ["window", "IronpadStorage"], js_name = "putBlob")]
    async fn js_put_blob(
        hash: &str,
        wasm: &js_sys::Uint8Array,
        glue: Option<String>,
        diagnostics: Option<String>,
        preamble_lines: u32,
    ) -> Result<JsValue, JsValue>;
}

// ── Fingerprint memo ────────────────────────────────────────────────────────

thread_local! {
    // wasm is single-threaded, so a thread_local is a process-global memo.
    static FINGERPRINT: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The server's toolchain fingerprint, fetched once per session. `None` when
/// the fetch fails (offline first load) — callers then skip local caching for
/// this run rather than hashing with a guessed fingerprint.
async fn toolchain_fingerprint() -> Option<String> {
    if let Some(fp) = FINGERPRINT.with(|c| c.borrow().clone()) {
        return Some(fp);
    }
    match get_toolchain_fingerprint().await {
        Ok(fp) => {
            FINGERPRINT.with(|c| c.borrow_mut().replace(fp.clone()));
            Some(fp)
        }
        Err(e) => {
            leptos::logging::warn!("toolchain fingerprint fetch failed: {e}; local blob cache off");
            None
        }
    }
}

// ── Public API ──────────────────────────────────────────────────────────────

/// Compute the cache key for a compile request with the shared recipe — the
/// same hash the server derives in `compile_cell`, so local entries line up
/// with server cache semantics exactly. `None` when the fingerprint is
/// unavailable (treat as "local cache disabled").
pub async fn request_hash(request: &CompileRequest) -> Option<String> {
    let fingerprint = toolchain_fingerprint().await?;
    let needs_atomics = cache_key::merged_deps_contain_rayon(
        request.shared_cargo_toml.as_deref(),
        &request.cargo_toml,
    );
    let needs_autodiff =
        cache_key::uses_std_autodiff(&request.source, request.shared_source.as_deref());
    let needs_simd = cache_key::uses_wasm_simd(&request.source, request.shared_source.as_deref());
    Some(cache_key::content_hash_with_fingerprint(
        &request.source,
        &request.cargo_toml,
        &request.previous_cell_types,
        request.shared_cargo_toml.as_deref(),
        request.shared_source.as_deref(),
        needs_atomics,
        needs_autodiff,
        needs_simd,
        &fingerprint,
    ))
}

/// Probe the local store; a hit synthesizes a [`CompileResponse`] identical in
/// shape to what the server would have returned for the same key (blob, glue,
/// stored diagnostics with their preamble offset).
pub async fn try_local_hit(hash: &str) -> Option<CompileResponse> {
    let record = js_get_blob(hash).await.ok()?;
    if record.is_null() || record.is_undefined() {
        return None;
    }

    let wasm_val = js_sys::Reflect::get(&record, &"wasm".into()).ok()?;
    if !wasm_val.is_instance_of::<js_sys::Uint8Array>() {
        return None;
    }
    let wasm_blob = js_sys::Uint8Array::from(wasm_val).to_vec();
    if wasm_blob.is_empty() {
        return None;
    }

    let js_glue = js_sys::Reflect::get(&record, &"glue".into())
        .ok()
        .and_then(|v| v.as_string());
    let diagnostics = js_sys::Reflect::get(&record, &"diagnostics".into())
        .ok()
        .and_then(|v| v.as_string())
        .and_then(|json| serde_json::from_str(&json).ok())
        .unwrap_or_default();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let preamble_lines = js_sys::Reflect::get(&record, &"preambleLines".into())
        .ok()
        .and_then(|v| v.as_f64())
        .unwrap_or(0.0) as u32;

    Some(CompileResponse {
        wasm_blob,
        diagnostics,
        cached: true,
        preamble_lines,
        js_glue,
    })
}

/// Store a server compile result under its key (best-effort; failures only
/// cost a future cache miss). Called for every server response — including
/// Fresh-mode compiles, which is exactly what makes Fresh overwrite the local
/// entry. Failed compiles (empty blob) are never cached.
pub async fn store_local(hash: &str, response: &CompileResponse) {
    if response.wasm_blob.is_empty() {
        return;
    }
    let wasm = js_sys::Uint8Array::from(response.wasm_blob.as_slice());
    let diagnostics = if response.diagnostics.is_empty() {
        None
    } else {
        serde_json::to_string(&response.diagnostics).ok()
    };
    if let Err(e) = js_put_blob(
        hash,
        &wasm,
        response.js_glue.clone(),
        diagnostics,
        response.preamble_lines,
    )
    .await
    {
        leptos::logging::warn!("local blob store write failed: {e:?}");
    }
}
