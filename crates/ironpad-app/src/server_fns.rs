use ironpad_common::{CompileRequest, CompileResponse, IronpadNotebook, PublicNotebookSummary};
use leptos::prelude::*;

// ── Compilation ──────────────────────────────────────────────────────────────

/// Compile a single cell's Rust source into a WASM blob.
///
/// Ties together the full compilation pipeline: cache check → scaffold →
/// cargo build → diagnostic parsing → wasm-opt → cache store.
#[server]
pub async fn compile_cell(request: CompileRequest) -> Result<CompileResponse, ServerFnError> {
    use ironpad_common::AppConfig;

    use crate::compiler::{
        build::{build_micro_crate, BuildResult},
        cache::{content_hash, store_blob, try_cache_hit},
        diagnostics::parse_diagnostics,
        optimize::optimize_wasm,
        scaffold::scaffold_micro_crate,
    };

    let config = expect_context::<AppConfig>();
    let session_id = "default";

    let hash = content_hash(
        &request.source,
        &request.cargo_toml,
        &request.previous_cell_types,
        request.shared_cargo_toml.as_deref(),
        request.shared_source.as_deref(),
    );
    tracing::info!(cell_id = %request.cell_id, hash = %hash, "compile_cell started");

    // Cache check (skipped when force-recompile is requested).

    if !request.force {
        if let Some(cache_hit) = try_cache_hit(&config.cache_dir, &hash) {
            tracing::info!(cell_id = %request.cell_id, blob_size = cache_hit.wasm_bytes.len(), "cache hit");
            return Ok(CompileResponse {
                wasm_blob: cache_hit.wasm_bytes,
                diagnostics: vec![],
                cached: true,
                preamble_lines: 0,
                js_glue: cache_hit.js_glue,
            });
        }
    }

    if request.force {
        tracing::info!(cell_id = %request.cell_id, "force recompile requested — skipping cache");
    } else {
        tracing::info!(cell_id = %request.cell_id, "cache miss — compiling");
    }

    // Scaffold micro-crate.

    let (crate_dir, preamble_lines, _is_async) = scaffold_micro_crate(
        &config.cache_dir,
        &config.ironpad_cell_path,
        session_id,
        &request.cell_id,
        &request.source,
        &request.cargo_toml,
        &request.previous_cell_types,
        request.shared_cargo_toml.as_deref(),
        request.shared_source.as_deref(),
    )
    .map_err(|e| ServerFnError::new(format!("scaffold failed: {e}")))?;

    // Build.

    let build_result =
        build_micro_crate(&crate_dir, &config.cache_dir, session_id, &request.cell_id)
            .await
            .map_err(|e| ServerFnError::new(format!("build invocation failed: {e}")))?;

    match build_result {
        BuildResult::Success {
            wasm_path,
            stdout,
            stderr: _,
            js_glue,
        } => {
            let diagnostics = parse_diagnostics(&stdout, preamble_lines);

            let wasm_bytes = tokio::fs::read(&wasm_path)
                .await
                .map_err(|e| ServerFnError::new(format!("failed to read wasm blob: {e}")))?;

            // Best-effort optimization (runs on the wasm-bindgen _bg.wasm).

            let wasm_blob =
                optimize_wasm(&wasm_bytes, crate_dir.parent().unwrap_or(&crate_dir)).await;

            // Cache the result (WASM blob + JS glue).

            if let Err(e) = store_blob(&config.cache_dir, &hash, &wasm_blob, Some(&js_glue)) {
                tracing::warn!(error = %e, "failed to cache compiled blob");
            }

            tracing::info!(
                cell_id = %request.cell_id,
                blob_size = wasm_blob.len(),
                diagnostic_count = diagnostics.len(),
                "compilation succeeded"
            );

            Ok(CompileResponse {
                wasm_blob,
                diagnostics,
                cached: false,
                preamble_lines,
                js_glue: Some(js_glue),
            })
        }

        BuildResult::Failure { stdout, stderr } => {
            let diagnostics = parse_diagnostics(&stdout, preamble_lines);

            tracing::warn!(
                cell_id = %request.cell_id,
                diagnostic_count = diagnostics.len(),
                stderr_len = stderr.len(),
                "compilation failed"
            );

            // Log the full stderr — this is where linker errors
            // (e.g. rust-lld failures) and other non-JSON diagnostics appear.
            if !stderr.is_empty() {
                tracing::warn!(
                    cell_id = %request.cell_id,
                    stderr = %stderr,
                    "compilation stderr",
                );
            }

            // On failure, if we parsed structured diagnostics, return them.
            // Otherwise, synthesize a single error from the raw output.
            //
            // Linker errors (e.g. rust-lld failures) appear in stderr and are
            // not captured by rustc's JSON diagnostic format, so we combine
            // both streams when building the fallback message.
            let diagnostics = if diagnostics.is_empty() {
                let raw = if stderr.is_empty() { &stdout } else { &stderr };

                // Include the full output so linker errors (undefined
                // symbols, missing libraries, etc.) are visible to the user.
                let message = format!("Compilation failed:\n{raw}");

                vec![ironpad_common::Diagnostic {
                    message,
                    severity: ironpad_common::Severity::Error,
                    spans: vec![],
                    code: None,
                }]
            } else {
                // If we have structured diagnostics but also a linker error in
                // stderr, append the linker error as an additional diagnostic
                // so it isn't silently lost.
                let mut diagnostics = diagnostics;

                if !stderr.is_empty() {
                    diagnostics.push(ironpad_common::Diagnostic {
                        message: format!("Build stderr:\n{stderr}"),
                        severity: ironpad_common::Severity::Error,
                        spans: vec![],
                        code: None,
                    });
                }

                diagnostics
            };

            Ok(CompileResponse {
                wasm_blob: vec![],
                diagnostics,
                cached: false,
                preamble_lines,
                js_glue: None,
            })
        }
    }
}

