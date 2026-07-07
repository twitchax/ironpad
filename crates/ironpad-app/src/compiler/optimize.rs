//! Best-effort WASM optimization via `wasm-opt`.
//!
//! If `wasm-opt` is not installed, optimization is skipped silently
//! (logged at debug level). This is a size optimization pass only.

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
pub async fn optimize_wasm(wasm_bytes: &[u8], work_dir: &Path, needs_atomics: bool) -> Vec<u8> {
    match try_optimize(wasm_bytes, work_dir, needs_atomics).await {
        Ok(optimized) => {
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

    tokio::fs::write(&input_path, wasm_bytes).await?;

    let mut cmd = Command::new("wasm-opt");
    cmd.arg("-O3").arg("--debuginfo");

    if needs_atomics {
        cmd.arg("--enable-threads");
    }

    cmd.arg(&input_path)
        .arg("-o")
        .arg(&output_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let output = run_with_timeout(cmd, WASM_OPT_TIMEOUT).await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        warn!(stderr = %stderr, "wasm-opt exited with non-zero status");
        anyhow::bail!("wasm-opt failed: {stderr}");
    }

    let optimized = tokio::fs::read(&output_path).await?;

    // Clean up temp files (best-effort).
    let _ = tokio::fs::remove_file(&input_path).await;
    let _ = tokio::fs::remove_file(&output_path).await;

    Ok(optimized)
}

/// Run `cmd` to completion under a hard timeout, killing the whole process group
/// if it exceeds it.
///
/// Mirrors the kill discipline in [`super::build::build_micro_crate`]: the child
/// leads its own process group so a timeout can `killpg` the entire tree in one
/// signal (`wasm-opt` spawns no children today, but this is robust if that ever
/// changes), and `kill_on_drop` is a backstop if the future is dropped.
async fn run_with_timeout(mut cmd: Command, timeout: Duration) -> Result<Output> {
    // Run as its own process-group leader so a timeout kills the whole tree.
    #[cfg(unix)]
    cmd.process_group(0);
    // Backstop: if the future is dropped, kill at least the direct child.
    cmd.kill_on_drop(true);

    let program = cmd.as_std().get_program().to_string_lossy().to_string();
    let child = cmd
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to spawn {program}: {e}"))?;
    let child_pid = child.id();

    let Ok(result) = tokio::time::timeout(timeout, child.wait_with_output()).await else {
        // Timed out: kill the whole process group. The negated pgid equals the
        // child's pid because we made it the group leader; SIGKILL can't be
        // caught, so the tree dies.
        #[cfg(unix)]
        if let Some(pid) = child_pid.and_then(|p| i32::try_from(p).ok()) {
            // SAFETY: kill(2) with a negative pid signals the process group.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        anyhow::bail!("{program} timed out after {}s", timeout.as_secs());
    };
    result.map_err(|e| anyhow::anyhow!("{program} failed: {e}"))
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
    async fn run_with_timeout_kills_overlong_process() {
        // A 30s sleep under a 200ms budget must be killed, not waited out.
        let mut cmd = Command::new("sleep");
        cmd.arg("30");

        let start = Instant::now();
        let result = run_with_timeout(cmd, Duration::from_millis(200)).await;

        assert!(result.is_err(), "an over-budget process must time out");
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the child must be killed promptly, not waited out ({:?})",
            start.elapsed()
        );
    }
}
