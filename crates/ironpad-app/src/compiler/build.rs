//! WASM compilation via `cargo build` for scaffolded micro-crates.
//!
//! Given a scaffolded micro-crate directory (produced by [`super::scaffold`]),
//! this module invokes `cargo build --target {triple} --release`, where the
//! triple comes from the cell's [`CellTarget`] — the same value its cache key
//! was computed from, so the artifact on disk and the key it is stored under
//! can never describe different targets.
//!
//! Ordinary cells build a `cdylib` for `wasm32-unknown-unknown` and are
//! post-processed with `wasm-bindgen` into JS glue plus a transformed blob for
//! browser `import()`. Linux cells (PRD-0066) build a whole program for
//! `wasm32-browserpod-linux-musl`, whose artifact is an executable exporting
//! `_start`; there is no wasm-bindgen stage for them, because nothing about
//! that binary crosses a JS boundary — a pod loads it as a process image.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use tokio::process::Command;
use tracing::Instrument as _;

use ironpad_common::cache_key::CellTarget;

use crate::CELL_TOOLCHAIN;

/// Spawn `cmd` as its own process-group leader and await its output under
/// `timeout`. Returns `None` on timeout — after `SIGKILL`ing the WHOLE group.
///
/// The group kill is the point: cargo fans out rustc and build-script
/// children, and killing only the direct child (what `kill_on_drop` does)
/// leaves a compile-bomb's children burning CPU/RAM and writing into the
/// shared target dir after the caller's locks are released — racing the next
/// build that enters it. Shared by the build and check paths (and any other
/// subprocess with a fan-out risk).
pub(crate) async fn run_group_with_timeout(
    mut cmd: Command,
    timeout: Duration,
) -> anyhow::Result<Option<std::process::Output>> {
    // Backstop: if this future is dropped (not just timed out), kill at
    // least the direct child.
    cmd.kill_on_drop(true);
    #[cfg(unix)]
    cmd.process_group(0);

    let child = cmd.spawn().context("failed to spawn subprocess")?;
    let child_pid = child.id();

    let Ok(wait_result) = tokio::time::timeout(timeout, child.wait_with_output()).await else {
        // Timed out: kill the whole process group. The negated pgid equals
        // the child's pid because we made it the group leader; SIGKILL can't
        // be caught, so the tree dies.
        #[cfg(unix)]
        if let Some(pid) = child_pid.and_then(|p| i32::try_from(p).ok()) {
            // SAFETY: kill(2) with a negative pid signals the process group.
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
        #[cfg(not(unix))]
        let _ = child_pid;
        return Ok(None);
    };
    wait_result.map(Some).context("subprocess failed to run")
}

/// Cap on the `wasm-bindgen` post-processing subprocess. Normally sub-second
/// per blob; the cap only exists so a wedged run cannot hold a compile slot
/// forever.
const WASM_BINDGEN_TIMEOUT: Duration = Duration::from_mins(2);

/// Target features for atomics/shared-memory WASM builds (rayon cells).
///
/// Kept separate from [`ATOMICS_LINK_RUSTFLAGS`] because rustc keeps only the
/// **last** `-C target-feature=` occurrence — features from independent
/// concerns (atomics, simd) must be merged into a single flag or the earlier
/// set is silently dropped (see [`compose_rustflags`]).
const ATOMICS_TARGET_FEATURES: &str = "+atomics,+bulk-memory,+mutable-globals";

/// Linker flags for atomics/shared-memory WASM builds (rayon cells).
///
/// Required by wasm-bindgen-rayon: shared memory, imported memory, and TLS
/// exports so that the resulting WASM module can be shared across Web Workers
/// via `postMessage`.
const ATOMICS_LINK_RUSTFLAGS: &str = "\
    -C link-arg=--shared-memory \
    -C link-arg=--max-memory=2147483648 \
    -C link-arg=--import-memory \
    -C link-arg=--export=__wasm_init_tls \
    -C link-arg=--export=__tls_size \
    -C link-arg=--export=__tls_align \
    -C link-arg=--export=__tls_base";

/// Target feature enabling baseline WASM SIMD (fixed-width 128-bit) for cells
/// that use `std::simd` / `std::arch::wasm32` (PRD-0042). No `-Zbuild-std`
/// needed: simd128 cell code links fine against the precompiled non-simd std.
const SIMD_TARGET_FEATURES: &str = "+simd128";

/// RUSTFLAGS enabling Enzyme autodiff (`std::autodiff`) for a cell build.
const AUTODIFF_RUSTFLAGS: &str = "-Zautodiff=Enable";

/// Toolchain for Linux cells (PRD-0066), which target
/// `wasm32-browserpod-linux-musl`.
///
/// Unlike the other three pins this is not a nightly date: it is a vendor
/// toolchain (`BrowserPod` 3.0.0, pinning `nightly-2026-05-19` beneath) that
/// installs as an ordinary rustup toolchain and carries the target spec, a
/// musl sysroot, a prebuilt std, and cargo/rustc wrappers that inject its
/// `[patch]` set and `RUST_TARGET_PATH`. Invoking it as `cargo
/// +browserpod-3.0.0` is therefore all the setup a Linux cell build needs —
/// no RUSTFLAGS, no `-Zbuild-std`, no target JSON to locate.
///
/// This pin is NOT part of the cache fingerprint (which tracks only
/// `CELL_TOOLCHAIN`), so bumping it needs a `CACHE_EPOCH` bump to invalidate
/// stale Linux blobs.
///
/// Since PRD-0067 it carries a second obligation: the nightly this pack pins
/// beneath itself IS [`crate::CELL_TOOLCHAIN`], so a pack whose `nightly-pin`
/// moves regrows the image by a whole toolchain. `browserpod_pin_matches_cell_toolchain`
/// fails the build rather than letting that happen quietly.
const BROWSERPOD_TOOLCHAIN: &str = "browserpod-3.0.0";

/// Pick the pinned toolchain for a cell build from its target.
///
/// Two pins, and the target alone decides: [`BROWSERPOD_TOOLCHAIN`] builds for
/// `wasm32-browserpod-linux-musl` (no other toolchain can), and
/// [`crate::CELL_TOOLCHAIN`] builds everything else.
///
/// It used to take `needs_atomics` and `needs_autodiff` and route to two more
/// nightlies. PRD-0067 collapsed those onto one date after finding that two of
/// the three justifications had expired: rayon builds clean on nightlies five
/// months past the pin that was "the newest one wasm-bindgen-rayon's atomics
/// guard tolerates", and the workspace pin beneath it was still held for a
/// dependency (`thaw`) deleted in v0.12.13. The autodiff ICE is the one that
/// was real, and it is what sets the date.
///
/// **The features did not go away, only the toolchain split did.** Atomics
/// still gets its target features, link args and `-Zbuild-std`; autodiff still
/// gets `-Zautodiff=Enable` and a fat-LTO profile. Cargo fingerprints those
/// builds apart by RUSTFLAGS and profile exactly as it used to by rustc
/// version, so they still stay warm side by side in `targets/default` without
/// churn. Linux cells collide with nothing either: their artifacts land under
/// a target-triple subdirectory of their own.
const fn cell_toolchain(target: CellTarget) -> &'static str {
    if target.is_linux() {
        BROWSERPOD_TOOLCHAIN
    } else {
        CELL_TOOLCHAIN
    }
}

