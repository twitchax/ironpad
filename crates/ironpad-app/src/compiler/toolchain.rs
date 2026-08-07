//! Host toolchain identity: wasm-bindgen CLI version + rustc version.
//!
//! Cell compilation shells out to a fixed host `wasm-bindgen` CLI
//! (`compiler/build.rs`) to post-process the compiled WASM. If the scaffolded
//! `wasm-bindgen` crate version drifts from the CLI version, wasm-bindgen's
//! exact-schema check fails and every cell breaks. This module reads the CLI
//! version once per process so the scaffold can pin the crate dependency to
//! match (`compiler/scaffold.rs`) and so the compilation cache key can capture
//! toolchain identity (`compiler/cache.rs`).

use std::sync::LazyLock;

/// Env var naming a file of build-time-baked `--version` output.
///
/// A deploy image's toolchain is fixed the moment the image is built, but
/// discovering it at runtime costs a cold `rustc --version`, which demand-pages
/// ~350 MB of `libLLVM` + `librustc_driver` before it can print a version
/// string. Measured on a Fly cold start that single call was 5.9s of a 7.2s
/// boot, paid by every visitor who woke the machine, on pages that never
/// compile anything.
///
/// Line 1 is the raw output of `rustc +CELL_TOOLCHAIN --version`, line 2 the
/// raw output of `wasm-bindgen --version`. Both are read back through the SAME
/// parsers used on live command output, so the derived fingerprint is
/// byte-identical to the probed one and cached blobs survive the change (the
/// fingerprint is part of the compile cache key: a one-byte difference is a
/// full cold cache).
///
/// Unset — dev boxes, CI, tests — probes exactly as before. So does a file
/// that is missing, unreadable, or malformed: correctness first, speed second.
pub const BAKED_VERSIONS_ENV: &str = "IRONPAD_TOOLCHAIN_VERSIONS_FILE";

/// The two raw `--version` lines baked into the image, if any.
struct BakedVersions {
    /// Raw `rustc --version` output, trimmed.
    rustc: String,
    /// Raw `wasm-bindgen --version` output, trimmed (still prefixed).
    wasm_bindgen: String,
}

static BAKED_VERSIONS: LazyLock<Option<BakedVersions>> = LazyLock::new(|| {
    let path = std::env::var_os(BAKED_VERSIONS_ENV)?;
    match std::fs::read_to_string(&path) {
        Ok(text) => parse_baked_versions(&text),
        Err(e) => {
            tracing::warn!(
                path = %std::path::Path::new(&path).display(),
                error = %e,
                "baked toolchain versions unreadable; probing the toolchain instead"
            );
            None
        }
    }
});

/// Split a baked versions file into its two raw `--version` lines, ignoring
/// blank lines. `None` when either line is absent.
fn parse_baked_versions(text: &str) -> Option<BakedVersions> {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    Some(BakedVersions {
        rustc: lines.next()?.to_string(),
        wasm_bindgen: lines.next()?.to_string(),
    })
}

/// Host `wasm-bindgen` CLI version (e.g. `"0.2.126"`), read once per process
/// from the baked file when present, otherwise by shelling out to
/// `wasm-bindgen --version`.
///
/// `None` if the CLI is missing or its output couldn't be parsed. Callers
/// should fall back to a floating version requirement in that case — the
/// build stage already surfaces a clear error if the CLI is truly absent.
static WASM_BINDGEN_CLI_VERSION: LazyLock<Option<String>> = LazyLock::new(|| {
    if let Some(baked) = BAKED_VERSIONS.as_ref() {
        if let Some(version) = parse_wasm_bindgen_version(&baked.wasm_bindgen) {
            return Some(version.to_string());
        }
        tracing::warn!(
            line = %baked.wasm_bindgen,
            "baked wasm-bindgen version unparseable; probing the CLI instead"
        );
    }

    let output = std::process::Command::new("wasm-bindgen")
        .arg("--version")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_wasm_bindgen_version(&stdout).map(str::to_string)
});

