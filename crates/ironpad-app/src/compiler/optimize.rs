//! Best-effort WASM optimization via `wasm-opt`.
//!
//! If `wasm-opt` is not installed, optimization is skipped silently
//! (logged at debug level). The pass runs `-O3`: runtime performance over
//! binary size, since cells execute repeatedly (simulations tick every
//! frame) while the blob is fetched once and cached.

use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use anyhow::Result;
use tokio::process::Command;
use tracing::{debug, info, warn};

/// Hard timeout for a single `wasm-opt` invocation. `wasm-opt` on a cell blob is
/// quick, so this is a generous backstop that stops a pathological input from
/// hanging a compile indefinitely (build has its own 300s timeout). Optimization
/// is best-effort — on timeout we simply fall back to the unoptimized bytes.
const WASM_OPT_TIMEOUT: Duration = Duration::from_secs(120);

/// Attempt to optimize a WASM blob in-place using `wasm-opt -O3`.
///
/// Uses `-O3` (runtime performance) rather than `-Oz` (size) because cell
/// execution speed matters more than shaving a few KB off download size.
///
/// Returns the (possibly optimized) bytes. If `wasm-opt` is unavailable
/// or fails, returns the original bytes unchanged.
#[tracing::instrument(name = "wasm_opt", level = "info", skip_all, fields(original_size = wasm_bytes.len(), optimized_size = tracing::field::Empty))]
pub async fn optimize_wasm(wasm_bytes: &[u8], work_dir: &Path, needs_atomics: bool) -> Vec<u8> {
    match try_optimize(wasm_bytes, work_dir, needs_atomics).await {
        Ok(optimized) => {
            tracing::Span::current().record("optimized_size", optimized.len());
            // WASM blob sizes are always well within i64 range.
            #[allow(clippy::cast_possible_wrap)]
            let saved = wasm_bytes.len() as i64 - optimized.len() as i64;
            info!(
                original_size = wasm_bytes.len(),
                optimized_size = optimized.len(),
                bytes_saved = saved,
                "wasm-opt optimization applied"
            );
            optimized
        }
        Err(e) => {
            debug!(error = %e, "wasm-opt optimization skipped");
            wasm_bytes.to_vec()
        }
    }
}

async fn try_optimize(wasm_bytes: &[u8], work_dir: &Path, needs_atomics: bool) -> Result<Vec<u8>> {
    // Unique per-compile filenames: `work_dir` is shared across cells, so fixed
    // names (`pre_opt.wasm`/`post_opt.wasm`) let concurrent compiles of
    // different cells clobber each other's temp files mid-optimization.
    let stem = uuid::Uuid::new_v4();
    let input_path = work_dir.join(format!("opt-{stem}.pre.wasm"));
    let output_path = work_dir.join(format!("opt-{stem}.post.wasm"));

    let result = run_wasm_opt(wasm_bytes, &input_path, &output_path, needs_atomics).await;

    // Clean up on EVERY path (best-effort): the work dir is persistent and
    // shared, and failure paths used to leak both temp blobs per failed
    // optimization — an unbounded disk drip on a box where wasm-opt is
    // broken or the input is pathological.
    let _ = tokio::fs::remove_file(&input_path).await;
    let _ = tokio::fs::remove_file(&output_path).await;

    result
}

/// The fallible middle of [`try_optimize`], separated so its `?` returns all
/// funnel through the caller's temp-file cleanup.
async fn run_wasm_opt(
    wasm_bytes: &[u8],
    input_path: &Path,
    output_path: &Path,
    needs_atomics: bool,
) -> Result<Vec<u8>> {
    tokio::fs::write(input_path, wasm_bytes).await?;

    let mut cmd = Command::new("wasm-opt");
    cmd.arg("-O3").arg("--debuginfo");

    if needs_atomics {
        cmd.arg("--enable-threads");
    }

    cmd.arg(input_path)
        .arg("-o")
        .arg(output_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    // Shared group-kill discipline with the build/check paths (wasm-opt
    // spawns no children today, but the tree dies either way).
    let output = super::build::run_group_with_timeout(cmd, WASM_OPT_TIMEOUT)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!("wasm-opt timed out after {}s", WASM_OPT_TIMEOUT.as_secs())
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(stderr = %stderr, "wasm-opt exited with non-zero status");
        anyhow::bail!("wasm-opt failed: {stderr}");
    }

    Ok(tokio::fs::read(output_path).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use tempfile::tempdir;

    #[tokio::test]
    async fn optimize_returns_original_when_wasm_opt_missing() {
        let dir = tempdir().unwrap();
        let fake_wasm = b"not-a-real-wasm-file";

        let result = optimize_wasm(fake_wasm, dir.path(), false).await;

        // Should return original bytes since wasm-opt either isn't installed
        // or will fail on invalid input.
        assert_eq!(result.len(), fake_wasm.len());
    }

    #[tokio::test]
    async fn failure_paths_leave_no_temp_files_behind() {
        // wasm-opt on garbage bytes fails (or wasm-opt is missing entirely);
        // either way the shared work dir must come back empty — failure
        // paths used to leak both temp blobs.
        let dir = tempdir().unwrap();
        let _ = optimize_wasm(b"not-a-real-wasm-file", dir.path(), false).await;
        let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert!(leftovers.is_empty(), "temp files leaked: {leftovers:?}");
    }

    #[tokio::test]
    async fn run_group_timeout_kills_overlong_process() {
        // A 30s sleep under a 200ms budget must be killed, not waited out
        // (the shared helper's own tests cover grandchildren).
        let mut cmd = Command::new("sleep");
        cmd.arg("30");

        let start = Instant::now();
        let result = crate::compiler::build::run_group_with_timeout(cmd, Duration::from_millis(200))
            .await
            .unwrap();

        assert!(result.is_none(), "an over-budget process must time out");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the child must be killed promptly, not waited out ({:?})",
            start.elapsed()
        );
    }
}