/// The feature flags as a build for `target` actually applies them.
///
/// Every one of atomics, autodiff and SIMD is a statement about building for
/// `wasm32-unknown-unknown`: a `-C target-feature` set, wasm-bindgen-rayon's
/// shared-memory link args, Enzyme's fat-LTO profile. None of them mean
/// anything on `wasm32-browserpod-linux-musl`, whose target spec already
/// carries `+atomics`, shared memory and its export set, and which has no
/// Enzyme component. So the target wins over all three — in ONE place, rather
/// than at each of the four consumers (toolchain, target dir, RUSTFLAGS,
/// `-Zbuild-std`) where three of them would eventually be remembered and one
/// forgotten.
///
/// Note this does not change the cache key: the flags are pure functions of
/// the source and are hashed as such, so a Linux cell that merely *mentions*
/// `std::simd` keys consistently whether or not the flag is applied.
const fn effective_features(
    target: CellTarget,
    needs_atomics: bool,
    needs_autodiff: bool,
    needs_simd: bool,
) -> (bool, bool, bool) {
    if target.is_linux() {
        (false, false, false)
    } else {
        (needs_atomics, needs_autodiff, needs_simd)
    }
}

// The default toolchain for cell builds is [`crate::CELL_TOOLCHAIN`] — every
// cell except rayon/atomics ones compiles on that pin (autodiff cells
// additionally get `-Zautodiff=Enable`, SIMD cells `+simd128`; plain cells
// just run on it). See the const's docs for the rationale and the deploy-image
// requirements.