/// Toolchain fingerprint: rustc version + wasm-bindgen CLI version, joined by a
/// NUL separator. Stable per process (the toolchain doesn't change mid-run).
///
/// Folded into the compilation cache key (`compiler/cache.rs`) so that
/// upgrading either the rustc toolchain or the wasm-bindgen CLI invalidates
/// stale cached blobs instead of silently serving incompatible output.
///
/// The rustc queried is the pinned [`crate::CELL_TOOLCHAIN`] — the compiler
/// that actually builds cells — NOT the host default, which differs between
/// dev (nightly) and the deploy image and never touches a cell. Falls back to
/// the default `rustc` on hosts without the pin installed (e.g. plain CI
/// runners that never build cells).
static TOOLCHAIN_FINGERPRINT: LazyLock<String> = LazyLock::new(|| {
    let (rustc_version, source) = BAKED_VERSIONS
        .as_ref()
        .map(|baked| (baked.rustc.clone(), "baked"))
        .or_else(|| rustc_version_output(Some(crate::CELL_TOOLCHAIN)).map(|v| (v, "probed")))
        .or_else(|| rustc_version_output(None).map(|v| (v, "probed-default")))
        .unwrap_or_else(|| ("rustc-unknown".to_string(), "unknown"));

    let wasm_bindgen_version = wasm_bindgen_cli_version().unwrap_or("wasm-bindgen-unknown");

    // Logged so a deploy can be checked at a glance: `source="baked"` means the
    // image's bake landed, and `source="probed"` on a deploy image means it did
    // not and the boot is paying the cold `rustc --version` again.
    tracing::info!(
        source,
        rustc = %rustc_version,
        wasm_bindgen = %wasm_bindgen_version,
        "toolchain fingerprint"
    );

    compose_fingerprint(&rustc_version, wasm_bindgen_version)
});

/// Join the two version strings into the fingerprint. One function so the
/// baked and probed paths cannot drift into different separators or ordering.
fn compose_fingerprint(rustc: &str, wasm_bindgen: &str) -> String {
    format!("{rustc}\x00{wasm_bindgen}")
}

/// `rustc [+toolchain] --version` via the rustup shim, or `None` on any
/// failure (missing toolchain, missing rustup, unparseable output).
fn rustc_version_output(toolchain: Option<&str>) -> Option<String> {
    let mut cmd = std::process::Command::new("rustc");
    if let Some(toolchain) = toolchain {
        cmd.arg(format!("+{toolchain}"));
    }
    let output = cmd.arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let version = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if version.is_empty() {
        return None;
    }
    Some(version)
}

/// Parse the version out of `wasm-bindgen --version` output, e.g.
/// `"wasm-bindgen 0.2.126\n"` -> `Some("0.2.126")`.
///
/// Returns `None` if the output doesn't match the expected `wasm-bindgen X.Y.Z`
/// shape, or if the version token isn't a bare dotted-numeric version (e.g. a
/// future CLI printing a build hash like `"0.2.126 (abc1234)"`). Emitting a
/// non-semver token here would produce an invalid exact requirement
/// (`wasm-bindgen = "=0.2.126 (abc1234)"`) that breaks every cell — worse
/// than falling back to the floating `"0.2"` requirement.
fn parse_wasm_bindgen_version(stdout: &str) -> Option<&str> {
    let version = stdout.trim().strip_prefix("wasm-bindgen ")?.trim();
    if version.is_empty() || !is_bare_semver(version) {
        return None;
    }
    Some(version)
}

/// True if `s` looks like a bare dotted-numeric version (e.g. `"0.2.126"`):
/// only ASCII digits and `.`, with no interior whitespace or other trailing
/// content (such as a build hash in parentheses).
fn is_bare_semver(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|byte| byte.is_ascii_digit() || byte == b'.')
}

/// Cached host `wasm-bindgen` CLI version (e.g. `"0.2.126"`), or `None` if it
/// couldn't be determined.
pub fn wasm_bindgen_cli_version() -> Option<&'static str> {
    WASM_BINDGEN_CLI_VERSION.as_deref()
}

/// Cached toolchain fingerprint (rustc version + wasm-bindgen CLI version).
pub fn toolchain_fingerprint() -> &'static str {
    &TOOLCHAIN_FINGERPRINT
}

