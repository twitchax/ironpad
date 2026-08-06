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
    let admission = expect_context::<crate::compiler::admission::BuildAdmission>();
    // Client identity for the build rate limiter (PRD-0052). Header-derived so
    // it survives the Fly edge; a missing map (non-HTTP test harness) shares
    // the "local" bucket.
    let client_ip = leptos_axum::extract::<http::HeaderMap>().await.map_or_else(
        |_| "local".to_string(),
        |headers| crate::compiler::admission::client_ip(&headers),
    );
    compile_cell_core(&config, &compile_locks, &admission, &client_ip, request).await
}

/// Server-side compilation pipeline behind [`compile_cell`].
///
/// Split out from the `#[server]` wrapper so it takes an explicit [`AppConfig`]
/// and [`CompileLocks`] instead of Leptos context, which keeps it unit-testable
/// (mirrors the `*_core` helpers for the other server functions).
#[cfg(feature = "ssr")]
#[tracing::instrument(
    name = "compile_cell",
    level = "info",
    skip_all,
    fields(cell_id = %request.cell_id, force = request.force, cache = tracing::field::Empty)
)]
async fn compile_cell_core(
    config: &ironpad_common::AppConfig,
    compile_locks: &crate::compiler::CompileLocks,
    admission: &crate::compiler::admission::BuildAdmission,
    client_ip: &str,
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
    // contend — they scaffold into distinct directories. The span makes queue
    // time behind another compile of this cell visible in traces.
    let _compile_guard = tracing::Instrument::instrument(
        compile_locks.acquire(&request.cell_id),
        tracing::info_span!("compile_lock_wait"),
    )
    .await;

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
            tracing::Span::current().record("cache", "hit");
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
        tracing::Span::current().record("cache", "bypassed");
        tracing::info!(cell_id = %request.cell_id, "force recompile requested — skipping cache");
    } else {
        tracing::Span::current().record("cache", "miss");
        tracing::info!(cell_id = %request.cell_id, "cache miss — compiling");
    }

    // Admission (PRD-0052): this request is about to spawn cargo. Sits AFTER
    // the cache lookup on purpose — hits are free, so warmed notebooks and
    // repeat runs never touch the rate limiter or occupy a build slot. The
    // permit is held for the whole miss path (scaffold → build → wasm-opt →
    // store), which is the actual footprint on the machine.
    let _build_permit = admission
        .admit_compile(client_ip)
        .await
        .map_err(|denied| ServerFnError::new(denied.to_string()))?;

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
    let admission = expect_context::<crate::compiler::admission::BuildAdmission>();
    check_cell_core(&config, &compile_locks, &admission, request).await
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
#[tracing::instrument(
    name = "check_cell",
    level = "info",
    skip_all,
    fields(cell_id = %request.cell_id, status = tracing::field::Empty)
)]
async fn check_cell_core(
    config: &ironpad_common::AppConfig,
    compile_locks: &crate::compiler::CompileLocks,
    admission: &crate::compiler::admission::BuildAdmission,
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
        tracing::Span::current().record("status", "skipped");
        return Ok(CheckResponse {
            status: CheckStatus::Skipped,
            diagnostics: vec![],
        });
    };

    // Admission (PRD-0052): shed load instead of queueing — Skipped is the
    // status the client already retries after the next quiet period, so a
    // busy server degrades to fewer live markers, never to blocked typing.
    let Some(_check_permit) = admission.try_admit_check() else {
        tracing::Span::current().record("status", "skipped");
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
        Ok(CheckResult::Ok) => {
            tracing::Span::current().record("status", "clean");
            Ok(CheckResponse {
                status: CheckStatus::Clean,
                diagnostics: vec![],
            })
        }
        Ok(CheckResult::Failure { stdout, .. }) => {
            tracing::Span::current().record("status", "errors");
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
            tracing::Span::current().record("status", "timed_out");
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
#[tracing::instrument(name = "list_public_notebooks", level = "info", skip_all)]
pub async fn list_public_notebooks_core(
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

/// Process-lifetime cache behind [`list_public_notebooks_cached`].
#[cfg(feature = "ssr")]
static PUBLIC_NOTEBOOKS: tokio::sync::OnceCell<Vec<PublicNotebookSummary>> =
    tokio::sync::OnceCell::const_new();

/// [`list_public_notebooks_core`], computed once per process.
///
/// The public notebooks are static files baked into `{site_root}/notebooks` at
/// build time, so the listing cannot change while the server runs; a deploy is
/// what replaces them, and that restarts the process. Enumerating them means
/// reading and JSON-parsing every one, which was happening on each home-page
/// load, each `/sitemap.xml`, and each `/og/ironpad.png` — several megabytes of
/// parsing per request for a value that is fixed at startup.
///
/// The core function stays uncached so its unit tests can each point at their
/// own `TempDir`. That is also why the `site_root` of the first caller wins
/// here: there is exactly one for the lifetime of a real server, and baking
/// that assumption into the shared cache instead of the pure function keeps the
/// tested code honest.
///
/// A read failure is cached as an empty list rather than retried. Missing
/// `notebooks/` is a build problem, not a transient one, and it already
/// degrades to an empty list one layer down.
#[cfg(feature = "ssr")]
pub async fn list_public_notebooks_cached(
    site_root: &std::path::Path,
) -> &'static [PublicNotebookSummary] {
    PUBLIC_NOTEBOOKS
        .get_or_init(|| async {
            list_public_notebooks_core(site_root)
                .await
                .unwrap_or_default()
        })
        .await
}

/// Lists all available public notebooks by enumerating `*.ironpad` files at runtime.
#[server]
pub async fn list_public_notebooks() -> Result<Vec<PublicNotebookSummary>, ServerFnError> {
    let leptos_options = expect_context::<LeptosOptions>();
    let site_root = leptos_options.site_root.as_ref();
    Ok(
        list_public_notebooks_cached(std::path::Path::new(site_root))
            .await
            .to_vec(),
    )
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
#[tracing::instrument(name = "get_public_notebook", level = "info", skip_all, fields(filename = %filename))]
pub async fn get_public_notebook_core(
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
#[tracing::instrument(name = "dir_size_scan", level = "info", skip_all, fields(dir = %dir.display()))]
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
#[tracing::instrument(name = "share_notebook", level = "info", skip_all, fields(bytes = notebook_json.len()))]
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

    // Atomic write (temp + rename), not a truncating fs::write: re-sharing an
    // existing hash overwrites in place, so a plain write truncates the live
    // file to zero first and a concurrent reader of that hash could read a
    // partial/empty file ("invalid shared notebook") for a share that is
    // perfectly valid. The blobs and manifest already use this helper.
    atomic_write_async(&path, notebook_json.as_bytes())
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
#[tracing::instrument(
    name = "snapshot_cell_blobs",
    level = "info",
    skip_all,
    fields(cells = notebook.cells.len(), snapshotted = tracing::field::Empty)
)]
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

    tracing::Span::current().record("snapshotted", fresh_entries.len());
    Ok(fresh_entries)
}

/// Core snapshot logic with an explicit blob-dir cap (so tests can exercise
/// the cap without writing gigabytes). See [`snapshot_share_blobs`].
#[cfg(feature = "ssr")]
#[tracing::instrument(name = "snapshot_share_blobs", level = "info", skip_all, fields(share_hash = %share_hash))]
async fn snapshot_share_blobs_capped(
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
    notebook: &IronpadNotebook,
    cell_type_tags: &[String],
    share_hash: &str,
    max_total_bytes: u64,
) -> anyhow::Result<usize> {
    use ironpad_common::ShareManifest;

    let fresh_entries = write_cell_blobs_capped(
        data_dir,
        cache_dir,
        notebook,
        cell_type_tags,
        max_total_bytes,
    )
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
    // Insert-if-ABSENT, never overwrite: `share_notebook` is unauthenticated
    // and re-sharing an existing hash is idempotent, so an attacker who
    // reproduces a share's JSON could otherwise re-POST it with different
    // (caller-supplied, unverified) cell_type_tags, recompute a different
    // blob key, and REPLACE the manifest entry — changing which WASM every
    // existing viewer of that immutable link runs. The first sharer's entry
    // for a cell id is frozen; a legitimate re-share of identical content
    // with identical tags recomputes the same entry (a no-op), and a cell
    // that missed the cache originally can still be filled later.
    for (cell_id, entry) in fresh_entries {
        manifest.cells.entry(cell_id).or_insert(entry);
    }

    if !manifest.cells.is_empty() {
        atomic_write_async(&manifest_path, &serde_json::to_vec(&manifest)?).await?;
    }
    Ok(manifest.cells.len())
}

#[cfg(feature = "ssr")]
#[tracing::instrument(name = "get_shared_manifest", level = "info", skip_all, fields(hash = %hash))]
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
#[tracing::instrument(name = "get_shared_notebook", level = "info", skip_all, fields(hash = %hash))]
pub async fn get_shared_notebook_core(
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

// ── Mutable shares (PRD-0049; accounts-backed since PRD-0053) ────────────────
//
// A third storage class: `Share Mutable` CONVERTS a private notebook into a
// server-backed one at `/mutable/{id}`. Anyone with the link reads it; the
// owner overwrites it with an explicit Push. Ownership is an RBAC OWNER grant
// tied to the owner's GitHub account (PRD-0053): create requires a signed-in
// session, and push/delete require the grant. The PRD-0049 two-key mechanism
// is gone — GitHub owns identity now.
//
// Blobs reuse the immutable content-addressed `shares/blobs/` store (served by
// the `/share-blobs/` route), so mutable readers never trigger compiles for
// snapshotted cells. Only the id→content resolve is mutable (and served
// no-cache by the server's default policy). Each push RE-snapshots and REPLACES
// the embedded manifest, versus the merge semantics of immutable shares.

/// Aggregate cap on stored mutable-share notebook JSON, drafts included
/// (blobs live in the shared blob store under its own cap). Beyond it,
/// creating a *new* mutable share and saving a draft are both refused;
/// pushing an existing draft overwrites published in place and is always
/// allowed. The cores take the cap as a parameter so tests can exercise the
/// refusal without generating half a gigabyte of JSON.
#[cfg(feature = "ssr")]
const MAX_TOTAL_MUTABLE_BYTES: u64 = 512 * 1024 * 1024;

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

/// Serialize a manifest for the DB record.
#[cfg(feature = "ssr")]
fn manifest_to_json(manifest: Option<ironpad_common::ShareManifest>) -> Option<String> {
    manifest.and_then(|m| serde_json::to_string(&m).ok())
}

#[cfg(feature = "ssr")]
#[tracing::instrument(name = "create_mutable_share", level = "info", skip_all, fields(bytes = notebook_json.len()))]
pub(crate) async fn create_mutable_share_core(
    db: &crate::db::Db,
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
    owner_github_id: &str,
    notebook_json: &str,
    cell_type_tags: &[String],
    max_total_bytes: u64,
) -> anyhow::Result<String> {
    if notebook_json.len() > MAX_SHARE_BYTES {
        anyhow::bail!(
            "notebook too large: {} bytes (max {MAX_SHARE_BYTES})",
            notebook_json.len()
        );
    }
    let notebook: IronpadNotebook = serde_json::from_str(notebook_json)
        .map_err(|e| anyhow::anyhow!("invalid notebook JSON: {e}"))?;

    // Aggregate cap for a NEW share (minor TOCTOU is fine; the cap bounds
    // steady-state storage, it is not a hard quota).
    let existing_total = db.total_mutable_bytes().await?;
    if existing_total.saturating_add(notebook_json.len() as u64) > max_total_bytes {
        anyhow::bail!(
            "mutable share store full: {existing_total} bytes stored, cap is {max_total_bytes}"
        );
    }

    // Mint an id that isn't already taken. Collisions are astronomically
    // unlikely at 64 bits; the loop is a belt-and-suspenders guard.
    let mut minted = None;
    for _ in 0..8 {
        let candidate = random_mutable_id();
        if !db.mutable_share_exists(&candidate).await? {
            minted = Some(candidate);
            break;
        }
    }
    let id = minted.ok_or_else(|| anyhow::anyhow!("failed to mint a unique mutable id"))?;

    let manifest = snapshot_mutable_manifest(data_dir, cache_dir, &notebook, cell_type_tags).await;
    db.create_mutable_share(
        &id,
        owner_github_id,
        notebook_json,
        manifest_to_json(manifest),
    )
    .await?;

    tracing::info!(id = %id, "mutable share created");
    Ok(id)
}

/// Autosave the owner's draft (PRD-0054). Cheap by design: a size-capped,
/// validated JSON write with no blob work — this runs on the editor's
/// debounce, not on an editorial act.
#[cfg(feature = "ssr")]
#[tracing::instrument(name = "save_mutable_draft", level = "debug", skip_all, fields(id = %id, bytes = notebook_json.len()))]
pub(crate) async fn save_mutable_draft_core(
    db: &crate::db::Db,
    github_id: &str,
    id: &str,
    notebook_json: &str,
    max_total_bytes: u64,
) -> anyhow::Result<()> {
    if notebook_json.len() > MAX_SHARE_BYTES {
        anyhow::bail!(
            "notebook too large: {} bytes (max {MAX_SHARE_BYTES})",
            notebook_json.len()
        );
    }
    let _: IronpadNotebook = serde_json::from_str(notebook_json)
        .map_err(|e| anyhow::anyhow!("invalid notebook JSON: {e}"))?;
    if !db.user_owns_share(github_id, id).await? {
        anyhow::bail!("unauthorized: you do not own this share");
    }
    // Aggregate cap: drafts count (they are the one write path an owner can
    // drive at will; uncounted, N tiny shares each autosaving a 4 MiB draft
    // would grow storage unboundedly). Conservative by up to the size of the
    // draft being replaced — the total still includes it — which at worst
    // refuses one draft-size early near the cap.
    let existing_total = db.total_mutable_bytes().await?;
    if existing_total.saturating_add(notebook_json.len() as u64) > max_total_bytes {
        anyhow::bail!(
            "mutable share store full: {existing_total} bytes stored, cap is {max_total_bytes}"
        );
    }
    db.save_draft(id, notebook_json).await
}

/// Push (PRD-0054): promote the server draft to published. Uploads nothing —
/// the draft is already on the server; this snapshots blobs from it and
/// swaps it in. Returns false when there was no draft to promote.
#[cfg(feature = "ssr")]
#[tracing::instrument(name = "push_mutable", level = "info", skip_all, fields(id = %id))]
pub(crate) async fn push_mutable_core(
    db: &crate::db::Db,
    data_dir: &std::path::Path,
    cache_dir: &std::path::Path,
    github_id: &str,
    id: &str,
    cell_type_tags: &[String],
) -> anyhow::Result<bool> {
    // The OWNER grant gates the push, before any blob work. A share that
    // doesn't exist reads the same as one you don't own — deliberately.
    if !db.user_owns_share(github_id, id).await? {
        anyhow::bail!("unauthorized: you do not own this share");
    }
    let edit = db
        .get_share_for_edit(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("mutable share not found"))?;
    if !edit.dirty {
        return Ok(false);
    }

    let notebook: IronpadNotebook = serde_json::from_str(&edit.notebook_json)
        .map_err(|e| anyhow::anyhow!("invalid stored draft: {e}"))?;
    let manifest = snapshot_mutable_manifest(data_dir, cache_dir, &notebook, cell_type_tags).await;
    // A concurrent autosave between the read and this promote is fine: the
    // promote takes whatever draft is newest (last-write-wins, PRD-0054);
    // the recorded byte size then lags by the same one autosave.
    db.promote_draft(
        id,
        manifest_to_json(manifest),
        edit.notebook_json.len() as u64,
    )
    .await?;

    tracing::info!(id = %id, "mutable share pushed");
    Ok(true)
}

/// Fetch just the notebook of a mutable share for ANONYMOUS surfaces — the
/// OG-card handler, the oEmbed provider, and tests. A private share returns
/// `None` here unconditionally (PRD-0061): these surfaces serve crawlers and
/// unfurlers, which never hold a session, and a title in an OG card is
/// already a leak. The reader page uses [`get_mutable_notebook`], which
/// resolves the caller's session.
#[cfg(feature = "ssr")]
pub async fn get_mutable_notebook_core(
    db: &crate::db::Db,
    id: &str,
) -> anyhow::Result<Option<IronpadNotebook>> {
    let Some(row) = db.get_mutable_share(id).await? else {
        return Ok(None);
    };
    if row.private {
        return Ok(None);
    }
    serde_json::from_str(&row.notebook_json)
        .map(Some)
        .map_err(|e| anyhow::anyhow!("invalid stored notebook: {e}"))
}

#[cfg(feature = "ssr")]
pub(crate) async fn delete_mutable_core(
    db: &crate::db::Db,
    github_id: &str,
    id: &str,
) -> anyhow::Result<()> {
    if !db.user_owns_share(github_id, id).await? {
        anyhow::bail!("unauthorized: you do not own this share");
    }
    // The content-addressed blobs are shared across every share that
    // references them and bounded by the blob-store cap, so (like immutable
    // unshares) they are left in place; only the record + grants go.
    db.delete_mutable_share(id).await?;
    tracing::info!(id = %id, "mutable share unpublished");
    Ok(())
}

#[cfg(feature = "ssr")]
#[tracing::instrument(name = "list_mutable_shares", level = "info", skip_all)]
pub(crate) async fn list_mutable_core(
    db: &crate::db::Db,
    github_id: &str,
) -> anyhow::Result<Vec<ironpad_common::MutableShareSummary>> {
    let rows = db.list_shares_owned_by(github_id).await?;
    Ok(rows
        .into_iter()
        .filter_map(|row| {
            let nb: IronpadNotebook = serde_json::from_str(&row.notebook_json).ok()?;
            Some(ironpad_common::MutableShareSummary {
                id: row.id,
                title: nb.title,
                pushed_at: row.pushed_at,
                cell_count: nb.cells.len(),
            })
        })
        .collect())
}

/// Resolve the signed-in user or fail the (write) server fn with a clear,
/// toast-ready message.
#[cfg(feature = "ssr")]
async fn require_login(db: &crate::db::Db) -> Result<crate::db::AuthUser, ServerFnError> {
    crate::auth::current_user(db)
        .await
        .ok_or_else(|| ServerFnError::new("sign in with GitHub to publish"))
}

/// Convert a notebook into a mutable share owned by the signed-in user.
/// Returns the server-minted id (the `/mutable/{id}` path segment).
#[server]
pub async fn create_mutable_share(
    notebook_json: String,
    // Option, not Vec: an empty Vec is omitted by the URL-encoded server-fn
    // body and would fail deserialization (see `share_notebook`).
    cell_type_tags: Option<Vec<String>>,
) -> Result<String, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let db = expect_context::<crate::db::Db>();
    let user = require_login(&db).await?;
    create_mutable_share_core(
        &db,
        &config.data_dir,
        &config.cache_dir,
        &user.github_id,
        &notebook_json,
        &cell_type_tags.unwrap_or_default(),
        MAX_TOTAL_MUTABLE_BYTES,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Push: promote the server draft to published (PRD-0054). Requires the
/// OWNER grant. Returns false when the draft was already clean.
#[server]
pub async fn push_mutable(
    id: String,
    cell_type_tags: Option<Vec<String>>,
) -> Result<bool, ServerFnError> {
    use ironpad_common::AppConfig;

    let config = expect_context::<AppConfig>();
    let db = expect_context::<crate::db::Db>();
    let user = require_login(&db).await?;
    push_mutable_core(
        &db,
        &config.data_dir,
        &config.cache_dir,
        &user.github_id,
        &id,
        &cell_type_tags.unwrap_or_default(),
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Autosave the owner's draft (PRD-0054). Readers are unaffected until Push.
#[server]
pub async fn save_mutable_draft(id: String, notebook_json: String) -> Result<(), ServerFnError> {
    let db = expect_context::<crate::db::Db>();
    let user = require_login(&db).await?;
    save_mutable_draft_core(
        &db,
        &user.github_id,
        &id,
        &notebook_json,
        MAX_TOTAL_MUTABLE_BYTES,
    )
    .await
    .map_err(|e| ServerFnError::new(e.to_string()))
}

/// The owner's editing payload: the draft (else published) plus whether they
/// differ. Errs for non-owners — the reader path is `get_mutable_notebook`.
#[server]
pub async fn get_mutable_for_edit(
    id: String,
) -> Result<Option<ironpad_common::MutableEditResponse>, ServerFnError> {
    let db = expect_context::<crate::db::Db>();
    require_share_owner(&db, &id).await?;
    let Some(edit) = db
        .get_share_for_edit(&id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
    else {
        return Ok(None);
    };
    let notebook: IronpadNotebook = serde_json::from_str(&edit.notebook_json)
        .map_err(|e| ServerFnError::new(format!("invalid stored draft: {e}")))?;
    Ok(Some(ironpad_common::MutableEditResponse {
        notebook,
        dirty: edit.dirty,
        private: edit.private,
    }))
}

/// Discard the draft: revert the editor to the published copy (PRD-0054).
#[server]
pub async fn discard_mutable_draft(id: String) -> Result<(), ServerFnError> {
    let db = expect_context::<crate::db::Db>();
    require_share_owner(&db, &id).await?;
    db.discard_draft(&id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// The ONE private-share read predicate (PRD-0061): may `viewer` read a
/// share whose row carries `private`? Public shares admit everyone; private
/// shares admit sessions holding an OWNER or READ grant, never anonymous.
/// Consumed by BOTH [`mutable_access_core`] and
/// [`mutable_manifest_access_core`] — the manifest is the hash list that
/// unlocks the deliberately ungated `/share-blobs/` route, so a policy
/// change must not be able to land in one gate and not the other.
#[cfg(feature = "ssr")]
async fn private_share_readable(
    db: &crate::db::Db,
    id: &str,
    private: bool,
    viewer: Option<&crate::db::AuthUser>,
) -> anyhow::Result<bool> {
    if !private {
        return Ok(true);
    }
    match viewer {
        Some(v) => db.user_can_read_share(&v.github_id, id).await,
        None => Ok(false),
    }
}

/// Resolve a viewer's access to a mutable share (PRD-0061): the notebook
/// with attribution, or the explicit `Private` / `NotFound` outcome.
/// Content never rides the denied arms. `viewer` is the caller's resolved
/// session, `None` for anonymous.
#[cfg(feature = "ssr")]
pub async fn mutable_access_core(
    db: &crate::db::Db,
    id: &str,
    viewer: Option<&crate::db::AuthUser>,
) -> anyhow::Result<ironpad_common::MutableNotebookAccess> {
    use ironpad_common::{MutableNotebookAccess, MutableNotebookResponse, ShareOwner};

    let Some(row) = db.get_mutable_share(id).await? else {
        return Ok(MutableNotebookAccess::NotFound);
    };

    if !private_share_readable(db, id, row.private, viewer).await? {
        return Ok(MutableNotebookAccess::Private {
            signed_in: viewer.is_some(),
        });
    }

    let notebook: IronpadNotebook = serde_json::from_str(&row.notebook_json)
        .map_err(|e| anyhow::anyhow!("invalid stored notebook: {e}"))?;
    let is_owner = matches!(
        (viewer, &row.owner),
        (Some(v), Some(o)) if v.github_id == o.github_id
    );
    Ok(MutableNotebookAccess::Found(Box::new(
        MutableNotebookResponse {
            notebook,
            owner: row.owner.map(|o| ShareOwner {
                login: o.login,
                avatar_url: o.avatar_url,
            }),
            is_owner,
        },
    )))
}

/// Retrieve a mutable share's current notebook plus owner attribution and
/// whether the CALLER's session owns it — or the explicit `Private` /
/// `NotFound` outcome (PRD-0061).
#[server]
pub async fn get_mutable_notebook(
    id: String,
) -> Result<ironpad_common::MutableNotebookAccess, ServerFnError> {
    let db = expect_context::<crate::db::Db>();
    let viewer = crate::auth::current_user(&db).await;
    mutable_access_core(&db, &id, viewer.as_ref())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// A mutable share's blob-snapshot manifest, gated like the notebook: a
/// private share's manifest is withheld from non-grantees — the manifest is
/// the hash list, and the content-addressed blob route is only reachable
/// with hashes (PRD-0061).
#[cfg(feature = "ssr")]
pub(crate) async fn mutable_manifest_access_core(
    db: &crate::db::Db,
    id: &str,
    viewer: Option<&crate::db::AuthUser>,
) -> anyhow::Result<Option<ironpad_common::ShareManifest>> {
    let Some(row) = db.get_mutable_share(id).await? else {
        return Ok(None);
    };
    if !private_share_readable(db, id, row.private, viewer).await? {
        return Ok(None);
    }
    Ok(row
        .manifest_json
        .and_then(|json| serde_json::from_str(&json).ok()))
}

/// Retrieve a mutable share's blob-snapshot manifest, if one exists and the
/// caller may view it (PRD-0061).
#[server]
pub async fn get_mutable_manifest(
    id: String,
) -> Result<Option<ironpad_common::ShareManifest>, ServerFnError> {
    let db = expect_context::<crate::db::Db>();
    let viewer = crate::auth::current_user(&db).await;
    mutable_manifest_access_core(&db, &id, viewer.as_ref())
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Unpublish a mutable share. Requires the OWNER grant.
#[server]
pub async fn delete_mutable_share(id: String) -> Result<(), ServerFnError> {
    let db = expect_context::<crate::db::Db>();
    let user = require_login(&db).await?;
    delete_mutable_core(&db, &user.github_id, &id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Require the caller to hold the OWNER grant on `id`, resolving them first.
#[cfg(feature = "ssr")]
async fn require_share_owner(
    db: &crate::db::Db,
    id: &str,
) -> Result<crate::db::AuthUser, ServerFnError> {
    let user = require_login(db).await?;
    if !db
        .user_owns_share(&user.github_id, id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
    {
        return Err(ServerFnError::new(
            "unauthorized: you do not own this share",
        ));
    }
    Ok(user)
}

/// Flip a share's privacy flag (PRD-0061). Owner only.
#[server]
pub async fn set_mutable_private(id: String, private: bool) -> Result<(), ServerFnError> {
    let db = expect_context::<crate::db::Db>();
    require_share_owner(&db, &id).await?;
    db.set_share_private(&id, private)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// Grant READ on a private share to a GitHub login (PRD-0061). Owner only.
/// The target must have signed in to ironpad at least once: grants link to
/// user records, and a bare login cannot be resolved to a GitHub id
/// server-side without calling GitHub.
#[server]
pub async fn grant_mutable_read(
    id: String,
    login: String,
) -> Result<ironpad_common::ShareGrant, ServerFnError> {
    let db = expect_context::<crate::db::Db>();
    let owner = require_share_owner(&db, &id).await?;
    let login = login.trim().trim_start_matches('@').to_string();
    if login.is_empty() {
        return Err(ServerFnError::new("enter a GitHub username"));
    }
    let Some(target) = db
        .find_user_by_login(&login)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
    else {
        return Err(ServerFnError::new(format!(
            "no ironpad user named \"{login}\" — they need to sign in to ironpad once before you can grant them access"
        )));
    };
    if target.github_id == owner.github_id {
        return Err(ServerFnError::new("you already own this share"));
    }
    db.grant_read(&id, &target.github_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    Ok(ironpad_common::ShareGrant {
        github_id: target.github_id,
        login: target.login,
        avatar_url: target.avatar_url,
    })
}

/// Revoke a READ grant (PRD-0061). Owner only.
#[server]
pub async fn revoke_mutable_read(id: String, github_id: String) -> Result<(), ServerFnError> {
    let db = expect_context::<crate::db::Db>();
    require_share_owner(&db, &id).await?;
    db.revoke_read(&id, &github_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

/// The owner's access view — privacy flag plus READ grants — in one round
/// trip for the Access UI (PRD-0061). Owner only.
#[server]
pub async fn get_mutable_access(id: String) -> Result<ironpad_common::ShareAccess, ServerFnError> {
    let db = expect_context::<crate::db::Db>();
    require_share_owner(&db, &id).await?;
    let row = db
        .get_mutable_share(&id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .ok_or_else(|| ServerFnError::new("share not found"))?;
    let grants = db
        .list_read_grants(&id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?
        .into_iter()
        .map(|u| ironpad_common::ShareGrant {
            github_id: u.github_id,
            login: u.login,
            avatar_url: u.avatar_url,
        })
        .collect();
    Ok(ironpad_common::ShareAccess {
        private: row.private,
        grants,
    })
}

/// Enumerate the mutable shares owned by the signed-in user (the home-page
/// "Published" listing). Anonymous callers get an empty list, not an error.
#[server]
pub async fn list_mutable_shares() -> Result<Vec<ironpad_common::MutableShareSummary>, ServerFnError>
{
    let db = expect_context::<crate::db::Db>();
    let Some(user) = crate::auth::current_user(&db).await else {
        return Ok(vec![]);
    };
    list_mutable_core(&db, &user.github_id)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))
}

// ── Accounts (PRD-0053) ─────────────────────────────────────────────────────

/// Auth surface for the client: whether sign-in exists on this deployment,
/// and who (if anyone) this request's session cookie belongs to.
#[server]
pub async fn get_auth_info() -> Result<ironpad_common::AuthInfo, ServerFnError> {
    use ironpad_common::{AuthInfo, CurrentUser};

    let enabled = expect_context::<crate::auth::AuthEnabled>().0;
    let db = expect_context::<crate::db::Db>();
    let user = crate::auth::current_user(&db).await.map(|u| CurrentUser {
        login: u.login,
        avatar_url: u.avatar_url,
    });
    Ok(AuthInfo { enabled, user })
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
            public_url: "http://localhost".to_string(),
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
        // A ZERO-token bucket, so this test also proves the PRD-0052 claim
        // that cache hits never consult admission: were the limiter in the
        // hit path, this unwrap would trip it.
        let admission = crate::compiler::admission::BuildAdmission::new(
            1,
            0.0,
            0.0,
            std::time::Duration::from_millis(50),
        );
        let response = compile_cell_core(&config, &locks, &admission, "test", request)
            .await
            .unwrap();

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

    /// The compile pipeline emits the stage spans traces are built from
    /// (`compile_cell` → `compile_lock_wait` → `cache_lookup`). Cache-hit
    /// path only, so no cargo is involved; the deeper stages (`cargo_build`,
    /// `wasm_opt`, …) hang off the same mechanism.
    #[tokio::test]
    async fn compile_pipeline_emits_stage_spans() {
        use crate::compiler::cache::{content_hash, store_blob};
        use crate::compiler::CompileLocks;
        use ironpad_common::AppConfig;
        use tracing_subscriber::layer::SubscriberExt as _;

        /// Collects the name of every span opened while installed.
        #[derive(Clone, Default)]
        struct SpanNames(std::sync::Arc<std::sync::Mutex<Vec<String>>>);

        impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for SpanNames {
            fn on_new_span(
                &self,
                attrs: &tracing::span::Attributes<'_>,
                _id: &tracing::span::Id,
                _ctx: tracing_subscriber::layer::Context<'_, S>,
            ) {
                self.0
                    .lock()
                    .unwrap()
                    .push(attrs.metadata().name().to_owned());
            }
        }

        let names = SpanNames::default();
        let _guard =
            tracing::subscriber::set_default(tracing_subscriber::registry().with(names.clone()));

        let cache = tempfile::tempdir().unwrap();
        let source = "    CellOutput::empty()";
        let cargo_toml = "[dependencies]";
        let hash = content_hash(source, cargo_toml, &[], None, None, false, false, false);
        store_blob(
            cache.path(),
            &hash,
            b"\x00asm\x01\x00\x00\x00spans",
            None,
            &[],
        )
        .unwrap();

        let config = AppConfig {
            data_dir: cache.path().to_path_buf(),
            cache_dir: cache.path().to_path_buf(),
            port: 0,
            ironpad_cell_path: cache.path().join("nonexistent-ironpad-cell"),
            compilation_proxy: None,
            public_url: "http://localhost".to_string(),
        };
        let request = CompileRequest {
            notebook_id: "nb".to_string(),
            cell_id: "span-cell".to_string(),
            source: source.to_string(),
            cargo_toml: cargo_toml.to_string(),
            previous_cell_types: vec![],
            shared_cargo_toml: None,
            shared_source: None,
            force: false,
            shared_check: None,
        };

        let locks = CompileLocks::default();
        let admission = crate::compiler::admission::BuildAdmission::new(
            1,
            10.0,
            600.0,
            std::time::Duration::from_secs(1),
        );
        compile_cell_core(&config, &locks, &admission, "test", request)
            .await
            .unwrap();

        let seen = names.0.lock().unwrap().clone();
        for expected in ["compile_cell", "compile_lock_wait", "cache_lookup"] {
            assert!(
                seen.iter().any(|n| n == expected),
                "expected a {expected:?} span; saw {seen:?}"
            );
        }
    }

    // ── Build admission (PRD-0052) ───────────────────────────────────────

    fn admission_test_request(cell_id: &str) -> CompileRequest {
        CompileRequest {
            notebook_id: "nb".to_string(),
            cell_id: cell_id.to_string(),
            source: "    CellOutput::empty()".to_string(),
            cargo_toml: "[dependencies]".to_string(),
            previous_cell_types: vec![],
            shared_cargo_toml: None,
            shared_source: None,
            force: false,
            shared_check: None,
        }
    }

    /// A cache MISS from an empty-bucket client is refused before any cargo
    /// or filesystem work — the error is the limiter's, not a scaffold
    /// failure from the deliberately bogus ironpad-cell path.
    #[tokio::test]
    async fn compile_cache_miss_from_rate_limited_client_is_refused() {
        use crate::compiler::CompileLocks;
        use ironpad_common::AppConfig;

        let dir = tempfile::tempdir().unwrap();
        let config = AppConfig {
            data_dir: dir.path().to_path_buf(),
            cache_dir: dir.path().to_path_buf(),
            port: 0,
            ironpad_cell_path: dir.path().join("nonexistent-ironpad-cell"),
            compilation_proxy: None,
            public_url: "http://localhost".to_string(),
        };
        let admission = crate::compiler::admission::BuildAdmission::new(
            1,
            0.0,
            0.0,
            std::time::Duration::from_millis(50),
        );

        let err = compile_cell_core(
            &config,
            &CompileLocks::default(),
            &admission,
            "9.9.9.9",
            admission_test_request("rate-limited-cell"),
        )
        .await
        .unwrap_err();

        assert!(
            err.to_string().contains("rate limited"),
            "expected the limiter's message, got: {err}"
        );
    }

    /// A live check with every slot busy degrades to `Skipped` — the status
    /// the client already retries — instead of queueing behind capacity.
    #[tokio::test]
    async fn check_with_no_free_slot_returns_skipped() {
        use crate::compiler::CompileLocks;
        use ironpad_common::AppConfig;
        use ironpad_common::CheckStatus;

        let dir = tempfile::tempdir().unwrap();
        let config = AppConfig {
            data_dir: dir.path().to_path_buf(),
            cache_dir: dir.path().to_path_buf(),
            port: 0,
            ironpad_cell_path: dir.path().join("nonexistent-ironpad-cell"),
            compilation_proxy: None,
            public_url: "http://localhost".to_string(),
        };
        let admission = crate::compiler::admission::BuildAdmission::new(
            1,
            10.0,
            600.0,
            std::time::Duration::from_millis(50),
        );
        let _held = admission.try_admit_check().expect("the single check slot");

        let response = check_cell_core(
            &config,
            &CompileLocks::default(),
            &admission,
            admission_test_request("busy-check-cell"),
        )
        .await
        .unwrap();

        assert_eq!(response.status, CheckStatus::Skipped);
        assert!(response.diagnostics.is_empty());
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
            saved_output: None,
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
    async fn snapshot_does_not_overwrite_an_existing_entry() {
        // Security: re-share is unauthenticated and idempotent, so a fresh
        // snapshot must never REPLACE an existing cell's blob pointer — that
        // would let an attacker re-POST a share's JSON with different tags
        // and change which WASM existing viewers run. The first entry freezes.
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();
        let nb = snapshot_notebook(vec![code_cell("cell-1", "let a = 1;")]);
        let tags: Vec<String> = vec![String::new()];
        // Seed the cache so this snapshot WOULD produce a fresh cell-1 entry.
        let fresh = seed_cache(cache.path(), &nb, &tags, 0, b"wasm-fresh", None);

        // A prior share already froze cell-1 to a DIFFERENT blob pointer.
        let shares = data.path().join("shares");
        tokio::fs::create_dir_all(&shares).await.unwrap();
        let frozen_blob = "a".repeat(64);
        assert_ne!(
            frozen_blob, fresh,
            "the fresh key must differ to prove freeze"
        );
        let old = ironpad_common::ShareManifest {
            version: 1,
            cells: std::collections::BTreeMap::from([(
                "cell-1".to_string(),
                ironpad_common::ShareBlobEntry {
                    blob: frozen_blob.clone(),
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

        snapshot_share_blobs(data.path(), cache.path(), &nb, &tags, "aabbccdd00112233")
            .await
            .unwrap();

        let manifest = get_shared_manifest_core(data.path(), "aabbccdd00112233")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            manifest.cells["cell-1"].blob, frozen_blob,
            "the original entry must survive; a re-share cannot replace it"
        );
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

    // ── Mutable shares (accounts-backed, PRD-0053) ──────────────────

    fn zero_cell_notebook_json(title: &str) -> String {
        let mut nb = IronpadNotebook::new(title);
        nb.cells = vec![];
        serde_json::to_string(&nb).unwrap()
    }

    /// A DB seeded with two users: "1" (alice) and "2" (bob).
    async fn accounts_db() -> (tempfile::TempDir, crate::db::Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(&dir.path().join("test.db"))
            .await
            .unwrap();
        db.upsert_user("1", "alice", "").await.unwrap();
        db.upsert_user("2", "bob", "").await.unwrap();
        (dir, db)
    }

    #[tokio::test]
    async fn private_shares_gate_every_read_surface() {
        use ironpad_common::MutableNotebookAccess as Access;

        let (_dbdir, db) = accounts_db().await;
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();

        let alice = crate::db::AuthUser {
            github_id: "1".into(),
            login: "alice".into(),
            avatar_url: String::new(),
        };
        let bob = crate::db::AuthUser {
            github_id: "2".into(),
            login: "bob".into(),
            avatar_url: String::new(),
        };

        let id = create_mutable_share_core(
            &db,
            data.path(),
            cache.path(),
            "1",
            VALID_NOTEBOOK_JSON,
            &[String::new()],
            MAX_TOTAL_MUTABLE_BYTES,
        )
        .await
        .unwrap();

        // Public by default: everyone reads.
        assert!(matches!(
            mutable_access_core(&db, &id, None).await.unwrap(),
            Access::Found(_)
        ));

        // Flip private: anonymous and non-grantees are denied with the
        // explicit arm (signed_in picks the copy); the owner still reads.
        db.set_share_private(&id, true).await.unwrap();
        assert_eq!(
            mutable_access_core(&db, &id, None).await.unwrap(),
            Access::Private { signed_in: false }
        );
        assert_eq!(
            mutable_access_core(&db, &id, Some(&bob)).await.unwrap(),
            Access::Private { signed_in: true }
        );
        assert!(matches!(
            mutable_access_core(&db, &id, Some(&alice)).await.unwrap(),
            Access::Found(_)
        ));

        // The anonymous surfaces (OG card, oEmbed) and the manifest deny.
        assert!(get_mutable_notebook_core(&db, &id).await.unwrap().is_none());
        assert!(mutable_manifest_access_core(&db, &id, None)
            .await
            .unwrap()
            .is_none());

        // Grant READ to bob (case-insensitive login lookup): he reads, and
        // the grant is idempotent + listed. An unknown handle errors.
        let target = db.find_user_by_login("BoB").await.unwrap().unwrap();
        assert_eq!(target.github_id, "2");
        assert!(db.find_user_by_login("ghost").await.unwrap().is_none());
        db.grant_read(&id, "2").await.unwrap();
        db.grant_read(&id, "2").await.unwrap(); // double-click is a no-op
        let access = mutable_access_core(&db, &id, Some(&bob)).await.unwrap();
        let Access::Found(found) = access else {
            panic!("grantee must read a private share");
        };
        assert!(!found.is_owner, "READ is not ownership");
        let grants = db.list_read_grants(&id).await.unwrap();
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].login, "bob");

        // Revoke: denied again. Flip public: everyone reads again.
        db.revoke_read(&id, "2").await.unwrap();
        assert_eq!(
            mutable_access_core(&db, &id, Some(&bob)).await.unwrap(),
            Access::Private { signed_in: true }
        );
        db.set_share_private(&id, false).await.unwrap();
        assert!(matches!(
            mutable_access_core(&db, &id, None).await.unwrap(),
            Access::Found(_)
        ));

        // The manifest gate runs the SAME predicate, on all four arms — it
        // is the hash list unlocking the ungated /share-blobs route, so the
        // signed-in arms matter as much as the anonymous one. A share with
        // an actual manifest distinguishes denial (None) from admission.
        let manifest = serde_json::to_string(&ironpad_common::ShareManifest {
            version: 1,
            cells: std::collections::BTreeMap::new(),
        })
        .unwrap();
        db.create_mutable_share("manifested0000ff", "1", VALID_NOTEBOOK_JSON, Some(manifest))
            .await
            .unwrap();
        db.set_share_private("manifested0000ff", true)
            .await
            .unwrap();
        assert!(
            mutable_manifest_access_core(&db, "manifested0000ff", None)
                .await
                .unwrap()
                .is_none(),
            "anonymous: manifest withheld"
        );
        assert!(
            mutable_manifest_access_core(&db, "manifested0000ff", Some(&bob))
                .await
                .unwrap()
                .is_none(),
            "signed-in non-grantee: manifest withheld"
        );
        db.grant_read("manifested0000ff", "2").await.unwrap();
        assert!(
            mutable_manifest_access_core(&db, "manifested0000ff", Some(&bob))
                .await
                .unwrap()
                .is_some(),
            "grantee: manifest served"
        );
        assert!(
            mutable_manifest_access_core(&db, "manifested0000ff", Some(&alice))
                .await
                .unwrap()
                .is_some(),
            "owner: manifest served"
        );

        // Unpublish wipes the READ grants with the share.
        db.grant_read(&id, "2").await.unwrap();
        delete_mutable_core(&db, "1", &id).await.unwrap();
        assert!(db.list_read_grants(&id).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn mutable_lifecycle_is_owner_gated() {
        let (_dbdir, db) = accounts_db().await;
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();

        let id = create_mutable_share_core(
            &db,
            data.path(),
            cache.path(),
            "1",
            VALID_NOTEBOOK_JSON,
            &[String::new()],
            MAX_TOTAL_MUTABLE_BYTES,
        )
        .await
        .unwrap();
        assert_eq!(id.len(), 16, "server mints a 16-hex id");

        let nb = get_mutable_notebook_core(&db, &id)
            .await
            .unwrap()
            .expect("share exists");
        assert_eq!(nb.title, "Test Notebook");
        // Unknown id resolves to None, not an error.
        assert!(get_mutable_notebook_core(&db, "0000000000000000")
            .await
            .unwrap()
            .is_none());

        // The owner drafts, then pushes; readers see nothing until the push.
        save_mutable_draft_core(
            &db,
            "1",
            &id,
            &zero_cell_notebook_json("Pushed"),
            MAX_TOTAL_MUTABLE_BYTES,
        )
        .await
        .expect("owner draft save");
        assert_eq!(
            get_mutable_notebook_core(&db, &id)
                .await
                .unwrap()
                .unwrap()
                .title,
            "Test Notebook",
            "readers must not see the draft"
        );
        assert!(
            push_mutable_core(&db, data.path(), cache.path(), "1", &id, &[])
                .await
                .expect("owner push"),
            "a dirty draft promotes"
        );
        assert_eq!(
            get_mutable_notebook_core(&db, &id)
                .await
                .unwrap()
                .unwrap()
                .title,
            "Pushed"
        );

        // Pushing a clean draft is a no-op, reported as such.
        assert!(
            !push_mutable_core(&db, data.path(), cache.path(), "1", &id, &[])
                .await
                .expect("clean push is Ok(false)")
        );

        // A different signed-in user can neither draft nor push.
        let err = save_mutable_draft_core(
            &db,
            "2",
            &id,
            &zero_cell_notebook_json("nope"),
            MAX_TOTAL_MUTABLE_BYTES,
        )
        .await
        .unwrap_err()
        .to_string();
        assert!(err.contains("unauthorized"), "unexpected error: {err}");
        let err = push_mutable_core(&db, data.path(), cache.path(), "2", &id, &[])
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("unauthorized"), "unexpected error: {err}");
        assert_eq!(
            get_mutable_notebook_core(&db, &id)
                .await
                .unwrap()
                .unwrap()
                .title,
            "Pushed"
        );

        // Push to an unknown id fails the same way (no existence oracle).
        assert!(
            push_mutable_core(&db, data.path(), cache.path(), "1", "0000000000000000", &[])
                .await
                .is_err()
        );

        // Non-owner can't unpublish; the owner can.
        assert!(delete_mutable_core(&db, "2", &id).await.is_err());
        assert!(get_mutable_notebook_core(&db, &id).await.unwrap().is_some());
        delete_mutable_core(&db, "1", &id).await.unwrap();
        assert!(get_mutable_notebook_core(&db, &id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn mutable_list_enumerates_only_the_owners_shares() {
        let (_dbdir, db) = accounts_db().await;
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();

        for (owner, title) in [("1", "Alice One"), ("1", "Alice Two"), ("2", "Bob One")] {
            create_mutable_share_core(
                &db,
                data.path(),
                cache.path(),
                owner,
                &zero_cell_notebook_json(title),
                &[],
                MAX_TOTAL_MUTABLE_BYTES,
            )
            .await
            .unwrap();
        }

        let alice = list_mutable_core(&db, "1").await.unwrap();
        assert_eq!(alice.len(), 2);
        assert!(alice.iter().all(|s| s.title.starts_with("Alice")));
        assert_eq!(list_mutable_core(&db, "2").await.unwrap().len(), 1);
        assert!(list_mutable_core(&db, "999").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn mutable_manifest_absent_on_cold_cache_and_create_validates() {
        let (_dbdir, db) = accounts_db().await;
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();

        // Cold cache → no snapshot → live-compile share (manifest None).
        let id = create_mutable_share_core(
            &db,
            data.path(),
            cache.path(),
            "1",
            VALID_NOTEBOOK_JSON,
            &[String::new()],
            MAX_TOTAL_MUTABLE_BYTES,
        )
        .await
        .unwrap();
        assert!(db
            .get_mutable_share(&id)
            .await
            .unwrap()
            .unwrap()
            .manifest_json
            .is_none());

        // Garbage JSON is rejected before anything is stored.
        assert!(create_mutable_share_core(
            &db,
            data.path(),
            cache.path(),
            "1",
            "not json",
            &[],
            MAX_TOTAL_MUTABLE_BYTES,
        )
        .await
        .is_err());
    }

    #[tokio::test]
    async fn draft_saves_count_toward_the_aggregate_cap() {
        let (_dbdir, db) = accounts_db().await;
        let data = tempfile::tempdir().unwrap();
        let cache = tempfile::tempdir().unwrap();

        // A cap sized so the published notebook fits but published + one
        // draft does not: drafts used to bypass the aggregate accounting
        // entirely, making stored bytes owner-drivable without limit.
        let published = zero_cell_notebook_json("Capped");
        let draft = zero_cell_notebook_json("Drafty");
        let one_byte_short = published.len() as u64 + draft.len() as u64 - 1;
        let id = create_mutable_share_core(
            &db,
            data.path(),
            cache.path(),
            "1",
            &published,
            &[],
            one_byte_short,
        )
        .await
        .expect("published copy fits under the cap");

        let err = save_mutable_draft_core(&db, "1", &id, &draft, one_byte_short)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("store full"), "unexpected error: {err}");

        // With headroom the same save lands, and an overwrite of the SAME
        // draft passes a cap of total + one draft: the check is conservative
        // (it still counts the draft being replaced) but the slot itself is
        // replaced, not appended.
        save_mutable_draft_core(&db, "1", &id, &draft, MAX_TOTAL_MUTABLE_BYTES)
            .await
            .expect("draft fits under the real cap");
        let total = db.total_mutable_bytes().await.unwrap();
        assert_eq!(total, (published.len() + draft.len()) as u64);
        save_mutable_draft_core(&db, "1", &id, &draft, total + draft.len() as u64)
            .await
            .expect("replacing the draft is bounded, not accumulating");
    }
}