/// Hard timeout for a single `cargo build` invocation.
/// Override with `IRONPAD_BUILD_TIMEOUT_SECS` env var (default: 300s).
pub(crate) fn build_timeout() -> Duration {
    let secs = std::env::var("IRONPAD_BUILD_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(300);
    Duration::from_secs(secs)
}

// ── Build Result ─────────────────────────────────────────────────────────────

/// Outcome of a micro-crate build attempt.
///
/// Infrastructure errors (spawn failure, timeout) are returned as `Err` from
/// [`build_micro_crate`].  Compilation success vs. failure is represented here
/// so the caller can inspect stdout (JSON diagnostics) in both cases.
pub enum BuildResult {
    /// Compilation succeeded; WASM blob (and JS glue, when there is one) are
    /// on disk.
    Success {
        wasm_path: PathBuf,
        stdout: String,
        stderr: String,
        /// JS glue module generated by `wasm-bindgen` (`--target web`).
        ///
        /// `None` for a Linux cell: its artifact is a process image, not a
        /// module the browser imports, so no wasm-bindgen stage runs. Modelled
        /// as an `Option` rather than an empty string so a caller cannot cache
        /// or hand out glue that does not exist.
        js_glue: Option<String>,
    },
    /// Compilation failed (non-zero exit); stdout contains JSON diagnostics.
    Failure { stdout: String, stderr: String },
}

// ── Public API ───────────────────────────────────────────────────────────────

/// Build a scaffolded micro-crate to WASM.
///
/// Runs `cargo build --target {triple} --release --message-format=json` in
/// `crate_dir` for the cell's [`CellTarget`], with:
///
/// * `CARGO_HOME` → shared registry cache under `cache_dir`
/// * `CARGO_TARGET_DIR` → per-session directory for incremental reuse
///
/// Returns [`BuildResult::Success`] with the artifact path on success, or
/// [`BuildResult::Failure`] with raw cargo output on compilation failure.
/// Ordinary cells additionally carry wasm-bindgen JS glue; Linux cells carry
/// none, and their artifact is the linked executable itself.
///
/// # Errors
///
/// Returns `Err` for infrastructure problems: failed to spawn cargo, build
/// timeout exceeded, or a missing artifact after a successful exit code.
#[allow(clippy::too_many_arguments)]
pub async fn build_micro_crate(
    crate_dir: &Path,
    cache_dir: &Path,
    session_id: &str,
    cell_id: &str,
    compilation_proxy: Option<&str>,
    target: CellTarget,
    needs_atomics: bool,
    needs_autodiff: bool,
    needs_simd: bool,
) -> anyhow::Result<BuildResult> {
    // The target decides which feature flags survive, once, before anything
    // reads them: the target dir below, the log line, and the cargo
    // invocation must all describe the same build.
    let (needs_atomics, needs_autodiff, needs_simd) =
        effective_features(target, needs_atomics, needs_autodiff, needs_simd);

    let cargo_home = cargo_home_dir(cache_dir);
    let target_dir = if needs_atomics {
        atomics_target_dir(cache_dir)
    } else {
        target_dir(cache_dir, session_id)
    };

    tokio::fs::create_dir_all(&cargo_home).await?;
    tokio::fs::create_dir_all(&target_dir).await?;

    // Canonicalize paths so they resolve correctly when cargo runs in crate_dir.
    let cargo_home = tokio::fs::canonicalize(&cargo_home).await?;
    let target_dir = tokio::fs::canonicalize(&target_dir).await?;

    tracing::info!(
        cell_id = %cell_id,
        crate_dir = %crate_dir.display(),
        cargo_home = %cargo_home.display(),
        target_dir = %target_dir.display(),
        target = %target.triple(),
        needs_atomics = needs_atomics,
        needs_autodiff = needs_autodiff,
        needs_simd = needs_simd,
        rustup_toolchain = %std::env::var("RUSTUP_TOOLCHAIN").unwrap_or_default(),
        "starting WASM build",
    );

    let timeout = build_timeout();

    let mut cmd = Command::new("cargo");
    configure_cargo_cmd(
        &mut cmd,
        "build",
        crate_dir,
        &cargo_home,
        &target_dir,
        compilation_proxy,
        target,
        needs_atomics,
        needs_autodiff,
        needs_simd,
    );
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Child span around the cargo subprocess, so a trace separates compile
    // time from the scaffold/wasm-bindgen/wasm-opt stages around it.
    let output = async {
        match run_group_with_timeout(cmd, timeout).await {
            Ok(Some(output)) => Ok(output),
            Ok(None) => {
                tracing::error!(cell_id = %cell_id, "cargo build timed out after {}s", timeout.as_secs());
                anyhow::bail!("compilation timed out after {}s", timeout.as_secs());
            }
            Err(e) => {
                tracing::error!(cell_id = %cell_id, error = %e, "cargo build failed to run");
                Err(e.context("cargo build failed"))
            }
        }
    }
    .instrument(tracing::info_span!("cargo_build", cell_id = %cell_id))
    .await?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        tracing::warn!(
            cell_id = %cell_id,
            exit_code = ?output.status.code(),
            stdout = %stdout,
            stderr = %stderr,
            "cargo build failed",
        );
        return Ok(BuildResult::Failure { stdout, stderr });
    }

    let wasm_path = expected_wasm_path(&target_dir, cell_id, target);

    anyhow::ensure!(
        wasm_path.exists(),
        "WASM blob not found at expected path: {}",
        wasm_path.display(),
    );

    if target.is_linux() {
        // A Linux cell's artifact is already what the pod runs: a linked
        // executable exporting `_start`. wasm-bindgen has nothing to do here
        // (there are no `#[wasm_bindgen]` items and no JS boundary) and would
        // only rewrite a binary whose shape the kernel depends on.
        tracing::info!(
            wasm_path = %wasm_path.display(),
            "Linux build succeeded (no wasm-bindgen stage)",
        );
        return Ok(BuildResult::Success {
            wasm_path,
            stdout,
            stderr,
            js_glue: None,
        });
    }

    tracing::info!(wasm_path = %wasm_path.display(), "WASM build succeeded, running wasm-bindgen");

    // Post-process with wasm-bindgen to generate JS glue + transformed WASM.
    let wasm_bindgen_out_dir = crate_dir.join("wasm-bindgen-out");
    std::fs::create_dir_all(&wasm_bindgen_out_dir)?;

    let mut wb_cmd = tokio::process::Command::new("wasm-bindgen");
    wb_cmd
        .arg("--target")
        .arg("web")
        .arg("--out-dir")
        .arg(&wasm_bindgen_out_dir)
        .arg("--no-typescript")
        .arg(&wasm_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    // Bounded like every other subprocess in this pipeline: a wedged
    // wasm-bindgen (pathological blob, fs stall) must not hold the cell's
    // compile slot forever.
    let wb_output = run_group_with_timeout(wb_cmd, WASM_BINDGEN_TIMEOUT)
        .instrument(tracing::info_span!("wasm_bindgen", cell_id = %cell_id))
        .await
        .context("wasm-bindgen CLI not found. Install it with: cargo install wasm-bindgen-cli")?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "wasm-bindgen timed out after {}s",
                WASM_BINDGEN_TIMEOUT.as_secs()
            )
        })?;

    if !wb_output.status.success() {
        let wb_stderr = String::from_utf8_lossy(&wb_output.stderr);
        anyhow::bail!("wasm-bindgen failed: {wb_stderr}");
    }

    // wasm-bindgen converts hyphens to underscores in output filenames.
    let cell_crate_name = format!("cell_{}", cell_id.replace('-', "_"));
    let js_glue_path = wasm_bindgen_out_dir.join(format!("{cell_crate_name}.js"));
    let bg_wasm_path = wasm_bindgen_out_dir.join(format!("{cell_crate_name}_bg.wasm"));

    let js_glue =
        std::fs::read_to_string(&js_glue_path).context("Failed to read wasm-bindgen JS glue")?;

    anyhow::ensure!(
        bg_wasm_path.exists(),
        "wasm-bindgen transformed WASM not found at: {}",
        bg_wasm_path.display(),
    );

    tracing::info!(
        js_glue_len = js_glue.len(),
        bg_wasm = %bg_wasm_path.display(),
        "wasm-bindgen post-processing succeeded",
    );

    Ok(BuildResult::Success {
        wasm_path: bg_wasm_path,
        stdout,
        stderr,
        js_glue: Some(js_glue),
    })
}

