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

/// Host `wasm-bindgen` CLI version (e.g. `"0.2.126"`), read once per process by
/// shelling out to `wasm-bindgen --version`.
///
/// `None` if the CLI is missing or its output couldn't be parsed. Callers
/// should fall back to a floating version requirement in that case — the
/// build stage already surfaces a clear error if the CLI is truly absent.
static WASM_BINDGEN_CLI_VERSION: LazyLock<Option<String>> = LazyLock::new(|| {
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
static TOOLCHAIN_FINGERPRINT: LazyLock<String> = LazyLock::new(|| {
    let rustc_version = std::process::Command::new("rustc")
        .arg("--version")
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map_or_else(
            || "rustc-unknown".to_string(),
            |output| String::from_utf8_lossy(&output.stdout).trim().to_string(),
        );

    let wasm_bindgen_version = wasm_bindgen_cli_version().unwrap_or("wasm-bindgen-unknown");

    format!("{rustc_version}\x00{wasm_bindgen_version}")
});

/// Parse the version out of `wasm-bindgen --version` output, e.g.
/// `"wasm-bindgen 0.2.126\n"` -> `Some("0.2.126")`.
///
/// Returns `None` if the output doesn't match the expected `wasm-bindgen X.Y.Z`
/// shape.
fn parse_wasm_bindgen_version(stdout: &str) -> Option<&str> {
    let version = stdout.trim().strip_prefix("wasm-bindgen ")?.trim();
    if version.is_empty() {
        return None;
    }
    Some(version)
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
    fn toolchain_fingerprint_is_stable_across_calls() {
        let a = toolchain_fingerprint();
        let b = toolchain_fingerprint();
        assert_eq!(a, b);
    }

    #[test]
    fn toolchain_fingerprint_is_non_empty() {
        assert!(!toolchain_fingerprint().is_empty());
    }
}
