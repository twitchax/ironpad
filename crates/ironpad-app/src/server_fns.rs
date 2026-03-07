use ironpad_common::{CompileRequest, CompileResponse};
use leptos::prelude::*;

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

    let hash = content_hash(&request.source, &request.cargo_toml);
    tracing::info!(cell_id = %request.cell_id, hash = %hash, "compile_cell started");

    // Cache check.

    if let Some(wasm_blob) = try_cache_hit(&config.cache_dir, &hash) {
        tracing::info!(cell_id = %request.cell_id, blob_size = wasm_blob.len(), "cache hit");
        return Ok(CompileResponse {
            wasm_blob,
            diagnostics: vec![],
            cached: true,
        });
    }

    tracing::info!(cell_id = %request.cell_id, "cache miss — compiling");

    // Scaffold micro-crate.

    let crate_dir = scaffold_micro_crate(
        &config.cache_dir,
        &config.ironpad_cell_path,
        session_id,
        &request.cell_id,
        &request.source,
        &request.cargo_toml,
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
        } => {
            let diagnostics = parse_diagnostics(&stdout);

            let wasm_bytes = tokio::fs::read(&wasm_path)
                .await
                .map_err(|e| ServerFnError::new(format!("failed to read wasm blob: {e}")))?;

            // Best-effort optimization.

            let wasm_blob =
                optimize_wasm(&wasm_bytes, crate_dir.parent().unwrap_or(&crate_dir)).await;

            // Cache the result.

            if let Err(e) = store_blob(&config.cache_dir, &hash, &wasm_blob) {
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
            })
        }

        BuildResult::Failure { stdout, stderr } => {
            let diagnostics = parse_diagnostics(&stdout);

            tracing::info!(
                cell_id = %request.cell_id,
                diagnostic_count = diagnostics.len(),
                "compilation failed"
            );

            // On failure, if we parsed structured diagnostics, return them.
            // Otherwise, synthesize a single error from the raw output.
            let diagnostics = if diagnostics.is_empty() {
                let raw = if stderr.is_empty() { &stdout } else { &stderr };
                vec![ironpad_common::Diagnostic {
                    message: format!("Compilation failed:\n{raw}"),
                    severity: ironpad_common::Severity::Error,
                    spans: vec![],
                }]
            } else {
                diagnostics
            };

            Ok(CompileResponse {
                wasm_blob: vec![],
                diagnostics,
                cached: false,
            })
        }
    }
}