/// Apply the cargo invocation shared by [`build_micro_crate`] and
/// [`check_micro_crate`]: toolchain selection, the `{subcommand} --target
/// {triple} --release --message-format=json` args,
/// `CARGO_HOME`/`CARGO_TARGET_DIR`, host-flag scrubbing, the optional compile
/// proxy, and the atomics/shared-memory flags for rayon cells.
///
/// Extracting this keeps the build and check entry points from silently
/// drifting — e.g. an `env_remove` added later for correctness lands in both the
/// production build and the `cargo check` guard behind
/// `all_public_notebook_cells_compile`. Stdio/process-group setup stays with the
/// caller since only the real build needs it.
///
/// The feature flags must already have passed through [`effective_features`]
/// (both entry points apply it first), so what is set here is what the build
/// actually gets.
#[allow(clippy::too_many_arguments)]
fn configure_cargo_cmd(
    cmd: &mut Command,
    subcommand: &str,
    crate_dir: &Path,
    cargo_home: &Path,
    target_dir: &Path,
    compilation_proxy: Option<&str>,
    target: CellTarget,
    needs_atomics: bool,
    needs_autodiff: bool,
    needs_simd: bool,
) {
    // Every cell build pins its toolchain explicitly — never the host default,
    // which differs between dev (nightly) and the deploy image, and once let
    // nightly-only cells validate green locally and fail on prod.
    let toolchain = cell_toolchain(target);
    cmd.arg(format!("+{toolchain}"));
    // Ensure the rustup shim respects our +toolchain over any inherited
    // override (e.g. RUSTUP_TOOLCHAIN set by the parent process).
    cmd.env_remove("RUSTUP_TOOLCHAIN");

    cmd.arg(subcommand)
        .arg("--target")
        .arg(target.triple())
        .arg("--release")
        .arg("--message-format=json")
        .current_dir(crate_dir)
        .env("CARGO_HOME", cargo_home)
        .env("CARGO_TARGET_DIR", target_dir)
        // Clear host-target flags that may leak from the parent process
        // (e.g. cargo-leptos setting RUSTFLAGS with `-fuse-ld=mold`).
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env_remove("CARGO_BUILD_RUSTFLAGS");

    if let Some(proxy) = compilation_proxy {
        cmd.env("HTTPS_PROXY", proxy);
        cmd.env("HTTP_PROXY", proxy);
    }

    if needs_atomics {
        cmd.arg("-Zbuild-std=std,panic_abort");
    }
    if let Some(rustflags) = compose_rustflags(needs_atomics, needs_autodiff, needs_simd) {
        cmd.env("RUSTFLAGS", rustflags);
    }
}

/// Compose the cell-build `RUSTFLAGS` for the requested feature set, or `None`
/// when no flags are needed.
///
/// Target features from independent concerns (atomics, simd) are merged into a
/// **single** `-C target-feature=` flag: rustc keeps only the last occurrence
/// of the option, so emitting two would silently drop the earlier feature set
/// (a rayon+simd cell would lose its atomics features and fail to link).
fn compose_rustflags(
    needs_atomics: bool,
    needs_autodiff: bool,
    needs_simd: bool,
) -> Option<String> {
    let mut target_features: Vec<&str> = Vec::new();
    if needs_atomics {
        target_features.push(ATOMICS_TARGET_FEATURES);
    }
    if needs_simd {
        target_features.push(SIMD_TARGET_FEATURES);
    }

    let mut rustflags: Vec<String> = Vec::new();
    if !target_features.is_empty() {
        rustflags.push(format!("-C target-feature={}", target_features.join(",")));
    }
    if needs_atomics {
        rustflags.push(ATOMICS_LINK_RUSTFLAGS.to_string());
    }
    if needs_autodiff {
        rustflags.push(AUTODIFF_RUSTFLAGS.to_string());
    }

    if rustflags.is_empty() {
        None
    } else {
        Some(rustflags.join(" "))
    }
}

