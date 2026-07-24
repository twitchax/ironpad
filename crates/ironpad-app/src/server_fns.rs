//! Leptos `#[server]` functions — the SSR boundary between the WASM client and
//! the server.
//!
//! Each `#[server]` fn compiles to a network call on the client (hydrate) and to
//! the real implementation on the server (ssr). Server-only logic lives in
//! `ssr`-gated `*_core` helpers so it stays unit-testable without a Leptos
//! context. Endpoints: [`compile_cell`] and [`check_cell`] (the WASM
//! compilation pipeline), [`list_public_notebooks`] and [`get_public_notebook`]
//! (static `*.ironpad` notebooks under the site root), and [`share_notebook`],
//! [`get_shared_notebook`], and [`get_shared_manifest`] (content-addressed
//! shared notebooks plus their blob-snapshot sidecars under the data dir).

use ironpad_common::{
    CheckResponse, CompileRequest, CompileResponse, IronpadNotebook, PublicNotebookSummary,
};
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

    let config = expect_context::<AppConfig>();
    let compile_locks = expect_context::<crate::compiler::CompileLocks>();
    compile_cell_core(&config, &compile_locks, request).await
}

/// Server-side compilation pipeline behind [`compile_cell`].
///
/// Split out from the `#[server]` wrapper so it takes an explicit [`AppConfig`]
/// and [`CompileLocks`] instead of Leptos context, which keeps it unit-testable
/// (mirrors the `*_core` helpers for the other server functions).
#[cfg(feature = "ssr")]
async fn compile_cell_core(
    config: &ironpad_common::AppConfig,
    compile_locks: &crate::compiler::CompileLocks,
    request: CompileRequest,
) -> Result<CompileResponse, ServerFnError> {
    use crate::compiler::{
        build::{build_micro_crate, BuildResult},
        cache::{content_hash, store_blob, try_cache_hit},
        diagnostics::parse_diagnostics,
        optimize::optimize_wasm,
        scaffold::{
            merged_deps_contain_rayon, scaffold_micro_crate, uses_std_autodiff, uses_wasm_simd,
        },
    };

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
    let _compile_guard = compile_locks.acquire(&request.cell_id).await;

    // `needs_atomics` is a pure function of the inputs (no I/O), so the cache key
    // can be derived before scaffolding. Scaffolding writes Cargo.toml + lib.rs
    // (+ shared.rs) to disk, so defer it until a confirmed cache miss: a repeat
    // compile of an unchanged cell (the common case, a hot path) hits the cache
    // and must not pay those filesystem writes.
    let needs_atomics =
        merged_deps_contain_rayon(request.shared_cargo_toml.as_deref(), &request.cargo_toml);
    let needs_autodiff = uses_std_autodiff(&request.source, request.shared_source.as_deref());
    let needs_simd = uses_wasm_simd(&request.source, request.shared_source.as_deref());

    let hash = content_hash(
        &request.source,
        &request.cargo_toml,
        &request.previous_cell_types,
        request.shared_cargo_toml.as_deref(),
        request.shared_source.as_deref(),
        needs_atomics,
        needs_autodiff,
        needs_simd,
    );
    tracing::info!(cell_id = %request.cell_id, hash = %hash, needs_atomics, needs_autodiff, needs_simd, "compile_cell started");

    // Cache check (skipped when force-recompile is requested).

    if !request.force {
        if let Some(cache_hit) = try_cache_hit(&config.cache_dir, &hash) {
            tracing::info!(cell_id = %request.cell_id, blob_size = cache_hit.wasm_bytes.len(), "cache hit");
            return Ok(CompileResponse {
                wasm_blob: cache_hit.wasm_bytes,
                diagnostics: cache_hit.diagnostics,
                cached: true,
                // Cached diagnostics were span-adjusted before storage, so no
                // preamble offset is needed on a hit; the client never reads this
                // field for a cached response.
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

    // Cache miss (or forced recompile): scaffold the micro-crate now — the
    // on-disk work a cache hit skips. Its returned `needs_atomics` matches the
    // value hashed above (both derive from the same inputs), so we keep ours.

    let (crate_dir, preamble_lines, _is_async, _is_simulation) = scaffold_micro_crate(
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

    let build_result = build_micro_crate(
        &crate_dir,
        &config.cache_dir,
        session_id,
        &request.cell_id,
        config.compilation_proxy.as_deref(),
        needs_atomics,
        needs_autodiff,
        needs_simd,
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

            if let Err(e) = store_blob(
                &config.cache_dir,
                &hash,
                &wasm_blob,
                Some(&js_glue),
                &diagnostics,
            ) {
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

// ── Live check (PRD-0045) ────────────────────────────────────────────────────

/// Type-check a cell without codegen, for live editor diagnostics.
///
/// Same pipeline as [`compile_cell`] minus LLVM/link/wasm-bindgen: shared
/// scaffolding, toolchain selection, RUSTFLAGS, and preamble-adjusted
/// diagnostics — a cell that checks clean here builds clean there. Designed
/// to never block typing: the per-cell lock is try-acquired (busy compiles
/// yield `Skipped`), and the check runs under a short budget (`TimedOut`
/// instead of hanging on a cold dependency tree).
#[server]
pub async fn check_cell(request: CompileRequest) -> Result<CheckResponse, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let compile_locks = expect_context::<crate::compiler::CompileLocks>();
    check_cell_core(&config, &compile_locks, request).await
}

/// Time budget for a single live check (default 10s, override with
/// `IRONPAD_LIVE_CHECK_TIMEOUT_SECS`). Warm incremental checks land in 1-3s
/// on the production hardware; anything past the budget is a cold cache the
/// warmth policy should have caught, and the round degrades to "no markers".
/// The env override exists for dev/test hosts whose target dirs lack check
/// artifacts (the deploy image seeds them; local caches accumulate them).
#[cfg(feature = "ssr")]
fn live_check_timeout() -> std::time::Duration {
    let secs = std::env::var("IRONPAD_LIVE_CHECK_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(10);
    std::time::Duration::from_secs(secs)
}

/// Server-side live-check pipeline behind [`check_cell`].
#[cfg(feature = "ssr")]
async fn check_cell_core(
    config: &ironpad_common::AppConfig,
    compile_locks: &crate::compiler::CompileLocks,
    request: CompileRequest,
) -> Result<CheckResponse, ServerFnError> {
    use crate::compiler::{
        build::{check_micro_crate, CheckResult, CheckTimedOut},
        diagnostics::{parse_diagnostics, parse_shared_range_diagnostics},
        scaffold::{
            merged_deps_contain_rayon, scaffold_micro_crate, uses_std_autodiff, uses_wasm_simd,
        },
    };
    use ironpad_common::CheckStatus;

    let session_id = "default";

    if !crate::compiler::scaffold::is_valid_cell_id(&request.cell_id) {
        return Err(ServerFnError::new(format!(
            "invalid cell_id {:?}: expected 1-64 chars of [A-Za-z0-9_-]",
            request.cell_id
        )));
    }

    // Never queue a live check behind an in-flight compile (or another
    // check) of the same cell: skip and let the client try again after the
    // next quiet period. The post-compile diagnostics cover this window.
    let Some(_check_guard) = compile_locks.try_acquire(&request.cell_id) else {
        return Ok(CheckResponse {
            status: CheckStatus::Skipped,
            diagnostics: vec![],
        });
    };

    let needs_atomics =
        merged_deps_contain_rayon(request.shared_cargo_toml.as_deref(), &request.cargo_toml);
    let needs_autodiff = uses_std_autodiff(&request.source, request.shared_source.as_deref());
    let needs_simd = uses_wasm_simd(&request.source, request.shared_source.as_deref());

    let (crate_dir, preamble_lines, _is_async, _is_simulation) = scaffold_micro_crate(
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

    let result = check_micro_crate(
        &crate_dir,
        &config.cache_dir,
        session_id,
        &request.cell_id,
        config.compilation_proxy.as_deref(),
        needs_atomics,
        needs_autodiff,
        needs_simd,
        live_check_timeout(),
    )
    .await;

    match result {
        Ok(CheckResult::Ok) => Ok(CheckResponse {
            status: CheckStatus::Clean,
            diagnostics: vec![],
        }),
        Ok(CheckResult::Failure { stdout, .. }) => {
            // A shared-cell check (PRD-0046) anchors only diagnostics landing
            // inside the target cell's slice of shared.rs, remapped to
            // cell-local lines; an ordinary check maps the cell body. An
            // empty list under `Errors` is meaningful for shared checks: the
            // assembly failed, but not in this cell, so its markers clear.
            let diagnostics = match request.shared_check {
                Some(range) => {
                    parse_shared_range_diagnostics(&stdout, range.start_line, range.line_count)
                }
                None => parse_diagnostics(&stdout, preamble_lines),
            };
            Ok(CheckResponse {
                status: CheckStatus::Errors,
                diagnostics,
            })
        }
        Err(e) if e.is::<CheckTimedOut>() => {
            tracing::info!(cell_id = %request.cell_id, "live check timed out — cold cache");
            Ok(CheckResponse {
                status: CheckStatus::TimedOut,
                diagnostics: vec![],
            })
        }
        Err(e) => Err(ServerFnError::new(format!("check invocation failed: {e}"))),
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

    // Accept both name forms (PRD-0048): the canonical route is
    // extension-less (`/public/welcome`), while legacy links and embed specs
    // on third-party pages carry `.ironpad` forever. Appending the extension
    // when missing also keeps this endpoint serving ONLY notebook files —
    // any other name resolves to `{name}.ironpad`, which won't exist.
    let filename = if filename.ends_with(".ironpad") {
        filename.to_string()
    } else {
        format!("{filename}.ironpad")
    };

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

/// Maximum accepted size of a single shared-notebook upload (before parsing).
#[cfg(feature = "ssr")]
/// Maximum accepted size for one shared-notebook upload. The server binary
/// derives its framework-level body cap from this — keep them coupled.
pub const MAX_SHARE_BYTES: usize = 4 * 1024 * 1024;

/// Aggregate cap on the whole shares directory. Even with the per-upload cap, an
/// attacker could otherwise post many *distinct* notebooks to fill the disk;
/// beyond this total the endpoint refuses new distinct shares. Idempotent
/// re-shares of an already-stored hash overwrite in place and are always
/// allowed (they add no bytes). 512 MiB is generous for a single-author deploy.
#[cfg(feature = "ssr")]
const MAX_TOTAL_SHARE_BYTES: u64 = 512 * 1024 * 1024;

#[cfg(feature = "ssr")]
pub(crate) async fn share_notebook_core(
    data_dir: &std::path::Path,
    notebook_json: &str,
) -> anyhow::Result<String> {
    share_notebook_core_capped(data_dir, notebook_json, MAX_TOTAL_SHARE_BYTES).await
}

/// Sum of the sizes of all regular files directly under `dir`. Returns `Ok(0)`
/// if the directory doesn't exist yet.
#[cfg(feature = "ssr")]
async fn dir_total_bytes(dir: &std::path::Path) -> anyhow::Result<u64> {
    let mut total: u64 = 0;
    let mut read_dir = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(anyhow::anyhow!("failed to read shares dir: {e}")),
    };
    while let Some(entry) = read_dir
        .next_entry()
        .await
        .map_err(|e| anyhow::anyhow!("failed to enumerate shares dir: {e}"))?
    {
        if let Ok(meta) = entry.metadata().await {
            if meta.is_file() {
                total = total.saturating_add(meta.len());
            }
        }
    }
    Ok(total)
}

/// Core share logic with an explicit aggregate cap (so tests can exercise the
/// cap without writing hundreds of MiB).
#[cfg(feature = "ssr")]
async fn share_notebook_core_capped(
    data_dir: &std::path::Path,
    notebook_json: &str,
    max_total_bytes: u64,
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

    // Enforce the aggregate cap only for a *new* distinct share — re-sharing an
    // existing hash overwrites in place and adds no bytes. (Minor TOCTOU under
    // concurrent shares is acceptable: the cap bounds steady-state disk use.)
    if !path.exists() {
        let existing_total = dir_total_bytes(&shares_dir).await?;
        let projected = existing_total.saturating_add(notebook_json.len() as u64);
        if projected > max_total_bytes {
            anyhow::bail!(
                "share store full: {existing_total} bytes stored, aggregate cap is {max_total_bytes}"
            );
        }
    }

    tokio::fs::write(&path, notebook_json.as_bytes())
        .await
        .map_err(|e| anyhow::anyhow!("failed to write shared notebook: {e}"))?;

    tracing::info!(hash = %hash_hex, "notebook shared");

    Ok(hash_hex.to_string())
}

/// Uploads a notebook for sharing. Returns the blake3 content hash (16 hex chars).
///
/// The notebook JSON is stored at `{data_dir}/shares/{hash}.json`. When the
/// client supplies positional `cell_type_tags` (one per cell, empty for
/// markdown/shared/unrun cells), the server also snapshots each cell's
/// compiled blob from the compile cache into `{data_dir}/shares/blobs/` and
/// writes a `{hash}.manifest.json` sidecar (PRD-0047) — best-effort: a failed
/// or partial snapshot never fails the share, it only means viewers fall back
/// to live compilation for the missing cells.
#[server]
pub async fn share_notebook(
    notebook_json: String,
    // Option, NOT Vec: the URL-encoded server-fn body omits an empty Vec
    // entirely, and a bare Vec then fails deserialization with "missing
    // field" — which broke sharing zero-cell notebooks. A missing field
    // deserializes to None implicitly, which also tolerates stale clients
    // that predate the argument.
    cell_type_tags: Option<Vec<String>>,
) -> Result<String, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let hash = share_notebook_core(&config.data_dir, &notebook_json)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;

    // The JSON parsed inside share_notebook_core, so a failure here is
    // unreachable in practice; treat it as "nothing to snapshot".
    if let Ok(notebook) = serde_json::from_str::<IronpadNotebook>(&notebook_json) {
        match snapshot_share_blobs(
            &config.data_dir,
            &config.cache_dir,
            &notebook,
            &cell_type_tags.unwrap_or_default(),
            &hash,
        )
        .await
        {
            Ok(count) => {
                tracing::info!(share = %hash, cells = count, "share blob snapshot written");
            }
            Err(e) => {
                tracing::warn!(share = %hash, error = %e, "share blob snapshot failed; share is live-compile only");
            }
        }
    }

    Ok(hash)
}

// ── Toolchain fingerprint (PRD-0047) ─────────────────────────────────────────

/// The server's toolchain fingerprint (rustc + wasm-bindgen CLI versions).
///
/// The client folds this into the shared cache-key recipe
/// (`ironpad_common::cache_key`) for its local IndexedDB blob store, so its
/// keys match the server's exactly — and a deploy/toolchain bump invalidates
/// every client-side entry for free, the same way it invalidates the server
/// cache.
#[server]
#[allow(clippy::unused_async)] // `#[server]` requires an async fn.
pub async fn get_toolchain_fingerprint() -> Result<String, ServerFnError> {
    Ok(crate::compiler::toolchain::toolchain_fingerprint().to_string())
}

// ── Share blob snapshots (PRD-0047) ──────────────────────────────────────────

/// Aggregate cap on the share blob snapshot directory. Compiled blobs are
/// megabytes each, so they get their own budget separate from the
/// notebook-JSON cap; beyond it new shares degrade to live compilation (no
/// new snapshot entries) instead of failing the share.
#[cfg(feature = "ssr")]
const MAX_TOTAL_SHARE_BLOB_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Directory holding snapshotted share blobs, content-addressed by cache key
/// (so identical cells across shares dedupe into one file).
#[cfg(feature = "ssr")]
fn share_blobs_dir(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("shares").join("blobs")
}

/// Path of the blob-manifest sidecar for a share hash.
#[cfg(feature = "ssr")]
fn share_manifest_path(data_dir: &std::path::Path, share_hash: &str) -> std::path::PathBuf {
    data_dir
        .join("shares")
        .join(format!("{share_hash}.manifest.json"))
}

/// Write `contents` atomically: a uniquely-named temp sibling, then rename.
/// Async flavor of `compiler::cache`'s atomic write — a concurrent reader
/// (the `/share-blobs/` route, another in-flight share) sees either the old
/// file or the fully written new one, never a truncated partial.
#[cfg(feature = "ssr")]
async fn atomic_write_async(path: &std::path::Path, contents: &[u8]) -> anyhow::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(format!(".tmp.{}", uuid::Uuid::new_v4()));
    let tmp = std::path::PathBuf::from(tmp);

    tokio::fs::write(&tmp, contents)
        .await
        .map_err(|e| anyhow::anyhow!("failed to write {}: {e}", tmp.display()))?;
    if let Err(e) = tokio::fs::rename(&tmp, path).await {
        let _ = tokio::fs::remove_file(&tmp).await; // best-effort cleanup
        return Err(anyhow::anyhow!(
            "failed to rename into {}: {e}",
            path.display()
        ));
    }
    Ok(())
}

/// Snapshot the compiled artifacts of a shared notebook (PRD-0047).
///
/// For each runnable cell, recompute the cache key from the notebook content
/// plus the sharer-supplied positional type tags and copy CACHE HITS into
/// `{data_dir}/shares/blobs/`, recording them in the `{share_hash}.manifest.json`
/// sidecar. Cache misses are skipped, never compiled: the sharer just ran
/// these cells so they are warm, a cold cell means an unrun cell whose tag
/// chain is unreliable anyway, and cache-only keeps share latency at
/// file-copy speed. The manifest merges with any existing one (blobs are
/// content-addressed and immutable, so prior entries stay valid; a re-share
/// against a colder cache must not reduce coverage).
///
/// Returns the number of cells covered by the merged manifest.
#[cfg(feature = "ssr")]
pub(crate) async fn snapshot_share_blobs(
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
    notebook: &IronpadNotebook,
    cell_type_tags: &[String],
    share_hash: &str,
) -> anyhow::Result<usize> {
    snapshot_share_blobs_capped(
        data_dir,
        cache_dir,
        notebook,
        cell_type_tags,
        share_hash,
        MAX_TOTAL_SHARE_BLOB_BYTES,
    )
    .await
}

/// Write the cache-hit compiled blobs for a notebook's runnable cells into the
/// content-addressed `{data_dir}/shares/blobs/` store, returning fresh manifest
/// entries (`cell id → ShareBlobEntry`) for the cells that hit.
///
/// This is the shared blob-writing core: immutable shares MERGE the returned
/// entries into an on-disk `{hash}.manifest.json` sidecar
/// ([`snapshot_share_blobs_capped`]), while mutable shares (PRD-0049) REPLACE
/// their embedded manifest with a fresh `ShareManifest` built from exactly
/// these entries. Both classes serve the resulting blobs from the same
/// immutable `/share-blobs/` route, since the blobs are keyed by content hash
/// and shared across every share that references them.
#[cfg(feature = "ssr")]
async fn write_cell_blobs_capped(
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
    notebook: &IronpadNotebook,
    cell_type_tags: &[String],
    max_total_bytes: u64,
) -> anyhow::Result<std::collections::BTreeMap<String, ironpad_common::ShareBlobEntry>> {
    use crate::compiler::cache::{content_hash, try_cache_hit};
    use crate::compiler::scaffold::{merged_deps_contain_rayon, uses_std_autodiff, uses_wasm_simd};
    use ironpad_common::ShareBlobEntry;

    // One positional tag per cell is the contract (empty = no tag); a
    // mismatched vector (stale client bundle) would hash garbage chains, so
    // bail before doing any work.
    if cell_type_tags.len() != notebook.cells.len() {
        anyhow::bail!(
            "cell_type_tags length {} does not match cell count {}",
            cell_type_tags.len(),
            notebook.cells.len()
        );
    }

    let blobs_dir = share_blobs_dir(data_dir);
    // Budget check up front; a snapshot adds at most a few MB per cell, and
    // the minor TOCTOU under concurrent shares is fine — the cap bounds
    // steady-state disk use, it is not a hard quota.
    let existing_total = dir_total_bytes(&blobs_dir).await?;
    if existing_total > max_total_bytes {
        anyhow::bail!(
            "share blob store full: {existing_total} bytes stored, cap is {max_total_bytes}"
        );
    }
    tokio::fs::create_dir_all(&blobs_dir)
        .await
        .map_err(|e| anyhow::anyhow!("failed to create share blobs dir: {e}"))?;

    // Mirror the editor's CompileRequest exactly (cell_item.rs): raw
    // notebook-level shared_cargo_toml, EFFECTIVE shared source (notebook
    // shared source plus shared cells), positional tag chain over ALL
    // preceding cells. Any divergence changes the hash and misses the cache.
    let shared_cargo_toml = notebook.shared_cargo_toml.as_deref();
    let effective_shared = notebook.effective_shared_source();
    let shared_source = effective_shared.as_deref();

    let mut fresh_entries = std::collections::BTreeMap::new();
    for (idx, cell) in notebook.cells.iter().enumerate() {
        if !cell.is_runnable() {
            continue;
        }
        let cargo_toml = cell.cargo_toml.clone().unwrap_or_default();
        let needs_atomics = merged_deps_contain_rayon(shared_cargo_toml, &cargo_toml);
        let needs_autodiff = uses_std_autodiff(&cell.source, shared_source);
        let needs_simd = uses_wasm_simd(&cell.source, shared_source);
        let hash = content_hash(
            &cell.source,
            &cargo_toml,
            &cell_type_tags[..idx],
            shared_cargo_toml,
            shared_source,
            needs_atomics,
            needs_autodiff,
            needs_simd,
        );

        let Some(hit) = try_cache_hit(cache_dir, &hash) else {
            tracing::debug!(cell_id = %cell.id, hash = %hash, "share snapshot: cache miss, skipping cell");
            continue;
        };

        atomic_write_async(&blobs_dir.join(format!("{hash}.wasm")), &hit.wasm_bytes).await?;
        if let Some(glue) = hit.js_glue.as_deref() {
            atomic_write_async(&blobs_dir.join(format!("{hash}.js")), glue.as_bytes()).await?;
        }
        fresh_entries.insert(
            cell.id.clone(),
            ShareBlobEntry {
                blob: hash,
                has_js_glue: hit.js_glue.is_some(),
            },
        );
    }

    Ok(fresh_entries)
}

/// Core snapshot logic with an explicit blob-dir cap (so tests can exercise
/// the cap without writing gigabytes). See [`snapshot_share_blobs`].
#[cfg(feature = "ssr")]
async fn snapshot_share_blobs_capped(
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
    notebook: &IronpadNotebook,
    cell_type_tags: &[String],
    share_hash: &str,
    max_total_bytes: u64,
) -> anyhow::Result<usize> {
    use ironpad_common::ShareManifest;

    let fresh_entries =
        write_cell_blobs_capped(data_dir, cache_dir, notebook, cell_type_tags, max_total_bytes)
            .await?;

    let manifest_path = share_manifest_path(data_dir, share_hash);
    let mut manifest = match tokio::fs::read(&manifest_path).await {
        Ok(bytes) => serde_json::from_slice::<ShareManifest>(&bytes).unwrap_or(ShareManifest {
            version: 1,
            cells: std::collections::BTreeMap::new(),
        }),
        Err(_) => ShareManifest {
            version: 1,
            cells: std::collections::BTreeMap::new(),
        },
    };
    manifest.cells.extend(fresh_entries);

    if !manifest.cells.is_empty() {
        atomic_write_async(&manifest_path, &serde_json::to_vec(&manifest)?).await?;
    }
    Ok(manifest.cells.len())
}

#[cfg(feature = "ssr")]
pub(crate) async fn get_shared_manifest_core(
    data_dir: &std::path::Path,
    hash: &str,
) -> anyhow::Result<Option<ironpad_common::ShareManifest>> {
    // Reject path traversal attempts.
    validate_safe_path_segment(hash)?;

    let bytes = match tokio::fs::read(share_manifest_path(data_dir, hash)).await {
        Ok(bytes) => bytes,
        // No sidecar = a pre-PRD-0047 share (or a degraded snapshot): not an
        // error, the viewer just compiles live.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(anyhow::anyhow!("failed to read share manifest: {e}")),
    };

    // A corrupt manifest degrades to live compilation rather than failing the
    // page load.
    Ok(serde_json::from_slice(&bytes).ok())
}

/// Retrieves the blob-snapshot manifest for a shared notebook, if one exists.
///
/// `None` for shares created before PRD-0047 or whose snapshot was skipped —
/// the viewer falls back to live compilation.
#[server]
pub async fn get_shared_manifest(
    hash: String,
) -> Result<Option<ironpad_common::ShareManifest>, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    get_shared_manifest_core(&config.data_dir, &hash)
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

// ── Mutable shares (PRD-0049) ────────────────────────────────────────────────
//
// A third storage class: `Share Mutable` CONVERTS a private notebook into a
// server-backed one at `/mutable/{id}`. Anyone with the link reads it; the
// author overwrites it with an explicit Push. Authorization is two
// device-minted keys (a per-profile user key and a per-share notebook key) —
// the server accepts EITHER — hashed at rest with domain-separated blake3. No
// accounts: possession of a key is the whole identity model.
//
// Blobs reuse the immutable content-addressed `shares/blobs/` store (served by
// the `/share-blobs/` route), so mutable readers never trigger compiles for
// snapshotted cells. Only the id→content resolve is mutable (and served
// no-cache by the server's default policy). Each push RE-snapshots and REPLACES
// the embedded manifest, versus the merge semantics of immutable shares.

/// Aggregate cap on the mutable-share record directory (JSON records only;
/// blobs live in the shared blob store under its own 2 GiB cap). Beyond it,
/// creating a *new* mutable share is refused; pushing to an existing one
/// overwrites in place and is always allowed.
#[cfg(feature = "ssr")]
const MAX_TOTAL_MUTABLE_BYTES: u64 = 512 * 1024 * 1024;

/// Domain-separation context for mutable-share key hashing. A fixed string
/// (not a per-row salt): the keys are machine-generated full-entropy 256-bit
/// values, so salting/argon2 buy nothing, and an unsalted user-key hash is
/// what keeps enumeration ([`list_mutable_by_user_core`]) an index match. The
/// `v1` guards a future scheme change. See PRD-0049.
#[cfg(feature = "ssr")]
const MUTABLE_KEY_CONTEXT: &str = "ironpad mutable-share auth v1";

/// The on-disk record for one mutable share at `{data_dir}/mutable/{id}.json`.
/// Holds everything a reader or the author needs behind one atomic write: the
/// notebook, the two key hashes (never served), the blob manifest, and the
/// last-push timestamp.
#[cfg(feature = "ssr")]
#[derive(serde::Serialize, serde::Deserialize)]
struct MutableShareRecord {
    version: u32,
    notebook: IronpadNotebook,
    /// Hex blake3 (`derive_key`) of the owner's per-profile user key.
    user_key_hash: String,
    /// Hex blake3 (`derive_key`) of this share's per-notebook key.
    notebook_key_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    manifest: Option<ironpad_common::ShareManifest>,
    /// ISO 8601 timestamp of the last create/push.
    pushed_at: String,
}

/// Domain-separated blake3 hash of a key, as lowercase hex.
#[cfg(feature = "ssr")]
fn hash_mutable_key(key: &str) -> String {
    blake3::Hash::from(blake3::derive_key(MUTABLE_KEY_CONTEXT, key.as_bytes()))
        .to_hex()
        .to_string()
}

/// Constant-time check that `key` hashes to `stored_hex`. Length-guarded (a
/// corrupt stored hash simply never matches); the compare itself is
/// constant-time via `subtle` so a push endpoint can't become a timing oracle.
#[cfg(feature = "ssr")]
fn mutable_key_matches(key: &str, stored_hex: &str) -> bool {
    use subtle::ConstantTimeEq as _;
    let computed = hash_mutable_key(key);
    if computed.len() != stored_hex.len() {
        return false;
    }
    computed.as_bytes().ct_eq(stored_hex.as_bytes()).into()
}

#[cfg(feature = "ssr")]
fn mutable_dir(data_dir: &std::path::Path) -> std::path::PathBuf {
    data_dir.join("mutable")
}

#[cfg(feature = "ssr")]
fn mutable_record_path(data_dir: &std::path::Path, id: &str) -> std::path::PathBuf {
    mutable_dir(data_dir).join(format!("{id}.json"))
}

/// A fresh unguessable 16-hex share id (64 bits of randomness, matching the
/// immutable-share hash posture — the id doubles as the read capability).
#[cfg(feature = "ssr")]
fn random_mutable_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..16].to_string()
}

/// Snapshot a notebook's warm blobs into the shared content-addressed store
/// and build a FRESH manifest (replace semantics). Best-effort: a snapshot
/// failure (mismatched tags, cold cache, full blob store) degrades to a
/// live-compile mutable share rather than failing the create/push.
#[cfg(feature = "ssr")]
async fn snapshot_mutable_manifest(
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
    notebook: &IronpadNotebook,
    cell_type_tags: &[String],
) -> Option<ironpad_common::ShareManifest> {
    match write_cell_blobs_capped(
        data_dir,
        cache_dir,
        notebook,
        cell_type_tags,
        MAX_TOTAL_SHARE_BLOB_BYTES,
    )
    .await
    {
        Ok(cells) if !cells.is_empty() => Some(ironpad_common::ShareManifest { version: 1, cells }),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(error = %e, "mutable snapshot failed; share is live-compile only");
            None
        }
    }
}

#[cfg(feature = "ssr")]
pub(crate) async fn create_mutable_share_core(
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
    notebook_json: &str,
    user_key: &str,
    notebook_key: &str,
    cell_type_tags: &[String],
) -> anyhow::Result<String> {
    if notebook_json.len() > MAX_SHARE_BYTES {
        anyhow::bail!(
            "notebook too large: {} bytes (max {MAX_SHARE_BYTES})",
            notebook_json.len()
        );
    }
    if user_key.is_empty() || notebook_key.is_empty() {
        anyhow::bail!("both a user key and a notebook key are required");
    }

    let notebook: IronpadNotebook = serde_json::from_str(notebook_json)
        .map_err(|e| anyhow::anyhow!("invalid notebook JSON: {e}"))?;

    let mdir = mutable_dir(data_dir);
    tokio::fs::create_dir_all(&mdir)
        .await
        .map_err(|e| anyhow::anyhow!("failed to create mutable dir: {e}"))?;

    // Aggregate cap for a NEW share (minor TOCTOU is fine; the cap bounds
    // steady-state disk, it is not a hard quota).
    let existing_total = dir_total_bytes(&mdir).await?;
    if existing_total.saturating_add(notebook_json.len() as u64) > MAX_TOTAL_MUTABLE_BYTES {
        anyhow::bail!(
            "mutable share store full: {existing_total} bytes stored, cap is {MAX_TOTAL_MUTABLE_BYTES}"
        );
    }

    // Mint an id that isn't already taken. Collisions are astronomically
    // unlikely at 64 bits; the loop is a belt-and-suspenders guard.
    let mut id = None;
    for _ in 0..8 {
        let candidate = random_mutable_id();
        if !mutable_record_path(data_dir, &candidate).exists() {
            id = Some(candidate);
            break;
        }
    }
    let id = id.ok_or_else(|| anyhow::anyhow!("failed to mint a unique mutable id"))?;

    let manifest = snapshot_mutable_manifest(data_dir, cache_dir, &notebook, cell_type_tags).await;
    let record = MutableShareRecord {
        version: 1,
        notebook,
        user_key_hash: hash_mutable_key(user_key),
        notebook_key_hash: hash_mutable_key(notebook_key),
        manifest,
        pushed_at: chrono::Utc::now().to_rfc3339(),
    };
    atomic_write_async(
        &mutable_record_path(data_dir, &id),
        &serde_json::to_vec(&record)?,
    )
    .await?;

    tracing::info!(id = %id, "mutable share created");
    Ok(id)
}

#[cfg(feature = "ssr")]
pub(crate) async fn push_mutable_core(
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
    id: &str,
    key: &str,
    notebook_json: &str,
    cell_type_tags: &[String],
) -> anyhow::Result<()> {
    validate_safe_path_segment(id)?;
    if notebook_json.len() > MAX_SHARE_BYTES {
        anyhow::bail!(
            "notebook too large: {} bytes (max {MAX_SHARE_BYTES})",
            notebook_json.len()
        );
    }

    let path = mutable_record_path(data_dir, id);
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|_| anyhow::anyhow!("mutable share not found"))?;
    let mut record: MutableShareRecord = serde_json::from_slice(&bytes)
        .map_err(|e| anyhow::anyhow!("invalid mutable share record: {e}"))?;

    // Either key authorizes a push (PRD-0049). A wrong key is rejected here,
    // before any blob work.
    if !mutable_key_matches(key, &record.user_key_hash)
        && !mutable_key_matches(key, &record.notebook_key_hash)
    {
        anyhow::bail!("unauthorized: key does not match this share");
    }

    let notebook: IronpadNotebook = serde_json::from_str(notebook_json)
        .map_err(|e| anyhow::anyhow!("invalid notebook JSON: {e}"))?;
    let manifest = snapshot_mutable_manifest(data_dir, cache_dir, &notebook, cell_type_tags).await;

    // Key hashes are preserved; only content + manifest + timestamp change.
    record.notebook = notebook;
    record.manifest = manifest;
    record.pushed_at = chrono::Utc::now().to_rfc3339();
    atomic_write_async(&path, &serde_json::to_vec(&record)?).await?;

    tracing::info!(id = %id, "mutable share pushed");
    Ok(())
}

/// Read a mutable share record, mapping a missing file to `Ok(None)`.
#[cfg(feature = "ssr")]
async fn read_mutable_record(
    data_dir: &std::path::Path,
    id: &str,
) -> anyhow::Result<Option<MutableShareRecord>> {
    validate_safe_path_segment(id)?;
    match tokio::fs::read(mutable_record_path(data_dir, id)).await {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| anyhow::anyhow!("invalid mutable share record: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::anyhow!("failed to read mutable share: {e}")),
    }
}

#[cfg(feature = "ssr")]
pub(crate) async fn get_mutable_notebook_core(
    data_dir: &std::path::Path,
    id: &str,
) -> anyhow::Result<Option<IronpadNotebook>> {
    Ok(read_mutable_record(data_dir, id)
        .await?
        .map(|r| r.notebook))
}

#[cfg(feature = "ssr")]
pub(crate) async fn get_mutable_manifest_core(
    data_dir: &std::path::Path,
    id: &str,
) -> anyhow::Result<Option<ironpad_common::ShareManifest>> {
    Ok(read_mutable_record(data_dir, id)
        .await?
        .and_then(|r| r.manifest))
}

#[cfg(feature = "ssr")]
pub(crate) async fn verify_mutable_key_core(
    data_dir: &std::path::Path,
    id: &str,
    key: &str,
) -> anyhow::Result<bool> {
    Ok(read_mutable_record(data_dir, id).await?.is_some_and(|r| {
        mutable_key_matches(key, &r.user_key_hash) || mutable_key_matches(key, &r.notebook_key_hash)
    }))
}

#[cfg(feature = "ssr")]
pub(crate) async fn delete_mutable_core(
    data_dir: &std::path::Path,
    id: &str,
    key: &str,
) -> anyhow::Result<()> {
    let record = read_mutable_record(data_dir, id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("mutable share not found"))?;
    if !mutable_key_matches(key, &record.user_key_hash)
        && !mutable_key_matches(key, &record.notebook_key_hash)
    {
        anyhow::bail!("unauthorized: key does not match this share");
    }
    // The content-addressed blobs are shared across every share that
    // references them and bounded by the blob-store cap, so (like immutable
    // unshares) they are left in place; only the record is removed.
    tokio::fs::remove_file(mutable_record_path(data_dir, id))
        .await
        .map_err(|e| anyhow::anyhow!("failed to delete mutable share: {e}"))?;
    tracing::info!(id = %id, "mutable share unpublished");
    Ok(())
}

#[cfg(feature = "ssr")]
pub(crate) async fn list_mutable_by_user_core(
    data_dir: &std::path::Path,
    user_key: &str,
) -> anyhow::Result<Vec<ironpad_common::MutableShareSummary>> {
    use ironpad_common::MutableShareSummary;

    if user_key.is_empty() {
        return Ok(vec![]);
    }
    let target = hash_mutable_key(user_key);

    let mut read_dir = match tokio::fs::read_dir(mutable_dir(data_dir)).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(anyhow::anyhow!("failed to read mutable dir: {e}")),
    };

    let mut out = Vec::new();
    while let Some(entry) = read_dir
        .next_entry()
        .await
        .map_err(|e| anyhow::anyhow!("failed to enumerate mutable dir: {e}"))?
    {
        let path = entry.path();
        // Only `{id}.json` records; atomic-write temp siblings
        // (`{id}.json.tmp.{uuid}`) have a different final extension.
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Ok(bytes) = tokio::fs::read(&path).await else {
            continue;
        };
        let Ok(record) = serde_json::from_slice::<MutableShareRecord>(&bytes) else {
            continue;
        };
        // Enumeration match is on the user's OWN key hash — plain equality is
        // fine (nothing secret-dependent leaks to the caller).
        if record.user_key_hash == target {
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            out.push(MutableShareSummary {
                id,
                title: record.notebook.title,
                pushed_at: record.pushed_at,
                cell_count: record.notebook.cells.len(),
            });
        }
    }
    // Newest push first.
    out.sort_by(|a, b| b.pushed_at.cmp(&a.pushed_at));
    Ok(out)
}

/// Convert a notebook into a mutable share. Returns the server-minted id (the
/// `/mutable/{id}` path segment). `user_key` and `notebook_key` are the two
/// device-minted keys; either authorizes future pushes.
#[server]
pub async fn create_mutable_share(
    notebook_json: String,
    user_key: String,
    notebook_key: String,
    // Option, not Vec: an empty Vec is omitted by the URL-encoded server-fn
    // body and would fail deserialization (see `share_notebook`).
    cell_type_tags: Option<Vec<String>>,
) -> Result<String, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    create_mutable_share_core(
        &config.data_dir,
        &config.cache_dir,
        &notebook_json,
        &user_key,
        &notebook_key,
        &cell_type_tags.unwrap_or_default(),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Overwrite a mutable share's content. Requires a key matching the share's
/// user OR notebook key.
#[server]
pub async fn push_mutable(
    id: String,
    key: String,
    notebook_json: String,
    cell_type_tags: Option<Vec<String>>,
) -> Result<(), ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    push_mutable_core(
        &config.data_dir,
        &config.cache_dir,
        &id,
        &key,
        &notebook_json,
        &cell_type_tags.unwrap_or_default(),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Retrieve a mutable share's current notebook, or `None` if the id is unknown.
#[server]
pub async fn get_mutable_notebook(id: String) -> Result<Option<IronpadNotebook>, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    get_mutable_notebook_core(&config.data_dir, &id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Retrieve a mutable share's blob-snapshot manifest, if one exists.
#[server]
pub async fn get_mutable_manifest(
    id: String,
) -> Result<Option<ironpad_common::ShareManifest>, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    get_mutable_manifest_core(&config.data_dir, &id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Check whether a key can edit a mutable share — powers the reader-page
/// "enter your key" rebind flow.
#[server]
pub async fn verify_mutable_key(id: String, key: String) -> Result<bool, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    verify_mutable_key_core(&config.data_dir, &id, &key)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Unpublish a mutable share (authorized delete). Requires a matching key.
#[server]
pub async fn delete_mutable_share(id: String, key: String) -> Result<(), ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    delete_mutable_core(&config.data_dir, &id, &key)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Enumerate the mutable shares owned by a user key (the "my published
/// notebooks" list; also how a fresh machine rediscovers them).
#[server]
pub async fn list_mutable_shares(
    user_key: String,
) -> Result<Vec<ironpad_common::MutableShareSummary>, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    list_mutable_by_user_core(&config.data_dir, &user_key)
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

    // ── compile_cell_core (cache-hit path) ───────────────────────────────

    #[tokio::test]
    async fn compile_cell_core_cache_hit_does_not_scaffold() {
        use crate::compiler::cache::{content_hash, store_blob};
        use crate::compiler::scaffold::merged_deps_contain_rayon;
        use crate::compiler::CompileLocks;
        use ironpad_common::AppConfig;

        let cache = tempfile::tempdir().unwrap();
        let cell_id = "cache-hit-cell";
        let source = "    CellOutput::empty()";
        let cargo_toml = "[dependencies]";

        // Pre-seed the cache with a blob under the exact hash compile_cell_core
        // derives for these inputs, so the request resolves as a cache hit.
        let needs_atomics = merged_deps_contain_rayon(None, cargo_toml);
        let hash = content_hash(
            source,
            cargo_toml,
            &[],
            None,
            None,
            needs_atomics,
            false,
            false,
        );
        let fake_wasm = b"\x00asm\x01\x00\x00\x00cache-hit";
        store_blob(
            cache.path(),
            &hash,
            fake_wasm,
            Some("export function init() {}"),
            &[],
        )
        .unwrap();

        let config = AppConfig {
            data_dir: cache.path().to_path_buf(),
            cache_dir: cache.path().to_path_buf(),
            port: 0,
            // A hit must not touch the scaffold, so this path is deliberately
            // bogus — reaching scaffold would fail before writing anything.
            ironpad_cell_path: cache.path().join("nonexistent-ironpad-cell"),
            compilation_proxy: None,
        };
        let request = CompileRequest {
            notebook_id: "nb".to_string(),
            cell_id: cell_id.to_string(),
            source: source.to_string(),
            cargo_toml: cargo_toml.to_string(),
            previous_cell_types: vec![],
            shared_cargo_toml: None,
            shared_source: None,
            force: false,
            shared_check: None,
        };

        let locks = CompileLocks::default();
        let response = compile_cell_core(&config, &locks, request).await.unwrap();

        assert!(response.cached, "response should be served from cache");
        assert_eq!(
            response.wasm_blob, fake_wasm,
            "cached blob should round-trip"
        );

        // The scaffold writes to {cache}/workspaces/default/{cell_id}; a cache
        // hit must never create it.
        let workspace = cache
            .path()
            .join("workspaces")
            .join("default")
            .join(cell_id);
        assert!(
            !workspace.exists(),
            "cache hit must not scaffold the micro-crate on disk: {} exists",
            workspace.display()
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
    async fn server_fn_core_share_notebook_rejects_over_aggregate_cap() {
        let dir = tempfile::tempdir().unwrap();

        // Cap chosen so the first notebook exactly fills the store; a second
        // *distinct* notebook then pushes the total over the cap.
        let cap = VALID_NOTEBOOK_JSON.len() as u64;
        let h1 = share_notebook_core_capped(dir.path(), VALID_NOTEBOOK_JSON, cap)
            .await
            .expect("first share fits within the cap");

        let err = share_notebook_core_capped(dir.path(), &second_notebook_json(), cap)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("share store full"), "unexpected error: {err}");

        // The rejected distinct notebook must not have been written.
        let count = std::fs::read_dir(dir.path().join("shares"))
            .unwrap()
            .count();
        assert_eq!(count, 1, "only the first share should be on disk");

        // Re-sharing an already-stored notebook overwrites in place and is still
        // allowed even at/over the cap (it adds no bytes).
        let h1_again = share_notebook_core_capped(dir.path(), VALID_NOTEBOOK_JSON, cap)
            .await
            .expect("idempotent re-share is allowed at the cap");
        assert_eq!(h1, h1_again);
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

    // ── snapshot_share_blobs (PRD-0047) ──────────────────────────────

    fn code_cell(id: &str, source: &str) -> ironpad_common::IronpadCell {
        ironpad_common::IronpadCell {
            id: id.into(),
            order: 0,
            label: id.into(),
            cell_type: ironpad_common::CellType::Code,
            source: source.into(),
            cargo_toml: Some("[dependencies]".into()),
            shared: false,
            collapsed: false,
            output_collapsed: false,
            version: 0,
        }
    }

    /// A notebook with no shared source/deps so the expected cache keys stay
    /// simple to recompute in assertions.
    fn snapshot_notebook(cells: Vec<ironpad_common::IronpadCell>) -> IronpadNotebook {
        let mut nb = IronpadNotebook::new("Snapshot");
        nb.shared_cargo_toml = None;
        nb.cells = cells;
        nb
    }

    /// Compute the cache key exactly as `snapshot_share_blobs` will for a
    /// plain cell of `notebook` at `idx`, and seed the cache with a blob.
    fn seed_cache(
        cache_dir: &std::path::Path,
        notebook: &IronpadNotebook,
        tags: &[String],
        idx: usize,
        blob: &[u8],
        glue: Option<&str>,
    ) -> String {
        use crate::compiler::cache::{content_hash, store_blob};
        let cell = &notebook.cells[idx];
        let hash = content_hash(
            &cell.source,
            cell.cargo_toml.as_deref().unwrap_or_default(),
            &tags[..idx],
            notebook.shared_cargo_toml.as_deref(),
            notebook.effective_shared_source().as_deref(),
            false,
            false,
            false,
        );
        store_blob(cache_dir, &hash, blob, glue, &[]).unwrap();
        hash
    }

    #[tokio::test]
    async fn snapshot_writes_blobs_and_manifest_for_cache_hits() {
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let nb = snapshot_notebook(vec![
            code_cell("cell-1", "let a = 1;"),
            code_cell("cell-2", "let b = last.unwrap_or(0) + 1;"),
        ]);
        let tags: Vec<String> = vec!["u32".into(), String::new()];
        let h1 = seed_cache(cache.path(), &nb, &tags, 0, b"wasm-1", None);
        let h2 = seed_cache(cache.path(), &nb, &tags, 1, b"wasm-2", Some("glue()"));

        let count = snapshot_share_blobs(data.path(), cache.path(), &nb, &tags, "aabbccdd00112233")
            .await
            .unwrap();
        assert_eq!(count, 2);

        let blobs = data.path().join("shares").join("blobs");
        assert!(blobs.join(format!("{h1}.wasm")).exists());
        assert!(blobs.join(format!("{h2}.wasm")).exists());
        assert!(!blobs.join(format!("{h1}.js")).exists());
        assert!(blobs.join(format!("{h2}.js")).exists());

        let manifest = get_shared_manifest_core(data.path(), "aabbccdd00112233")
            .await
            .unwrap()
            .expect("manifest should exist");
        assert_eq!(manifest.version, 1);
        assert_eq!(manifest.cells.len(), 2);
        assert_eq!(manifest.cells["cell-1"].blob, h1);
        assert!(!manifest.cells["cell-1"].has_js_glue);
        assert_eq!(manifest.cells["cell-2"].blob, h2);
        assert!(manifest.cells["cell-2"].has_js_glue);
    }

    #[tokio::test]
    async fn snapshot_skips_cache_misses_and_writes_partial_manifest() {
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let nb = snapshot_notebook(vec![
            code_cell("cell-1", "let a = 1;"),
            code_cell("cell-2", "let b = 2;"),
        ]);
        let tags: Vec<String> = vec![String::new(), String::new()];
        seed_cache(cache.path(), &nb, &tags, 0, b"wasm-1", None);
        // cell-2 deliberately not seeded: a cache miss must be skipped, never
        // compiled at share time.

        let count = snapshot_share_blobs(data.path(), cache.path(), &nb, &tags, "aabbccdd00112233")
            .await
            .unwrap();
        assert_eq!(count, 1);

        let manifest = get_shared_manifest_core(data.path(), "aabbccdd00112233")
            .await
            .unwrap()
            .unwrap();
        assert!(manifest.cells.contains_key("cell-1"));
        assert!(!manifest.cells.contains_key("cell-2"));
    }

    #[tokio::test]
    async fn snapshot_rejects_mismatched_tag_vector() {
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let nb = snapshot_notebook(vec![code_cell("cell-1", "let a = 1;")]);

        let err = snapshot_share_blobs(data.path(), cache.path(), &nb, &[], "aabbccdd00112233")
            .await
            .unwrap_err();
        assert!(err.to_string().contains("does not match cell count"));
        assert!(
            get_shared_manifest_core(data.path(), "aabbccdd00112233")
                .await
                .unwrap()
                .is_none(),
            "no manifest on rejected snapshot"
        );
    }

    #[tokio::test]
    async fn snapshot_merges_with_existing_manifest() {
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let nb = snapshot_notebook(vec![code_cell("cell-1", "let a = 1;")]);
        let tags: Vec<String> = vec![String::new()];
        let h1 = seed_cache(cache.path(), &nb, &tags, 0, b"wasm-1", None);

        // Pre-existing manifest from an earlier share of the same notebook,
        // covering a cell this snapshot won't touch. Blobs are immutable, so
        // the old entry must survive the merge.
        let shares = data.path().join("shares");
        tokio::fs::create_dir_all(&shares).await.unwrap();
        let old = ironpad_common::ShareManifest {
            version: 1,
            cells: std::collections::BTreeMap::from([(
                "cell-old".to_string(),
                ironpad_common::ShareBlobEntry {
                    blob: "f".repeat(64),
                    has_js_glue: false,
                },
            )]),
        };
        tokio::fs::write(
            shares.join("aabbccdd00112233.manifest.json"),
            serde_json::to_vec(&old).unwrap(),
        )
        .await
        .unwrap();

        let count = snapshot_share_blobs(data.path(), cache.path(), &nb, &tags, "aabbccdd00112233")
            .await
            .unwrap();
        assert_eq!(count, 2, "old entry + fresh entry");

        let manifest = get_shared_manifest_core(data.path(), "aabbccdd00112233")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(manifest.cells["cell-1"].blob, h1);
        assert!(manifest.cells.contains_key("cell-old"));
    }

    #[tokio::test]
    async fn snapshot_respects_blob_dir_cap() {
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let nb = snapshot_notebook(vec![code_cell("cell-1", "let a = 1;")]);
        let tags: Vec<String> = vec![String::new()];
        seed_cache(cache.path(), &nb, &tags, 0, b"wasm-1", None);

        // Pre-fill the blobs dir past a tiny cap.
        let blobs = data.path().join("shares").join("blobs");
        tokio::fs::create_dir_all(&blobs).await.unwrap();
        tokio::fs::write(
            blobs.join(format!("{}.wasm", "e".repeat(64))),
            vec![0u8; 64],
        )
        .await
        .unwrap();

        let err = snapshot_share_blobs_capped(
            data.path(),
            cache.path(),
            &nb,
            &tags,
            "aabbccdd00112233",
            32,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("share blob store full"));
    }

    #[tokio::test]
    async fn get_shared_manifest_core_rejects_path_traversal() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["../etc/passwd", "foo/bar", ".."] {
            assert!(
                get_shared_manifest_core(dir.path(), bad).await.is_err(),
                "should reject traversal: {bad}"
            );
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
    async fn server_fn_core_get_public_notebook_accepts_extensionless_name() {
        // The canonical route is extension-less (PRD-0048); legacy links and
        // embed specs carry .ironpad. Both must resolve to the same file.
        let dir = tempfile::tempdir().unwrap();
        let nb_dir = dir.path().join("notebooks");
        std::fs::create_dir_all(&nb_dir).unwrap();
        std::fs::write(nb_dir.join("demo.ironpad"), VALID_NOTEBOOK_JSON).unwrap();

        let nb = get_public_notebook_core(dir.path(), "demo").await.unwrap();
        assert_eq!(nb.title, "Test Notebook");

        // A non-notebook name resolves to {name}.ironpad and misses — the
        // endpoint still serves only notebook files.
        std::fs::write(nb_dir.join("evil.json"), b"{}").unwrap();
        assert!(get_public_notebook_core(dir.path(), "evil.json")
            .await
            .is_err());
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

    // ── Mutable shares (PRD-0049) ────────────────────────────────────

    #[test]
    fn hash_mutable_key_is_domain_separated_and_deterministic() {
        let a = hash_mutable_key("deadbeef");
        assert_eq!(a, hash_mutable_key("deadbeef"), "same key → same hash");
        assert_eq!(a.len(), 64);
        assert_ne!(hash_mutable_key("deadbeef"), hash_mutable_key("deadbeee"));
        // Domain separation: NOT a bare blake3 of the key bytes.
        assert_ne!(a, blake3::hash("deadbeef".as_bytes()).to_hex().to_string());
        // Constant-time matcher agrees with the hash.
        assert!(mutable_key_matches("deadbeef", &a));
        assert!(!mutable_key_matches("wrong", &a));
        assert!(!mutable_key_matches("deadbeef", "not-a-64-char-hash"));
    }

    fn zero_cell_notebook_json(title: &str) -> String {
        let mut nb = IronpadNotebook::new(title);
        nb.cells = vec![];
        serde_json::to_string(&nb).unwrap()
    }

    #[tokio::test]
    async fn mutable_create_and_get_round_trip() {
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let id = create_mutable_share_core(
            data.path(),
            cache.path(),
            VALID_NOTEBOOK_JSON,
            "userkey",
            "nbkey",
            &[String::new()],
        )
        .await
        .unwrap();
        assert_eq!(id.len(), 16, "server mints a 16-hex id");

        let nb = get_mutable_notebook_core(data.path(), &id)
            .await
            .unwrap()
            .expect("share exists");
        assert_eq!(nb.title, "Test Notebook");

        // Unknown id resolves to None, not an error.
        assert!(get_mutable_notebook_core(data.path(), "0000000000000000")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn mutable_push_accepts_either_key_and_rejects_wrong() {
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let id = create_mutable_share_core(
            data.path(),
            cache.path(),
            VALID_NOTEBOOK_JSON,
            "userkey",
            "nbkey",
            &[String::new()],
        )
        .await
        .unwrap();

        // User key authorizes.
        push_mutable_core(
            data.path(),
            cache.path(),
            &id,
            "userkey",
            &zero_cell_notebook_json("Via user key"),
            &[],
        )
        .await
        .expect("user key authorizes");
        assert_eq!(
            get_mutable_notebook_core(data.path(), &id)
                .await
                .unwrap()
                .unwrap()
                .title,
            "Via user key"
        );

        // Notebook key authorizes.
        push_mutable_core(
            data.path(),
            cache.path(),
            &id,
            "nbkey",
            &zero_cell_notebook_json("Via notebook key"),
            &[],
        )
        .await
        .expect("notebook key authorizes");

        // Wrong key is rejected and leaves content untouched.
        let err = push_mutable_core(
            data.path(),
            cache.path(),
            &id,
            "wrong",
            &zero_cell_notebook_json("Should not land"),
            &[],
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("unauthorized"), "unexpected error: {err}");
        assert_eq!(
            get_mutable_notebook_core(data.path(), &id)
                .await
                .unwrap()
                .unwrap()
                .title,
            "Via notebook key"
        );

        // Push to an unknown id fails.
        assert!(push_mutable_core(
            data.path(),
            cache.path(),
            "0000000000000000",
            "userkey",
            &zero_cell_notebook_json("x"),
            &[],
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn mutable_verify_key_and_delete() {
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let id = create_mutable_share_core(
            data.path(),
            cache.path(),
            VALID_NOTEBOOK_JSON,
            "userkey",
            "nbkey",
            &[String::new()],
        )
        .await
        .unwrap();

        assert!(verify_mutable_key_core(data.path(), &id, "userkey")
            .await
            .unwrap());
        assert!(verify_mutable_key_core(data.path(), &id, "nbkey")
            .await
            .unwrap());
        assert!(!verify_mutable_key_core(data.path(), &id, "wrong")
            .await
            .unwrap());
        // Unknown id → false, not error (powers the rebind gate).
        assert!(!verify_mutable_key_core(data.path(), "0000000000000000", "userkey")
            .await
            .unwrap());

        // Wrong key can't unpublish.
        assert!(delete_mutable_core(data.path(), &id, "wrong").await.is_err());
        assert!(get_mutable_notebook_core(data.path(), &id)
            .await
            .unwrap()
            .is_some());
        // Right key unpublishes.
        delete_mutable_core(data.path(), &id, "userkey")
            .await
            .unwrap();
        assert!(get_mutable_notebook_core(data.path(), &id)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn mutable_list_by_user_matches_only_owner() {
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        create_mutable_share_core(
            data.path(),
            cache.path(),
            VALID_NOTEBOOK_JSON,
            "alice",
            "n1",
            &[String::new()],
        )
        .await
        .unwrap();
        create_mutable_share_core(
            data.path(),
            cache.path(),
            &zero_cell_notebook_json("Alice Two"),
            "alice",
            "n2",
            &[],
        )
        .await
        .unwrap();
        create_mutable_share_core(
            data.path(),
            cache.path(),
            VALID_NOTEBOOK_JSON,
            "bob",
            "n3",
            &[String::new()],
        )
        .await
        .unwrap();

        assert_eq!(
            list_mutable_by_user_core(data.path(), "alice")
                .await
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            list_mutable_by_user_core(data.path(), "bob")
                .await
                .unwrap()
                .len(),
            1
        );
        // Empty key never enumerates.
        assert!(list_mutable_by_user_core(data.path(), "")
            .await
            .unwrap()
            .is_empty());
        assert!(list_mutable_by_user_core(data.path(), "carol")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn mutable_manifest_absent_on_cold_cache_and_create_validates() {
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        // Cold cache → no snapshot → live-compile share (manifest None).
        let id = create_mutable_share_core(
            data.path(),
            cache.path(),
            VALID_NOTEBOOK_JSON,
            "u",
            "n",
            &[String::new()],
        )
        .await
        .unwrap();
        assert!(get_mutable_manifest_core(data.path(), &id)
            .await
            .unwrap()
            .is_none());

        // Both keys are required.
        assert!(create_mutable_share_core(
            data.path(),
            cache.path(),
            VALID_NOTEBOOK_JSON,
            "",
            "n",
            &[String::new()],
        )
        .await
        .is_err());
        assert!(create_mutable_share_core(
            data.path(),
            cache.path(),
            VALID_NOTEBOOK_JSON,
            "u",
            "",
            &[String::new()],
        )
        .await
        .is_err());

        // Traversal ids are rejected by the read path.
        for bad in ["../etc/passwd", "foo/bar", ".."] {
            assert!(
                get_mutable_notebook_core(data.path(), bad).await.is_err(),
                "should reject traversal: {bad}"
            );
        }
    }
}
