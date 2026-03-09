use ironpad_common::{
    CellContent, CellManifest, CellType, CompileRequest, CompileResponse, NotebookManifest,
    NotebookSummary,
};
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
    );
    tracing::info!(cell_id = %request.cell_id, hash = %hash, "compile_cell started");

    // Cache check.

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

    tracing::info!(cell_id = %request.cell_id, "cache miss — compiling");

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

// ── Notebook CRUD ────────────────────────────────────────────────────────────

/// Lists all notebooks, sorted by most recently updated.
#[server]
pub async fn list_notebooks() -> Result<Vec<NotebookSummary>, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();

    let summaries = crate::notebook::storage::list_notebooks(&config.data_dir)
        .map_err(|e| ServerFnError::new(format!("failed to list notebooks: {e}")))?;

    Ok(summaries)
}

/// Creates a new notebook with the given title.
#[server]
pub async fn create_notebook(title: String) -> Result<NotebookManifest, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();

    let manifest = crate::notebook::storage::create_notebook(&config.data_dir, &title)
        .map_err(|e| ServerFnError::new(format!("failed to create notebook: {e}")))?;

    Ok(manifest)
}

/// Retrieves a notebook manifest by ID.
#[server]
pub async fn get_notebook(id: String) -> Result<NotebookManifest, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let uuid = parse_uuid(&id)?;

    let manifest = crate::notebook::storage::get_notebook(&config.data_dir, &uuid)
        .map_err(|e| ServerFnError::new(format!("failed to get notebook: {e}")))?;

    Ok(manifest)
}

/// Updates a notebook's title.
#[server]
pub async fn update_notebook(id: String, title: String) -> Result<NotebookManifest, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let uuid = parse_uuid(&id)?;

    let manifest =
        crate::notebook::storage::update_notebook(&config.data_dir, &uuid, Some(&title), None)
            .map_err(|e| ServerFnError::new(format!("failed to update notebook: {e}")))?;

    Ok(manifest)
}

/// Deletes a notebook by ID.
#[server]
pub async fn delete_notebook(id: String) -> Result<(), ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let uuid = parse_uuid(&id)?;

    crate::notebook::storage::delete_notebook(&config.data_dir, &uuid)
        .map_err(|e| ServerFnError::new(format!("failed to delete notebook: {e}")))?;

    Ok(())
}

// ── Shared Cargo.toml ────────────────────────────────────────────────────────

/// Retrieves the notebook-level shared `Cargo.toml` content, if any.
#[server]
pub async fn get_shared_cargo_toml(notebook_id: String) -> Result<Option<String>, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let uuid = parse_uuid(&notebook_id)?;

    let content = crate::notebook::storage::get_shared_cargo_toml(&config.data_dir, &uuid)
        .map_err(|e| ServerFnError::new(format!("failed to read shared Cargo.toml: {e}")))?;

    Ok(content)
}

/// Creates or updates the notebook-level shared `Cargo.toml`.
#[server]
pub async fn update_shared_cargo_toml(
    notebook_id: String,
    cargo_toml: String,
) -> Result<(), ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let uuid = parse_uuid(&notebook_id)?;

    crate::notebook::storage::update_shared_cargo_toml(&config.data_dir, &uuid, &cargo_toml)
        .map_err(|e| ServerFnError::new(format!("failed to update shared Cargo.toml: {e}")))?;

    Ok(())
}

// ── Cell Content ─────────────────────────────────────────────────────────────

/// Retrieves the source code and Cargo.toml content of a cell.
#[server]
pub async fn get_cell_content(
    notebook_id: String,
    cell_id: String,
) -> Result<CellContent, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let nb_uuid = parse_uuid(&notebook_id)?;

    let source = crate::notebook::cells::get_cell_source(&config.data_dir, &nb_uuid, &cell_id)
        .map_err(|e| ServerFnError::new(format!("failed to read cell source: {e}")))?;

    let cargo_toml =
        crate::notebook::cells::get_cell_cargo_toml(&config.data_dir, &nb_uuid, &cell_id)
            .map_err(|e| ServerFnError::new(format!("failed to read cell Cargo.toml: {e}")))?;

    Ok(CellContent { source, cargo_toml })
}

// ── Cell CRUD ────────────────────────────────────────────────────────────────