/// Check (type-check only) a scaffolded micro-crate without full codegen.
///
/// Runs `cargo check --target wasm32-unknown-unknown --release
/// --message-format=json`.  Much faster than [`build_micro_crate`] because it
/// skips LLVM codegen, WASM linking, and wasm-bindgen post-processing.
///
/// Two consumers with different patience: the notebook gate passes
/// [`build_timeout`] (a cold dep tree is legitimate there), and the live
/// check-on-type path passes a short budget so a misclassified cold check
/// degrades to "no markers this round" instead of a hang (PRD-0045). On
/// timeout the whole process GROUP is killed ([`run_group_with_timeout`],
/// same discipline as the build path): killing only cargo would orphan its
/// rustc children, which keep burning CPU outside the admission caps and
/// keep writing to the shared target dir after the per-cell lock is
/// released — racing the next check or build that enters it. Cold-tree
/// timeouts are an EXPECTED path here, not an edge case.
#[allow(clippy::too_many_arguments)]
pub async fn check_micro_crate(
    crate_dir: &Path,
    cache_dir: &Path,
    session_id: &str,
    cell_id: &str,
    compilation_proxy: Option<&str>,
    target: CellTarget,
    needs_atomics: bool,
    needs_autodiff: bool,
    needs_simd: bool,
    timeout: Duration,
) -> anyhow::Result<CheckResult> {
    // Same rule as the build path, applied before anything reads the flags.
    let (needs_atomics, needs_autodiff, needs_simd) =
        effective_features(target, needs_atomics, needs_autodiff, needs_simd);

    let cargo_home = cargo_home_dir(cache_dir);
    let target_dir = if needs_atomics {
        atomics_target_dir(cache_dir)
    } else {
        target_dir(cache_dir, session_id)
    };

    // Async fs like the build path: this is the latency-sensitive
    // check-on-type route, the one that most needs to not block a worker.
    tokio::fs::create_dir_all(&cargo_home).await?;
    tokio::fs::create_dir_all(&target_dir).await?;

    let cargo_home = tokio::fs::canonicalize(&cargo_home).await?;
    let target_dir = tokio::fs::canonicalize(&target_dir).await?;

    tracing::debug!(
        cell_id = %cell_id,
        target = %target.triple(),
        needs_atomics,
        needs_autodiff,
        needs_simd,
        "starting live check",
    );

    let mut cmd = Command::new("cargo");
    configure_cargo_cmd(
        &mut cmd,
        "check",
        crate_dir,
        &cargo_home,
        &target_dir,
        compilation_proxy,
        target,
        needs_atomics,
        needs_autodiff,
        needs_simd,
    );
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());

    let output = run_group_with_timeout(cmd, timeout)
        .instrument(tracing::info_span!("cargo_check", cell_id = %cell_id))
        .await?
        .ok_or(CheckTimedOut(timeout))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if output.status.success() {
        Ok(CheckResult::Ok)
    } else {
        Ok(CheckResult::Failure { stdout, stderr })
    }
}

/// Marker error for a check that exceeded its time budget, so callers can
/// distinguish "too slow right now" (expected for cold caches; degrade
/// gracefully) from real infrastructure failures.
#[derive(Debug, thiserror::Error)]
#[error("cargo check timed out after {}s", .0.as_secs())]
pub struct CheckTimedOut(pub Duration);