// ── Public notebooks ─────────────────────────────────────────────────────────

/// Lists all available public notebooks from the static index.
#[server]
pub async fn list_public_notebooks() -> Result<Vec<PublicNotebookSummary>, ServerFnError> {
    use ironpad_common::PublicNotebookIndex;

    let leptos_options = expect_context::<LeptosOptions>();
    let site_root: &str = &leptos_options.site_root;
    let index_path = std::path::Path::new(site_root)
        .join("notebooks")
        .join("index.json");

    let json = match tokio::fs::read_to_string(&index_path).await {
        Ok(json) => json,
        Err(_) => return Ok(vec![]),
    };

    let index: PublicNotebookIndex = serde_json::from_str(&json)
        .map_err(|e| ServerFnError::new(format!("invalid public notebook index: {e}")))?;

    Ok(index.notebooks)
}

/// Loads a public `.ironpad` notebook from the server's static files directory.
///
/// The `filename` must end with `.ironpad` and may not contain path separators.
#[server]
pub async fn get_public_notebook(filename: String) -> Result<IronpadNotebook, ServerFnError> {
    // Reject path traversal attempts.
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err(ServerFnError::new("invalid filename"));
    }

    let leptos_options = expect_context::<LeptosOptions>();
    let site_root: &str = &leptos_options.site_root;
    let path = std::path::Path::new(site_root)
        .join("notebooks")
        .join(&filename);

    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| ServerFnError::new(format!("notebook not found: {e}")))?;

    let notebook: IronpadNotebook = serde_json::from_slice(&bytes)
        .map_err(|e| ServerFnError::new(format!("invalid notebook file: {e}")))?;

    Ok(notebook)
}

// ── Shared notebooks ─────────────────────────────────────────────────────────

/// Uploads a notebook for sharing. Returns the blake3 content hash (16 hex chars).
///
/// The notebook JSON is stored at `{data_dir}/shares/{hash}.json`.
#[server]
pub async fn share_notebook(notebook_json: String) -> Result<String, ServerFnError> {
    use ironpad_common::AppConfig;

    // Validate the JSON is a valid IronpadNotebook.
    let _: IronpadNotebook = serde_json::from_str(&notebook_json)
        .map_err(|e| ServerFnError::new(format!("invalid notebook JSON: {e}")))?;

    let config = expect_context::<AppConfig>();

    // Compute blake3 hash (first 16 hex chars).
    let hash = blake3::hash(notebook_json.as_bytes());
    let hash_hex = &hash.to_hex()[..16];

    let shares_dir = config.data_dir.join("shares");
    tokio::fs::create_dir_all(&shares_dir)
        .await
        .map_err(|e| ServerFnError::new(format!("failed to create shares dir: {e}")))?;

    let path = shares_dir.join(format!("{hash_hex}.json"));
    tokio::fs::write(&path, notebook_json.as_bytes())
        .await
        .map_err(|e| ServerFnError::new(format!("failed to write shared notebook: {e}")))?;

    tracing::info!(hash = %hash_hex, "notebook shared");

    Ok(hash_hex.to_string())
}

/// Retrieves a shared notebook by its blake3 content hash.
///
/// Shared notebooks are stored as JSON blobs in `{data_dir}/shares/{hash}.json`.
#[server]
pub async fn get_shared_notebook(hash: String) -> Result<IronpadNotebook, ServerFnError> {
    use ironpad_common::AppConfig;

    // Reject path traversal attempts.
    if hash.contains('/') || hash.contains('\\') || hash.contains("..") {
        return Err(ServerFnError::new("invalid share hash"));
    }

    let config = expect_context::<AppConfig>();
    let path = config.data_dir.join("shares").join(format!("{hash}.json"));

    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| ServerFnError::new(format!("shared notebook not found: {e}")))?;

    let notebook: IronpadNotebook = serde_json::from_slice(&bytes)
        .map_err(|e| ServerFnError::new(format!("invalid shared notebook: {e}")))?;

    Ok(notebook)
}
