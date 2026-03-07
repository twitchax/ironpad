//! WASM compilation via `cargo build` for scaffolded micro-crates.
//!
//! Given a scaffolded micro-crate directory (produced by [`super::scaffold`]),
//! this module invokes `cargo build --target wasm32-unknown-unknown --release`
//! and returns either the path to the compiled `.wasm` blob or the raw cargo
//! output on failure.

use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::process::Command;

/// Hard timeout for a single `cargo build` invocation.
const BUILD_TIMEOUT: Duration = Duration::from_secs(30);

// ── Build Result ─────────────────────────────────────────────────────────────

/// Outcome of a micro-crate build attempt.
///
/// Infrastructure errors (spawn failure, timeout) are returned as `Err` from
/// [`build_micro_crate`].  Compilation success vs. failure is represented here
/// so the caller can inspect stdout (JSON diagnostics) in both cases.
pub enum BuildResult {
    /// Compilation succeeded; the WASM blob is on disk.
    Success {
        wasm_path: PathBuf,
        stdout: String,
        stderr: String,
    },
    /// Compilation failed (non-zero exit); stdout contains JSON diagnostics.
    Failure { stdout: String, stderr: String },
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Build a scaffolded micro-crate to WASM.
///
/// Runs `cargo build --target wasm32-unknown-unknown --release
/// --message-format=json` in `crate_dir`, with:
///
/// * `CARGO_HOME` → shared registry cache under `cache_dir`
/// * `CARGO_TARGET_DIR` → per-session directory for incremental reuse
///
/// Returns [`BuildResult::Success`] with the `.wasm` blob path on success,
/// or [`BuildResult::Failure`] with raw cargo output on compilation failure.
///
/// # Errors
///
/// Returns `Err` for infrastructure problems: failed to spawn cargo, build
/// timeout exceeded, or missing `.wasm` artifact after a successful exit code.
pub async fn build_micro_crate(
    crate_dir: &Path,
    cache_dir: &Path,
    session_id: &str,
    cell_id: &str,
) -> anyhow::Result<BuildResult> {
    let cargo_home = cargo_home_dir(cache_dir);
    let target_dir = target_dir(cache_dir, session_id);

    std::fs::create_dir_all(&cargo_home)?;
    std::fs::create_dir_all(&target_dir)?;

    // Canonicalize paths so they resolve correctly when cargo runs in crate_dir.
    let cargo_home = std::fs::canonicalize(&cargo_home)?;
    let target_dir = std::fs::canonicalize(&target_dir)?;

    tracing::info!(
        cell_id = %cell_id,
        crate_dir = %crate_dir.display(),
        cargo_home = %cargo_home.display(),
        target_dir = %target_dir.display(),
        "starting WASM build",
    );

    let output = tokio::time::timeout(BUILD_TIMEOUT, {
        Command::new("cargo")
            .arg("build")
            .arg("--target")
            .arg("wasm32-unknown-unknown")
            .arg("--release")
            .arg("--message-format=json")
            .current_dir(crate_dir)
            .env("CARGO_HOME", &cargo_home)
            .env("CARGO_TARGET_DIR", &target_dir)
            .output()
    })
    .await
    .map_err(|_| {
        tracing::error!(cell_id = %cell_id, "cargo build timed out after {}s", BUILD_TIMEOUT.as_secs());
        anyhow::anyhow!("compilation timed out after {}s", BUILD_TIMEOUT.as_secs())
    })?
    .map_err(|e| {
        tracing::error!(cell_id = %cell_id, error = %e, "failed to spawn cargo");
        anyhow::anyhow!("failed to spawn cargo: {e}")
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        tracing::warn!(
            cell_id = %cell_id,
            exit_code = ?output.status.code(),
            stderr = %stderr,
            "cargo build failed",
        );
        return Ok(BuildResult::Failure { stdout, stderr });
    }

    let wasm_path = expected_wasm_path(&target_dir, cell_id);

    anyhow::ensure!(
        wasm_path.exists(),
        "WASM blob not found at expected path: {}",
        wasm_path.display(),
    );

    tracing::info!(wasm_path = %wasm_path.display(), "WASM build succeeded");

    Ok(BuildResult::Success {
        wasm_path,
        stdout,
        stderr,
    })
}

// ── Path Helpers ─────────────────────────────────────────────────────────────

/// Shared `CARGO_HOME` directory for registry caching across all builds.
pub fn cargo_home_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("cargo-home")
}

/// Per-session `CARGO_TARGET_DIR` for incremental build reuse.
pub fn target_dir(cache_dir: &Path, session_id: &str) -> PathBuf {
    cache_dir.join("targets").join(session_id)
}

/// Compute the expected path to the compiled `.wasm` blob.
///
/// Cargo converts crate-name hyphens to underscores in artifact filenames.
/// The scaffolded crate name is `cell-{cell_id}` (see [`super::scaffold`]).
pub fn expected_wasm_path(target_dir: &Path, cell_id: &str) -> PathBuf {
    let crate_name = format!("cell-{cell_id}");
    let wasm_filename = format!("{}.wasm", crate_name.replace('-', "_"));

    target_dir
        .join("wasm32-unknown-unknown")
        .join("release")
        .join(wasm_filename)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── cargo_home_dir ──────────────────────────────────────────────────

    #[test]
    fn cargo_home_under_cache() {
        let dir = cargo_home_dir(Path::new("/cache"));
        assert_eq!(dir, PathBuf::from("/cache/cargo-home"));
    }

    // ── target_dir ──────────────────────────────────────────────────────

    #[test]
    fn target_dir_per_session() {
        let dir = target_dir(Path::new("/cache"), "session-1");
        assert_eq!(dir, PathBuf::from("/cache/targets/session-1"));
    }

    #[test]
    fn target_dir_different_sessions_are_isolated() {
        let a = target_dir(Path::new("/cache"), "sess-a");
        let b = target_dir(Path::new("/cache"), "sess-b");
        assert_ne!(a, b);
    }

    // ── expected_wasm_path ──────────────────────────────────────────────

    #[test]
    fn wasm_path_simple_id() {
        let path = expected_wasm_path(Path::new("/t"), "abc123");
        assert_eq!(
            path,
            PathBuf::from("/t/wasm32-unknown-unknown/release/cell_abc123.wasm"),
        );
    }

    #[test]
    fn wasm_path_hyphenated_id() {
        let path = expected_wasm_path(Path::new("/t"), "cell-0");
        assert_eq!(
            path,
            PathBuf::from("/t/wasm32-unknown-unknown/release/cell_cell_0.wasm"),
        );
    }

    #[test]
    fn wasm_path_nested_hyphens() {
        let path = expected_wasm_path(Path::new("/t"), "a-b-c");
        assert_eq!(
            path,
            PathBuf::from("/t/wasm32-unknown-unknown/release/cell_a_b_c.wasm"),
        );
    }

    #[test]
    fn wasm_path_underscore_id() {
        let path = expected_wasm_path(Path::new("/t"), "my_cell");
        assert_eq!(
            path,
            PathBuf::from("/t/wasm32-unknown-unknown/release/cell_my_cell.wasm"),
        );
    }
}