/// Outcome of a `cargo check` invocation.
pub enum CheckResult {
    /// Type-checking passed.
    Ok,
    /// Compilation errors; stdout has JSON diagnostics, stderr has human-readable output.
    Failure { stdout: String, stderr: String },
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

/// Shared target directory for atomics-enabled builds.
///
/// All rayon cells share this directory so they benefit from a pre-built
/// std sysroot with atomics support.
pub fn atomics_target_dir(cache_dir: &Path) -> PathBuf {
    cache_dir.join("targets").join("atomics-shared")
}

/// Compute the expected path to the compiled artifact.
///
/// The scaffolded crate is `cell-{cell_id}` (see [`super::scaffold`]) and the
/// two targets name their output differently, because they build different
/// kinds of crate:
///
/// * ordinary cells build a **lib** (`cdylib`), and cargo uplifts lib
///   artifacts with hyphens converted to underscores plus a `.wasm`
///   extension — `cell_a_b.wasm`;
/// * Linux cells build a **bin**, whose artifact keeps the target name
///   verbatim and carries no extension — `cell-a-b`. (Verified against the
///   toolchain rather than assumed: bins do not get the hyphen conversion
///   libs do.)
pub fn expected_wasm_path(target_dir: &Path, cell_id: &str, target: CellTarget) -> PathBuf {
    let crate_name = format!("cell-{cell_id}");
    let artifact = if target.is_linux() {
        crate_name
    } else {
        format!("{}.wasm", crate_name.replace('-', "_"))
    };

    target_dir
        .join(target.triple())
        .join("release")
        .join(artifact)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── compose_rustflags ───────────────────────────────────────────────

    #[test]
    fn rustflags_none_when_no_features() {
        assert_eq!(compose_rustflags(false, false, false), None);
    }

    #[test]
    fn rustflags_simd_only() {
        assert_eq!(
            compose_rustflags(false, false, true).as_deref(),
            Some("-C target-feature=+simd128"),
        );
    }

    #[test]
    fn rustflags_atomics_only_keeps_features_and_link_args() {
        let flags = compose_rustflags(true, false, false).unwrap();
        assert!(flags.starts_with("-C target-feature=+atomics,+bulk-memory,+mutable-globals"));
        assert!(flags.contains("-C link-arg=--shared-memory"));
        assert!(flags.contains("-C link-arg=--export=__tls_base"));
    }

    #[test]
    fn rustflags_atomics_plus_simd_merge_into_one_target_feature_flag() {
        let flags = compose_rustflags(true, false, true).unwrap();
        // One merged flag — a second `-C target-feature=` would make rustc
        // silently drop the first set.
        assert_eq!(flags.matches("-C target-feature=").count(), 1);
        assert!(flags.contains("+atomics,+bulk-memory,+mutable-globals,+simd128"));
        assert!(flags.contains("-C link-arg=--shared-memory"));
    }

    #[test]
    fn rustflags_autodiff_composes_with_simd() {
        let flags = compose_rustflags(false, true, true).unwrap();
        assert!(flags.contains("-C target-feature=+simd128"));
        assert!(flags.contains("-Zautodiff=Enable"));
    }

    /// Every arg of the configured cargo invocation, for a target and feature
    /// combination.
    fn configured_args(
        target: CellTarget,
        atomics: bool,
        autodiff: bool,
        simd: bool,
    ) -> Vec<String> {
        let mut cmd = Command::new("cargo");
        configure_cargo_cmd(
            &mut cmd,
            "build",
            Path::new("/crate"),
            Path::new("/cargo-home"),
            Path::new("/target"),
            None,
            target,
            atomics,
            autodiff,
            simd,
        );
        cmd.as_std()
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    /// The toolchain `+arg` (or its absence) for each feature combination.
    fn selected_toolchain_arg(atomics: bool, autodiff: bool, simd: bool) -> Option<String> {
        configured_args(CellTarget::Executor, atomics, autodiff, simd)
            .into_iter()
            .next()
            .filter(|a| a.starts_with('+'))
    }

    #[test]
    fn every_cell_build_pins_a_toolchain_explicitly() {
        // Never the host default: dev hosts run nightly, the deploy image ran
        // stable, and the divergence once let nightly-only cells (the injected
        // portable_simd gate) validate green locally and fail on prod. Whatever
        // the feature flags, a pinned `+nightly-...` is always present.
        for (atomics, autodiff, simd) in [
            (false, false, false),
            (false, false, true),
            (false, true, false),
            (false, true, true),
            (true, false, false),
            (true, true, true),
        ] {
            let arg = selected_toolchain_arg(atomics, autodiff, simd);
            assert!(
                matches!(arg.as_deref(), Some(a) if a.starts_with("+nightly")),
                "({atomics}, {autodiff}, {simd}) → {arg:?}",
            );
        }
    }

    #[test]
    fn normal_and_simd_cells_use_the_latest_cell_pin() {
        let cell = format!("+{CELL_TOOLCHAIN}");
        assert_eq!(
            selected_toolchain_arg(false, false, false).as_deref(),
            Some(cell.as_str()),
        );
        assert_eq!(
            selected_toolchain_arg(false, false, true).as_deref(),
            Some(cell.as_str()),
        );
    }

    #[test]
    fn every_feature_combination_lands_on_the_one_cell_pin() {
        // PRD-0067: autodiff and rayon used to route to their own nightlies.
        // They still get their own RUSTFLAGS and profile — only the toolchain
        // split collapsed — so the assertion that matters now is that NO
        // feature combination can pull a wasm cell off CELL_TOOLCHAIN.
        let cell = format!("+{CELL_TOOLCHAIN}");
        for atomics in [false, true] {
            for autodiff in [false, true] {
                for simd in [false, true] {
                    assert_eq!(
                        selected_toolchain_arg(atomics, autodiff, simd).as_deref(),
                        Some(cell.as_str()),
                        "({atomics}, {autodiff}, {simd})",
                    );
                }
            }
        }
    }

    #[test]
    fn cell_toolchain_routing() {
        assert_eq!(cell_toolchain(CellTarget::Executor), CELL_TOOLCHAIN);
        assert_eq!(cell_toolchain(CellTarget::Linux), BROWSERPOD_TOOLCHAIN);
    }

    #[test]
    fn the_linux_target_wins_over_every_feature_flag() {
        // `wasm32-browserpod-linux-musl` is buildable by exactly one toolchain,
        // so a Linux cell that happens to declare rayon (or mention
        // `std::autodiff` in a comment) must still get the browserpod pack.
        // Since PRD-0067 the routing takes only the target, which is what makes
        // that true by construction rather than by remembering an `if` order —
        // `effective_features` is where a Linux cell's feature flags are
        // dropped, and it has its own tests.
        assert_eq!(cell_toolchain(CellTarget::Linux), BROWSERPOD_TOOLCHAIN);
        assert_ne!(BROWSERPOD_TOOLCHAIN, CELL_TOOLCHAIN);
    }

    /// The whole image-size argument for PRD-0067 rests on this equality.
    ///
    /// The `BrowserPod` pack does not embed a compiler: its `libexec/rustc` and
    /// `libexec/cargo` are symlinks into the nightly named by its `nightly-pin`
    /// file, which its installer pulls with `--profile minimal`. That nightly
    /// is in the image whether or not anything else uses it, so pointing
    /// `CELL_TOOLCHAIN` at the same date makes every other cell build free.
    ///
    /// Let them drift and nothing breaks — the image just quietly carries two
    /// full toolchains again, which is the state this PRD removed and exactly
    /// the kind of regression no test would otherwise catch.
    #[test]
    fn browserpod_pin_matches_cell_toolchain() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("workspace root");
        let env_file = std::fs::read_to_string(root.join("docker/browserpod.env"))
            .expect("docker/browserpod.env defines the vendored BrowserPod toolchain");
        let pinned = env_file
            .lines()
            .find_map(|line| line.trim().strip_prefix("BROWSERPOD_NIGHTLY="))
            .expect("browserpod.env must record the nightly the pack pins");
        assert_eq!(
            pinned, CELL_TOOLCHAIN,
            "BrowserPod pins {pinned} but cells build on {CELL_TOOLCHAIN}; \
             they must match or the image pays for two full toolchains \
             (PRD-0067). Bumping the pack means re-verifying autodiff and \
             rayon on its nightly, then moving CELL_TOOLCHAIN with it."
        );
    }

    #[test]
    fn linux_builds_carry_no_wasm32_unknown_feature_flags() {
        // atomics/simd/autodiff describe a different architecture: applying
        // them here would fight the target spec's own atomics + shared-memory
        // link args, and `-Zbuild-std` would try to rebuild a std the vendor
        // toolchain already ships.
        assert_eq!(
            effective_features(CellTarget::Linux, true, true, true),
            (false, false, false)
        );
        assert_eq!(
            effective_features(CellTarget::Executor, true, true, true),
            (true, true, true)
        );
        assert_eq!(compose_rustflags(false, false, false), None);
    }

    #[test]
    fn the_target_triple_reaches_the_cargo_invocation() {
        let linux = configured_args(CellTarget::Linux, false, false, false);
        // The pin itself is checked against reality by the integration test,
        // which actually builds with it; here it only has to be the one this
        // module selected.
        let browserpod = format!("+{BROWSERPOD_TOOLCHAIN}");
        assert_eq!(linux.first(), Some(&browserpod));
        let target_pos = linux
            .iter()
            .position(|a| a == "--target")
            .expect("--target");
        assert_eq!(
            linux.get(target_pos + 1).map(String::as_str),
            Some("wasm32-browserpod-linux-musl"),
        );
        assert!(!linux.iter().any(|a| a.starts_with("-Zbuild-std")));

        let wasm = configured_args(CellTarget::Executor, false, false, false);
        let target_pos = wasm.iter().position(|a| a == "--target").expect("--target");
        assert_eq!(
            wasm.get(target_pos + 1).map(String::as_str),
            Some("wasm32-unknown-unknown"),
        );
    }

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

    // ── atomics_target_dir ──────────────────────────────────────────────

    #[test]
    fn atomics_target_dir_is_shared() {
        let dir = atomics_target_dir(Path::new("/cache"));
        assert_eq!(dir, PathBuf::from("/cache/targets/atomics-shared"));
    }

    // ── expected_wasm_path ──────────────────────────────────────────────

    #[test]
    fn wasm_path_simple_id() {
        let path = expected_wasm_path(Path::new("/t"), "abc123", CellTarget::Executor);
        assert_eq!(
            path,
            PathBuf::from("/t/wasm32-unknown-unknown/release/cell_abc123.wasm"),
        );
    }

    #[test]
    fn wasm_path_hyphenated_id() {
        let path = expected_wasm_path(Path::new("/t"), "cell-0", CellTarget::Executor);
        assert_eq!(
            path,
            PathBuf::from("/t/wasm32-unknown-unknown/release/cell_cell_0.wasm"),
        );
    }

    #[test]
    fn wasm_path_nested_hyphens() {
        let path = expected_wasm_path(Path::new("/t"), "a-b-c", CellTarget::Executor);
        assert_eq!(
            path,
            PathBuf::from("/t/wasm32-unknown-unknown/release/cell_a_b_c.wasm"),
        );
    }

    #[test]
    fn wasm_path_underscore_id() {
        let path = expected_wasm_path(Path::new("/t"), "my_cell", CellTarget::Executor);
        assert_eq!(
            path,
            PathBuf::from("/t/wasm32-unknown-unknown/release/cell_my_cell.wasm"),
        );
    }

    #[test]
    fn linux_artifact_is_a_bin_under_its_own_triple() {
        // A bin artifact keeps the package name verbatim: no `-`→`_` uplift
        // (that is a lib-only rule) and no `.wasm` extension. Reading the
        // wrong path here fails the build with "WASM blob not found" AFTER a
        // successful compile, which reads like an infrastructure fault.
        let path = expected_wasm_path(Path::new("/t"), "a-b-c", CellTarget::Linux);
        assert_eq!(
            path,
            PathBuf::from("/t/wasm32-browserpod-linux-musl/release/cell-a-b-c"),
        );
        // The two targets never write to the same file, whatever the id.
        assert_ne!(
            expected_wasm_path(Path::new("/t"), "x", CellTarget::Linux),
            expected_wasm_path(Path::new("/t"), "x", CellTarget::Executor),
        );
    }

    /// A timeout must kill the whole process GROUP, not just the direct
    /// child: cargo fans out rustc children that would otherwise reparent to
    /// init and keep burning CPU / writing to the shared target dir.
    #[cfg(unix)]
    #[tokio::test]
    async fn group_timeout_kills_grandchildren_too() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("grandchild.pid");

        // The shell backgrounds a grandchild, records its pid, then hangs.
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(format!("sleep 30 & echo $! > {}; wait", pid_file.display()));
        let result = run_group_with_timeout(cmd, Duration::from_millis(500))
            .await
            .unwrap();
        assert!(result.is_none(), "the hang must report as a timeout");

        let grandchild: i32 = std::fs::read_to_string(&pid_file)
            .expect("the shell ran long enough to record the grandchild pid")
            .trim()
            .parse()
            .unwrap();
        // SIGKILL delivery is asynchronous; poll briefly for the corpse.
        for _ in 0..40 {
            // SAFETY: kill(pid, 0) only probes for existence.
            let alive = unsafe { libc::kill(grandchild, 0) } == 0;
            if !alive {
                return;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("grandchild {grandchild} survived the group kill");
    }

    /// The subprocess's output is returned intact when it beats the timeout.
    #[cfg(unix)]
    #[tokio::test]
    async fn group_timeout_passes_through_a_fast_exit() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg("echo done")
            .stdout(std::process::Stdio::piped());
        let output = run_group_with_timeout(cmd, Duration::from_secs(10))
            .await
            .unwrap()
            .expect("no timeout");
        assert!(output.status.success());
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "done");
    }

