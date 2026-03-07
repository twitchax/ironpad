//! Best-effort WASM optimization via `wasm-opt`.
//!
//! If `wasm-opt` is not installed, optimization is skipped silently
//! (logged at debug level). This is a size optimization pass only.

use std::path::Path;

use anyhow::Result;
use tracing::{debug, info, warn};

/// Attempt to optimize a WASM blob in-place using `wasm-opt -Oz`.
///
/// Returns the (possibly optimized) bytes. If `wasm-opt` is unavailable
/// or fails, returns the original bytes unchanged.
pub async fn optimize_wasm(wasm_bytes: &[u8], work_dir: &Path) -> Vec<u8> {
    match try_optimize(wasm_bytes, work_dir).await {
        Ok(optimized) => {
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

async fn try_optimize(wasm_bytes: &[u8], work_dir: &Path) -> Result<Vec<u8>> {
    let input_path = work_dir.join("pre_opt.wasm");
    let output_path = work_dir.join("post_opt.wasm");

    tokio::fs::write(&input_path, wasm_bytes).await?;

    let output = tokio::process::Command::new("wasm-opt")
        .arg("-Oz")
        .arg(&input_path)
        .arg("-o")
        .arg(&output_path)
        .output()
        .await
        .map_err(|e| anyhow::anyhow!("wasm-opt not available: {e}"))?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn optimize_returns_original_when_wasm_opt_missing() {
        let dir = tempdir().unwrap();
        let fake_wasm = b"not-a-real-wasm-file";

        let result = optimize_wasm(fake_wasm, dir.path()).await;

        // Should return original bytes since wasm-opt either isn't installed
        // or will fail on invalid input.
        assert_eq!(result.len(), fake_wasm.len());
    }
}