/// Adds a new cell to a notebook.
///
/// If `after_cell_id` is provided, the cell is inserted after that cell;
/// otherwise it is appended at the end. Returns the new cell manifest.
#[server]
pub async fn add_cell(
    notebook_id: String,
    after_cell_id: Option<String>,
    cell_type: CellType,
) -> Result<CellManifest, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let nb_uuid = parse_uuid(&notebook_id)?;

    // Determine the next label by counting existing cells.
    let manifest = crate::notebook::storage::get_notebook(&config.data_dir, &nb_uuid)
        .map_err(|e| ServerFnError::new(format!("failed to read notebook: {e}")))?;
    let label = format!("Cell {}", manifest.cells.len());

    let cell_id = uuid::Uuid::new_v4().to_string();

    let cell = crate::notebook::cells::add_cell(
        &config.data_dir,
        &nb_uuid,
        &cell_id,
        &label,
        after_cell_id.as_deref(),
        cell_type,
    )
    .map_err(|e| ServerFnError::new(format!("failed to add cell: {e}")))?;

    Ok(cell)
}

/// Updates a cell's source code.
#[server]
pub async fn update_cell_source(
    notebook_id: String,
    cell_id: String,
    source: String,
) -> Result<(), ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let nb_uuid = parse_uuid(&notebook_id)?;

    crate::notebook::cells::update_cell_source(&config.data_dir, &nb_uuid, &cell_id, &source)
        .map_err(|e| ServerFnError::new(format!("failed to update cell source: {e}")))?;

    Ok(())
}

/// Updates a cell's Cargo.toml.
#[server]
pub async fn update_cell_cargo_toml(
    notebook_id: String,
    cell_id: String,
    cargo_toml: String,
) -> Result<(), ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let nb_uuid = parse_uuid(&notebook_id)?;

    crate::notebook::cells::update_cell_cargo_toml(
        &config.data_dir,
        &nb_uuid,
        &cell_id,
        &cargo_toml,
    )
    .map_err(|e| ServerFnError::new(format!("failed to update cell Cargo.toml: {e}")))?;

    Ok(())
}

/// Deletes a cell from a notebook.
#[server]
pub async fn delete_cell(notebook_id: String, cell_id: String) -> Result<(), ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let nb_uuid = parse_uuid(&notebook_id)?;

    crate::notebook::cells::delete_cell(&config.data_dir, &nb_uuid, &cell_id)
        .map_err(|e| ServerFnError::new(format!("failed to delete cell: {e}")))?;

    Ok(())
}

/// Reorders cells in a notebook.
///
/// `cell_ids` must contain the IDs of all existing cells in the desired order.
#[server]
pub async fn reorder_cells(
    notebook_id: String,
    cell_ids: Vec<String>,
) -> Result<(), ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let nb_uuid = parse_uuid(&notebook_id)?;

    crate::notebook::cells::reorder_cells(&config.data_dir, &nb_uuid, &cell_ids)
        .map_err(|e| ServerFnError::new(format!("failed to reorder cells: {e}")))?;

    Ok(())
}

/// Renames a cell's label.
#[server]
pub async fn rename_cell(
    notebook_id: String,
    cell_id: String,
    label: String,
) -> Result<(), ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let nb_uuid = parse_uuid(&notebook_id)?;

    crate::notebook::cells::rename_cell(&config.data_dir, &nb_uuid, &cell_id, &label)
        .map_err(|e| ServerFnError::new(format!("failed to rename cell: {e}")))?;

    Ok(())
}

/// Duplicates a cell, creating a new cell with copied source and Cargo.toml.
///
/// Returns the new cell's manifest entry.
#[server]
pub async fn duplicate_cell(
    notebook_id: String,
    cell_id: String,
) -> Result<CellManifest, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let nb_uuid = parse_uuid(&notebook_id)?;

    let new_cell_id = uuid::Uuid::new_v4().to_string();

    let cell =
        crate::notebook::cells::duplicate_cell(&config.data_dir, &nb_uuid, &cell_id, &new_cell_id)
            .map_err(|e| ServerFnError::new(format!("failed to duplicate cell: {e}")))?;

    Ok(cell)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Parses a UUID string, returning a `ServerFnError` on failure.
#[cfg(feature = "ssr")]
fn parse_uuid(id: &str) -> Result<uuid::Uuid, ServerFnError> {
    id.parse::<uuid::Uuid>()
        .map_err(|e| ServerFnError::new(format!("invalid notebook ID '{id}': {e}")))
}