    /// The three cell-toolchain pins are string literals that must appear in
    /// every environment that installs toolchains. Drift here has shipped
    /// breakage before (a pin bumped in Rust but not in the image leaves the
    /// fingerprint on the default rustc and every cell build failing in
    /// prod), so the constants are made authoritative by assertion.
    #[test]
    fn toolchain_pins_are_in_sync_across_dockerfile_ci_and_toolchain_toml() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let dockerfile = std::fs::read_to_string(root.join("docker/Dockerfile")).unwrap();
        let ci = std::fs::read_to_string(root.join(".github/workflows/build.yml")).unwrap();
        let toolchain_toml = std::fs::read_to_string(root.join("rust-toolchain.toml")).unwrap();

        assert!(
            dockerfile.contains(&format!("rustup toolchain install {CELL_TOOLCHAIN}")),
            "docker/Dockerfile does not install CELL_TOOLCHAIN ({CELL_TOOLCHAIN})"
        );
        assert!(
            ci.contains(CELL_TOOLCHAIN),
            ".github/workflows/build.yml does not reference CELL_TOOLCHAIN ({CELL_TOOLCHAIN})"
        );
        // The deploy image's default toolchain is the cell pin.
        assert!(
            dockerfile.contains(&format!("rustup default {CELL_TOOLCHAIN}")),
            "docker/Dockerfile must default to CELL_TOOLCHAIN"
        );
        // The workspace builds on the same nightly cells compile on (PRD-0067).
        // This used to ride ATOMICS_TOOLCHAIN, and the comment above the pin
        // justified it by a `thaw` codegen bug — for a dependency deleted in
        // v0.12.13. Holding the two together is still worth a test, but for a
        // reason that is true: it is one fewer toolchain on a dev box, and it
        // means clippy runs against the compiler cells are judged by.
        assert!(
            toolchain_toml.contains(&format!("channel = \"{CELL_TOOLCHAIN}\"")),
            "rust-toolchain.toml channel must match CELL_TOOLCHAIN"
        );

