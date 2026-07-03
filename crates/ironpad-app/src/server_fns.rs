use ironpad_common::{CompileRequest, CompileResponse, IronpadNotebook, PublicNotebookSummary};
use leptos::prelude::*;

// ── Compilation ──────────────────────────────────────────────────────────────

/// Replace known server filesystem paths in raw compiler output with
/// placeholders so user-facing diagnostics don't leak the crate/cache
/// directories or the server's home path (raw rustc/linker output embeds them).
#[cfg(feature = "ssr")]
fn redact_server_paths(
    raw: &str,
    crate_dir: &std::path::Path,
    cache_dir: &std::path::Path,
) -> String {
    let mut out = raw.to_string();
    // Most specific first: crate_dir lives under cache_dir.
    let crate_s = crate_dir.to_string_lossy();
    if !crate_s.is_empty() {
        out = out.replace(crate_s.as_ref(), "<cell>");
    }
    let cache_s = cache_dir.to_string_lossy();
    if !cache_s.is_empty() {
        out = out.replace(cache_s.as_ref(), "<cache>");
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            out = out.replace(&home, "~");
        }
    }
    out
}

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

    // Validate the cell_id before it touches the filesystem or Cargo.toml: it is
    // joined into cache paths and interpolated into `name = "cell-{id}"`, so an
    // unvalidated id can traverse directories (`../`) or inject manifest keys.
    if !crate::compiler::scaffold::is_valid_cell_id(&request.cell_id) {
        return Err(ServerFnError::new(format!(
            "invalid cell_id {:?}: expected 1-64 chars of [A-Za-z0-9_-]",
            request.cell_id
        )));
    }

    // Serialize concurrent compiles of the same cell so their shared scaffold
    // dir can't be overwritten mid-build (which would cache one source's output
    // under another's hash). Held for the whole compile. Distinct cells don't
    // contend — they scaffold into distinct directories.
    let compile_locks = expect_context::<crate::compiler::CompileLocks>();
    let _compile_guard = compile_locks.acquire(&request.cell_id).await;

    // Scaffold first so we can detect rayon (needs_atomics) before hashing.

    let (crate_dir, preamble_lines, _is_async, _is_simulation, needs_atomics) =
        scaffold_micro_crate(
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

    let hash = content_hash(
        &request.source,
        &request.cargo_toml,
        &request.previous_cell_types,
        request.shared_cargo_toml.as_deref(),
        request.shared_source.as_deref(),
        needs_atomics,
    );
    tracing::info!(cell_id = %request.cell_id, hash = %hash, needs_atomics, "compile_cell started");

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

    // Build.

    let build_result = build_micro_crate(
        &crate_dir,
        &config.cache_dir,
        session_id,
        &request.cell_id,
        config.compilation_proxy.as_deref(),
        needs_atomics,
    )
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

            let wasm_blob = optimize_wasm(
                &wasm_bytes,
                crate_dir.parent().unwrap_or(&crate_dir),
                needs_atomics,
            )
            .await;

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
                // symbols, missing libraries, etc.) are visible to the user —
                // but redact server filesystem paths first.
                let redacted = redact_server_paths(raw, &crate_dir, &config.cache_dir);
                let message = format!("Compilation failed:\n{redacted}");

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
                    let redacted = redact_server_paths(&stderr, &crate_dir, &config.cache_dir);
                    diagnostics.push(ironpad_common::Diagnostic {
                        message: format!("Build stderr:\n{redacted}"),
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

#[cfg(feature = "ssr")]
pub(crate) async fn list_public_notebooks_core(
    site_root: &std::path::Path,
) -> anyhow::Result<Vec<PublicNotebookSummary>> {
    let notebooks_dir = site_root.join("notebooks");

    let Ok(mut read_dir) = tokio::fs::read_dir(&notebooks_dir).await else {
        return Ok(vec![]);
    };

    let mut summaries = Vec::new();
    while let Ok(Some(entry)) = read_dir.next_entry().await {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("ironpad") {
            continue;
        }
        let filename = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let Ok(bytes) = tokio::fs::read(&path).await else {
            continue;
        };
        let Ok(nb) = serde_json::from_slice::<IronpadNotebook>(&bytes) else {
            continue;
        };
        summaries.push(PublicNotebookSummary {
            id: filename.clone(),
            title: nb.title,
            description: nb.description.unwrap_or_default(),
            filename,
            cell_count: nb.cells.len(),
            tags: nb.tags.unwrap_or_default(),
        });
    }

    summaries.sort_by(|a, b| a.title.cmp(&b.title));
    Ok(summaries)
}

/// Lists all available public notebooks by enumerating `*.ironpad` files at runtime.
#[server]
pub async fn list_public_notebooks() -> Result<Vec<PublicNotebookSummary>, ServerFnError> {
    let leptos_options = expect_context::<LeptosOptions>();
    let site_root = leptos_options.site_root.as_ref();
    list_public_notebooks_core(std::path::Path::new(site_root))
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Rejects path segments that contain directory separators or `..` traversal.
#[cfg(feature = "ssr")]
fn validate_safe_path_segment(s: &str) -> anyhow::Result<()> {
    if s.contains('/') || s.contains('\\') || s.contains("..") {
        anyhow::bail!("invalid filename: must not contain separators or '..'");
    }
    Ok(())
}

#[cfg(feature = "ssr")]
pub(crate) async fn get_public_notebook_core(
    site_root: &std::path::Path,
    filename: &str,
) -> anyhow::Result<IronpadNotebook> {
    // Reject path traversal attempts.
    validate_safe_path_segment(filename)?;

    // Only serve notebook files, not arbitrary files in the directory
    // (e.g. index.json or anything else that lands under notebooks/).
    if !filename.ends_with(".ironpad") {
        anyhow::bail!("not a notebook file: {filename}");
    }

    let path = site_root.join("notebooks").join(filename);

    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| anyhow::anyhow!("notebook not found: {e}"))?;

    let notebook: IronpadNotebook = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("invalid notebook file: {e}"))?;

    Ok(notebook)
}

/// Loads a public `.ironpad` notebook from the server's static files directory.
///
/// The `filename` must end with `.ironpad` and may not contain path separators.
#[server]
pub async fn get_public_notebook(filename: String) -> Result<IronpadNotebook, ServerFnError> {
    let leptos_options = expect_context::<LeptosOptions>();
    let site_root = leptos_options.site_root.as_ref();
    get_public_notebook_core(std::path::Path::new(site_root), &filename)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

// ── Shared notebooks ─────────────────────────────────────────────────────────

/// Maximum accepted size of a shared-notebook upload (before parsing).
#[cfg(feature = "ssr")]
const MAX_SHARE_BYTES: usize = 4 * 1024 * 1024;

#[cfg(feature = "ssr")]
pub(crate) async fn share_notebook_core(
    data_dir: &std::path::Path,
    notebook_json: &str,
) -> anyhow::Result<String> {
    // Reject oversized uploads before parsing: an arbitrarily large body would
    // CPU-block the runtime in serde and fill disk with distinct large writes.
    // 4 MiB is generous for a notebook (many cells of source plus outputs).
    if notebook_json.len() > MAX_SHARE_BYTES {
        anyhow::bail!(
            "notebook too large: {} bytes (max {MAX_SHARE_BYTES})",
            notebook_json.len()
        );
    }

    // Validate the JSON is a valid IronpadNotebook.
    let _: IronpadNotebook = serde_json::from_str(notebook_json)
        .map_err(|e| anyhow::anyhow!("invalid notebook JSON: {e}"))?;

    // Compute blake3 hash (first 16 hex chars).
    let hash = blake3::hash(notebook_json.as_bytes());
    let hash_hex = &hash.to_hex()[..16];

    let shares_dir = data_dir.join("shares");
    tokio::fs::create_dir_all(&shares_dir)
        .await
        .map_err(|e| anyhow::anyhow!("failed to create shares dir: {e}"))?;

    let path = shares_dir.join(format!("{hash_hex}.json"));
    tokio::fs::write(&path, notebook_json.as_bytes())
        .await
        .map_err(|e| anyhow::anyhow!("failed to write shared notebook: {e}"))?;

    tracing::info!(hash = %hash_hex, "notebook shared");

    Ok(hash_hex.to_string())
}

/// Uploads a notebook for sharing. Returns the blake3 content hash (16 hex chars).
///
/// The notebook JSON is stored at `{data_dir}/shares/{hash}.json`.
#[server]
pub async fn share_notebook(notebook_json: String) -> Result<String, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    share_notebook_core(&config.data_dir, &notebook_json)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

#[cfg(feature = "ssr")]
pub(crate) async fn get_shared_notebook_core(
    data_dir: &std::path::Path,
    hash: &str,
) -> anyhow::Result<IronpadNotebook> {
    // Reject path traversal attempts.
    validate_safe_path_segment(hash)?;

    let path = data_dir.join("shares").join(format!("{hash}.json"));

    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|e| anyhow::anyhow!("shared notebook not found: {e}"))?;

    let notebook: IronpadNotebook = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("invalid shared notebook: {e}"))?;

    Ok(notebook)
}

/// Retrieves a shared notebook by its blake3 content hash.
///
/// Shared notebooks are stored as JSON blobs in `{data_dir}/shares/{hash}.json`.
#[server]
pub async fn get_shared_notebook(hash: String) -> Result<IronpadNotebook, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    get_shared_notebook_core(&config.data_dir, &hash)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(all(test, feature = "ssr"))]
mod tests {
    use super::*;