/// Force both `LazyLock` statics to initialize.
///
/// Their first access shells out to `wasm-bindgen --version` and
/// `rustc --version` via blocking `std::process::Command`. Left to
/// initialize lazily, that first access would happen inside the async
/// `compile_cell` server fn, blocking a tokio worker thread. Call this once
/// at server startup (on a blocking thread) so the async request path never
/// pays the process-spawn cost.
pub fn prewarm() {
    let _ = wasm_bindgen_cli_version();
    let _ = toolchain_fingerprint();
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_wasm_bindgen_version_extracts_version() {
        assert_eq!(
            parse_wasm_bindgen_version("wasm-bindgen 0.2.126\n"),
            Some("0.2.126")
        );
    }

    #[test]
    fn parse_wasm_bindgen_version_trims_trailing_whitespace() {
        assert_eq!(
            parse_wasm_bindgen_version("wasm-bindgen 0.2.100\r\n"),
            Some("0.2.100")
        );
    }

    #[test]
    fn parse_wasm_bindgen_version_handles_bare_input() {
        assert_eq!(
            parse_wasm_bindgen_version("wasm-bindgen 1.0.0"),
            Some("1.0.0")
        );
    }

    #[test]
    fn parse_wasm_bindgen_version_returns_none_for_missing_prefix() {
        assert_eq!(parse_wasm_bindgen_version("not a version string"), None);
    }

    #[test]
    fn parse_wasm_bindgen_version_returns_none_for_empty_input() {
        assert_eq!(parse_wasm_bindgen_version(""), None);
    }

    #[test]
    fn parse_wasm_bindgen_version_returns_none_for_prefix_only() {
        assert_eq!(parse_wasm_bindgen_version("wasm-bindgen "), None);
        assert_eq!(parse_wasm_bindgen_version("wasm-bindgen"), None);
    }

    #[test]
    fn parse_wasm_bindgen_version_returns_none_for_non_semver_suffix() {
        // A hypothetical future CLI appending a build hash must not produce
        // an invalid exact requirement like `=0.2.126 (abc1234)`.
        assert_eq!(
            parse_wasm_bindgen_version("wasm-bindgen 0.2.126 (abc1234)"),
            None
        );
    }

    #[test]
    fn parse_baked_versions_takes_the_first_two_non_blank_lines() {
        let baked =
            parse_baked_versions("rustc 1.99.0-nightly (abc 2026-07-14)\nwasm-bindgen 0.2.114\n")
                .expect("two lines");
        assert_eq!(baked.rustc, "rustc 1.99.0-nightly (abc 2026-07-14)");
        assert_eq!(baked.wasm_bindgen, "wasm-bindgen 0.2.114");
    }

    #[test]
    fn parse_baked_versions_rejects_a_truncated_file() {
        // A half-written file must fall back to probing rather than compose a
        // fingerprint from one real version and one missing one.
        assert!(parse_baked_versions("rustc 1.99.0-nightly (abc 2026-07-14)\n").is_none());
        assert!(parse_baked_versions("").is_none());
        assert!(parse_baked_versions("\n\n").is_none());
    }

    /// The fingerprint is part of the compile cache key, so a baked value that
    /// differs from the probed one by a single byte cold-starts every cached
    /// blob on deploy. Feed real command output through the baked path and
    /// require the identical string back.
    #[test]
    fn a_baked_file_reproduces_the_probed_fingerprint_byte_for_byte() {
        let Some(rustc) = rustc_version_output(Some(crate::CELL_TOOLCHAIN))
            .or_else(|| rustc_version_output(None))
        else {
            return;
        };
        let Some(wasm_bindgen) = wasm_bindgen_cli_version() else {
            return;
        };

        // Exactly what the Dockerfile writes: raw output, one line each.
        let file = format!("{rustc}\nwasm-bindgen {wasm_bindgen}\n");
        let baked = parse_baked_versions(&file).expect("two lines");
        let from_baked = compose_fingerprint(
            &baked.rustc,
            parse_wasm_bindgen_version(&baked.wasm_bindgen).expect("baked wasm-bindgen parses"),
        );

        assert_eq!(from_baked, compose_fingerprint(&rustc, wasm_bindgen));
    }

    #[test]
    fn toolchain_fingerprint_is_stable_across_calls() {
        let a = toolchain_fingerprint();
        let b = toolchain_fingerprint();
        assert_eq!(a, b);
    }

    #[test]
    fn toolchain_fingerprint_is_non_empty() {
        assert!(!toolchain_fingerprint().is_empty());
    }

    #[test]
    fn fingerprint_reports_the_cell_toolchain_when_installed() {
        // On hosts with the pin installed (dev boxes, the deploy image), the
        // fingerprint must describe the rustc that actually builds cells, not
        // the host default. Hosts without it (plain CI runners) exercise the
        // fallback, which this test then skips.
        let Some(pinned) = rustc_version_output(Some(crate::CELL_TOOLCHAIN)) else {
            return;
        };
        assert!(
            toolchain_fingerprint().starts_with(&pinned),
            "fingerprint should lead with the pinned rustc version",
        );
    }
}