        // The fourth pin installs from a vendored tarball rather than
        // `rustup toolchain install`, so it is not in the loop above — but it
        // has the same failure mode (constant bumped in Rust, image left
        // behind) and `docker/browserpod.env` is the ONE place its version,
        // sha256 and underlying nightly live. Derive from that file rather
        // than re-literalling the version here, which is what having one
        // source of truth is for.
        let env_file = std::fs::read_to_string(root.join("docker/browserpod.env"))
            .expect("docker/browserpod.env defines the vendored BrowserPod toolchain");
        let version = env_file
            .lines()
            .find_map(|line| line.trim().strip_prefix("BROWSERPOD_VERSION="))
            .expect("browserpod.env must define BROWSERPOD_VERSION");
        assert_eq!(
            BROWSERPOD_TOOLCHAIN,
            format!("browserpod-{version}"),
            "BROWSERPOD_TOOLCHAIN must name the toolchain docker/browserpod.env installs"
        );

        // No unknown pin hiding anywhere: every nightly date literal in the
        // install environments must be one of the three nightly constants.
        // The browserpod pack pulls its own nightly, recorded in that same
        // env file, so it is a known one too.
        let browserpod_nightly = env_file
            .lines()
            .find_map(|line| line.trim().strip_prefix("BROWSERPOD_NIGHTLY="))
            .expect("browserpod.env must record the nightly the pack pins");
        let known = [CELL_TOOLCHAIN, browserpod_nightly];
        for (file, text) in [("docker/Dockerfile", &dockerfile), ("build.yml", &ci)] {
            for (idx, _) in text.match_indices("nightly-20") {
                let pin = &text[idx..(idx + CELL_TOOLCHAIN.len()).min(text.len())];
                assert!(
                    known.contains(&pin),
                    "{file} references unknown toolchain pin {pin}"
                );
            }
        }
    }

    /// The executor's env host-import table must list exactly the imports
    /// ironpad-cell's `#[link(wasm_import_module = "env")]` extern blocks
    /// declare. A name missing from the JS side fails cell instantiation at
    /// runtime (and only on the loading path that happened to lack it, back
    /// when there were three hand-written copies); a stale extra is dead
    /// weight. This turns the "keep in sync" comments into an assertion.
    #[test]
    fn env_host_import_table_matches_the_cell_extern_blocks() {
        use std::collections::BTreeSet;

        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");

        // Rust side: `fn ironpad_*` inside env-linked extern blocks.
        let mut rust_names = BTreeSet::new();
        for file in ["lib.rs", "sim.rs", "gpu.rs", "blocking.rs"] {
            let src =
                std::fs::read_to_string(root.join("crates/ironpad-cell/src").join(file)).unwrap();
            for (attr_idx, _) in src.match_indices("#[link(wasm_import_module = \"env\")]") {
                let block_start = src[attr_idx..].find('{').unwrap() + attr_idx;
                let block_end = src[block_start..].find("\n}").unwrap() + block_start;
                let block = &src[block_start..block_end];
                for (idx, _) in block.match_indices("fn ironpad_") {
                    let name_start = idx + "fn ".len();
                    let name_end = block[name_start..]
                        .find('(')
                        .map(|i| name_start + i)
                        .unwrap();
                    rust_names.insert(block[name_start..name_end].trim().to_string());
                }
            }
        }
        assert!(
            rust_names.len() >= 10,
            "extern scan looks broken: {rust_names:?}"
        );

        // JS side: keys of the generated env-import table.
        let core = std::fs::read_to_string(root.join("public/executor-glue.js")).unwrap();
        let table_start = core
            .find("function _envImportExprs")
            .expect("env-import table function present");
        let table = &core[table_start
            ..core[table_start..]
                .find("\n  }")
                .map(|i| table_start + i)
                .unwrap()];
        let mut js_names = BTreeSet::new();
        for (idx, _) in table.match_indices("\n      ironpad_") {
            let name_start = idx + "\n      ".len();
            let name_end = table[name_start..]
                .find(':')
                .map(|i| name_start + i)
                .unwrap();
            js_names.insert(table[name_start..name_end].trim().to_string());
        }

        assert_eq!(
            rust_names, js_names,
            "ironpad-cell extern blocks and the executor env-import table disagree"
        );
    }
}