    #[test]
    fn redact_server_paths_strips_crate_and_cache_dirs() {
        let raw = "error in /cache/workspaces/default/cell-1/src/lib.rs; dep at /cache/registry/foo-1.0/lib.rs";
        let out = redact_server_paths(
            raw,
            std::path::Path::new("/cache/workspaces/default/cell-1"),
            std::path::Path::new("/cache"),
        );
        assert!(
            !out.contains("/cache/workspaces/default/cell-1"),
            "crate dir must be redacted: {out}"
        );
        assert!(out.contains("<cell>/src/lib.rs"), "crate replaced: {out}");
        assert!(
            out.contains("<cache>/registry/foo"),
            "cache replaced: {out}"
        );
    }

    const VALID_NOTEBOOK_JSON: &str = r#"{
        "version": 1,
        "id": "00000000-0000-0000-0000-000000000001",
        "title": "Test Notebook",
        "description": "A test notebook",
        "tags": ["test"],
        "created_at": "2024-01-01T00:00:00Z",
        "updated_at": "2024-01-01T00:00:00Z",
        "cells": [
            {
                "id": "cell-1",
                "order": 0,
                "label": "Cell 1",
                "cell_type": "Code",
                "source": "let x = 42;",
                "version": 0
            }
        ]
    }"#;

    fn second_notebook_json() -> String {
        serde_json::to_string(&IronpadNotebook::new("Second")).unwrap()
    }

    // ── share_notebook_core ──────────────────────────────────────────

    #[tokio::test]
    async fn server_fn_core_share_notebook_creates_file() {
        let dir = tempfile::tempdir().unwrap();
        let hash = share_notebook_core(dir.path(), VALID_NOTEBOOK_JSON)
            .await
            .unwrap();

        assert_eq!(hash.len(), 16, "hash should be 16 hex chars");
        let path = dir.path().join("shares").join(format!("{hash}.json"));
        assert!(path.exists(), "share file should exist on disk");
    }

    #[tokio::test]
    async fn server_fn_core_share_notebook_rejects_oversized() {
        let dir = tempfile::tempdir().unwrap();
        // Just over the cap — rejected before parsing, so it needn't be valid JSON.
        let oversized = "x".repeat(MAX_SHARE_BYTES + 1);
        let err = share_notebook_core(dir.path(), &oversized)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("too large"), "unexpected error: {err}");
        assert!(
            !dir.path().join("shares").exists(),
            "nothing should be written for an oversized upload"
        );
    }

    #[tokio::test]
    async fn server_fn_core_share_notebook_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let h1 = share_notebook_core(dir.path(), VALID_NOTEBOOK_JSON)
            .await
            .unwrap();
        let h2 = share_notebook_core(dir.path(), VALID_NOTEBOOK_JSON)
            .await
            .unwrap();

        assert_eq!(h1, h2, "same content should produce the same hash");
    }

    #[tokio::test]
    async fn server_fn_core_share_notebook_different_content_different_hash() {
        let dir = tempfile::tempdir().unwrap();
        let h1 = share_notebook_core(dir.path(), VALID_NOTEBOOK_JSON)
            .await
            .unwrap();
        let h2 = share_notebook_core(dir.path(), &second_notebook_json())
            .await
            .unwrap();

        assert_ne!(h1, h2, "different content should produce different hashes");
    }

    #[tokio::test]
    async fn server_fn_core_share_notebook_rejects_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let result = share_notebook_core(dir.path(), "not json").await;

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("invalid notebook JSON"));
    }

    // ── get_shared_notebook_core ─────────────────────────────────────

    #[tokio::test]
    async fn server_fn_core_get_shared_notebook_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let hash = share_notebook_core(dir.path(), VALID_NOTEBOOK_JSON)
            .await
            .unwrap();

        let nb = get_shared_notebook_core(dir.path(), &hash).await.unwrap();
        assert_eq!(nb.title, "Test Notebook");
        assert_eq!(nb.cells.len(), 1);
        assert_eq!(nb.cells[0].source, "let x = 42;");
    }

    #[tokio::test]
    async fn server_fn_core_get_shared_notebook_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let result = get_shared_notebook_core(dir.path(), "deadbeefdeadbeef").await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn server_fn_core_get_shared_notebook_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["../etc/passwd", "foo/bar", "foo\\bar", ".."] {
            let result = get_shared_notebook_core(dir.path(), bad).await;
            assert!(result.is_err(), "should reject traversal: {bad}");
        }
    }

    // ── list_public_notebooks_core ──────────────────────────────────

    #[tokio::test]
    async fn server_fn_core_list_public_notebooks_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("notebooks")).unwrap();

        let result = list_public_notebooks_core(dir.path()).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn server_fn_core_list_public_notebooks_missing_dir() {
        let dir = tempfile::tempdir().unwrap();
        // No "notebooks" subdirectory — should return empty, not error.
        let result = list_public_notebooks_core(dir.path()).await.unwrap();
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn server_fn_core_list_public_notebooks_finds_ironpad_files() {
        let dir = tempfile::tempdir().unwrap();
        let nb_dir = dir.path().join("notebooks");
        std::fs::create_dir_all(&nb_dir).unwrap();

        // Write two valid .ironpad files.
        std::fs::write(nb_dir.join("alpha.ironpad"), VALID_NOTEBOOK_JSON).unwrap();
        let nb2 = IronpadNotebook::new("Zeta Notebook");
        std::fs::write(
            nb_dir.join("zeta.ironpad"),
            serde_json::to_string(&nb2).unwrap(),
        )
        .unwrap();

        // Write a non-.ironpad file that should be ignored.
        std::fs::write(nb_dir.join("readme.txt"), "ignored").unwrap();

        let summaries = list_public_notebooks_core(dir.path()).await.unwrap();
        assert_eq!(summaries.len(), 2);

        // Results are sorted by title.
        assert_eq!(summaries[0].title, "Test Notebook");
        assert_eq!(summaries[0].filename, "alpha.ironpad");
        assert_eq!(summaries[0].cell_count, 1);

        assert_eq!(summaries[1].title, "Zeta Notebook");
        assert_eq!(summaries[1].filename, "zeta.ironpad");
    }

    // ── get_public_notebook_core ────────────────────────────────────

    #[tokio::test]
    async fn server_fn_core_get_public_notebook_returns_notebook() {
        let dir = tempfile::tempdir().unwrap();
        let nb_dir = dir.path().join("notebooks");
        std::fs::create_dir_all(&nb_dir).unwrap();
        std::fs::write(nb_dir.join("demo.ironpad"), VALID_NOTEBOOK_JSON).unwrap();

        let nb = get_public_notebook_core(dir.path(), "demo.ironpad")
            .await
            .unwrap();
        assert_eq!(nb.title, "Test Notebook");
        assert_eq!(nb.cells.len(), 1);
    }

    #[tokio::test]
    async fn server_fn_core_get_public_notebook_not_found() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("notebooks")).unwrap();

        let result = get_public_notebook_core(dir.path(), "nope.ironpad").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn server_fn_core_get_public_notebook_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["../secret.ironpad", "sub/file.ironpad", "a\\b", ".."] {
            let result = get_public_notebook_core(dir.path(), bad).await;
            assert!(result.is_err(), "should reject traversal: {bad}");
            assert!(result.unwrap_err().to_string().contains("invalid filename"));
        }
    }
}
