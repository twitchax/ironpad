//! The shared cell-run engine: blob acquisition through execution.
//!
//! Extracted from the editor's `pipeline.rs` and the viewer's
//! `ViewOnlyCodeCell` (PRD-0059): the two surfaces had grown divergent
//! copies of the same async pipeline — the viewer retried compile transport
//! errors and the editor didn't. This module owns the one policy:
//!
//! 1. share snapshot (viewers of `/shared` with a blob manifest, PRD-0047)
//! 2. local blob probe (`blob_cache::probe_local`, both surfaces)
//! 3. server compile with transport retries (idempotent, cache-keyed)
//!
//! …then `load_and_execute` for the WASM half. Surfaces keep their own
//! signals, status models, session events, and (until PRD-0060) failure
//! policies; the engine is the part that must never fork.

#[cfg(feature = "hydrate")]
use ironpad_common::{CompileRequest, CompileResponse};
use leptos::prelude::*;

// ── Transport retry policy ──────────────────────────────────────────────────

/// Compile transport retries beyond the first attempt. Compile requests are
/// idempotent (cache-keyed server-side), so TRANSPORT failures — a dropped
/// connection mid-build, a network change on a mobile client — are safe to
/// retry; the server's incremental target dir makes retried builds much
/// cheaper than the first attempt. Compile diagnostics are Ok responses and
/// never retried.
#[cfg(feature = "hydrate")]
const COMPILE_TRANSPORT_RETRIES: i32 = 2;

/// Base backoff (ms) between transport retries, scaled by attempt number.
#[cfg(feature = "hydrate")]
const COMPILE_TRANSPORT_BACKOFF_MS: i32 = 1_500;

/// Await `ms` milliseconds via a `setTimeout`-backed promise, for spacing
/// compile-transport retries.
#[cfg(feature = "hydrate")]
async fn transport_backoff(ms: i32) {
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        if let Some(window) = leptos::web_sys::window() {
            let _ = window.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
        } else {
            let _ = resolve.call0(&wasm_bindgen::JsValue::NULL);
        }
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

// ── Blob acquisition ────────────────────────────────────────────────────────

/// The outcome of [`acquire_blob`]: the compile result plus what the
/// store-back policy needs (`store_unless_served` stays a caller call so the
/// editor's store-before-load ordering is preserved).
#[cfg(feature = "hydrate")]
pub struct BlobAcquisition {
    pub result: Result<CompileResponse, ServerFnError>,
    /// Served by the share snapshot or the local store — the store-back
    /// half skips these (nothing new to persist).
    pub served_without_server: bool,
    /// Content hash for the local store, when the fingerprint was available.
    pub request_hash: Option<String>,
}

/// Acquire a compiled blob for `request` through every cache layer in order:
/// share snapshot (when the caller has a manifest entry and the request is
/// not forced), local `IndexedDB` store, then the server — with transport
/// retries. Fresh mode (`request.force`) bypasses snapshot and local store
/// (the probe self-gates) so the server really recompiles.
#[cfg(feature = "hydrate")]
pub async fn acquire_blob(
    request: &CompileRequest,
    share_blob: Option<&ironpad_common::ShareBlobEntry>,
) -> BlobAcquisition {
    // Snapshotted share blob (PRD-0047): in cache mode, a manifest entry
    // replaces the compile round trip with an immutable static GET. Any
    // fetch failure falls back to the live pipeline below.
    let mut snapshot_response: Option<CompileResponse> = None;
    if !request.force {
        if let Some(entry) = share_blob {
            match crate::components::executor::fetch_share_blob(entry).await {
                Ok((wasm_blob, js_glue)) => {
                    snapshot_response = Some(CompileResponse {
                        wasm_blob,
                        diagnostics: vec![],
                        cached: true,
                        preamble_lines: 0,
                        js_glue,
                    });
                }
                Err(e) => {
                    leptos::logging::warn!(
                        "share blob fetch failed ({e}); falling back to compile"
                    );
                }
            }
        }
    }

    // Local blob store (PRD-0047): skipped entirely when the share snapshot
    // already served the blob, which keeps a snapshot replay free of even
    // the fingerprint round trip.
    let request_hash = if snapshot_response.is_some() {
        None
    } else {
        let (hash, local_hit) = crate::blob_cache::probe_local(request).await;
        snapshot_response = local_hit;
        hash
    };
    let served_without_server = snapshot_response.is_some();

    let result = if let Some(response) = snapshot_response {
        Ok(response)
    } else {
        let mut attempt: i32 = 0;
        loop {
            match crate::server_fns::compile_cell(request.clone()).await {
                Err(e) if attempt < COMPILE_TRANSPORT_RETRIES => {
                    attempt += 1;
                    leptos::logging::warn!(
                        "compile transport error (attempt {attempt}): {e}; retrying"
                    );
                    transport_backoff(COMPILE_TRANSPORT_BACKOFF_MS * attempt).await;
                }
                other => break other,
            }
        }
    };

    BlobAcquisition {
        result,
        served_without_server,
        request_hash,
    }
}

// ── Load + execute ──────────────────────────────────────────────────────────

/// Load the compiled blob into the executor and run it with the assembled
/// piped inputs. Returns the execution tuple plus wall-clock execution time,
/// or the pre-formatted error message both surfaces render (`WASM load
/// error: …` / `Execution error: …` — the latter carries `AbortError` when
/// the user cancelled via `terminate()`).
#[cfg(feature = "hydrate")]
pub async fn load_and_execute(
    cell_id: &str,
    response: &CompileResponse,
    input_bytes: &[u8],
) -> Result<(crate::components::executor::CellExecResult, f64), String> {
    use crate::components::executor;

    let hash = executor::hash_wasm_blob(&response.wasm_blob);
    executor::load_blob(
        cell_id,
        &hash,
        &response.wasm_blob,
        response.js_glue.clone(),
    )
    .await
    .map_err(|e| format!("WASM load error: {e}"))?;

    let exec_start = js_sys::Date::now();
    let result = executor::execute_cell(cell_id, input_bytes)
        .await
        .map_err(|e| format!("Execution error: {e}"))?;
    Ok((result, js_sys::Date::now() - exec_start))
}

// ── Queue bookkeeping ───────────────────────────────────────────────────────

/// Advance a run queue past `cell_id` if it is at the front — the
/// pop-front-if-me every surface used to hand-roll. A no-op when another
/// cell holds the front slot (a stale completion must not eat its turn).
pub fn advance_queue(queue: RwSignal<Vec<String>>, cell_id: &str) {
    queue.update(|q| {
        if q.first().is_some_and(|id| id == cell_id) {
            q.remove(0);
        }
    });
}
