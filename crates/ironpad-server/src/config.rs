//! CLI/environment configuration for the ironpad server binary.
//!
//! Parses process arguments and environment variables (via `clap`) into the
//! shared [`ironpad_common::AppConfig`] consumed by the Axum app and
//! `#[server]` functions.

use std::path::PathBuf;

use clap::Parser;
use ironpad_common::AppConfig;

/// ironpad — Interactive Rust Notebooks
#[derive(Parser, Debug)]
#[command(name = "ironpad", about = "Interactive Rust Notebooks")]
pub struct CliArgs {
    /// Directory for notebook data storage.
    #[arg(long, default_value = "./data", env = "IRONPAD_DATA_DIR")]
    pub data_dir: PathBuf,

    /// Directory for compilation cache.
    #[arg(long, default_value = "./cache", env = "IRONPAD_CACHE_DIR")]
    pub cache_dir: PathBuf,

    /// Port to serve the application on.
    #[arg(long, default_value_t = 3111, env = "IRONPAD_PORT")]
    pub port: u16,

    /// Path to the ironpad-cell crate (injected into user cells as a path dependency).
    #[arg(
        long,
        default_value = "./crates/ironpad-cell",
        env = "IRONPAD_CELL_PATH"
    )]
    pub ironpad_cell_path: PathBuf,

    /// Optional HTTPS proxy URL for cargo builds (e.g., `http://127.0.0.1:3112`).
    /// When set, user cell compilations route through this proxy for domain filtering.
    #[arg(long, env = "IRONPAD_COMPILATION_PROXY")]
    pub compilation_proxy: Option<String>,

    /// Origin this instance is reachable at from the public internet, e.g.
    /// `https://ironpad.twitchax.com`. Defaults to `http://localhost:{port}`.
    ///
    /// Only social-preview metadata and the sitemap consume it, both of which
    /// must emit absolute URLs because crawlers resolve them with no document
    /// base to fall back on.
    #[arg(long, env = "IRONPAD_PUBLIC_URL")]
    pub public_url: Option<String>,

    /// Global cap on concurrent cargo builds (PRD-0052). Compiles queue for a
    /// slot (bounded); live checks shed to Skipped. Cache hits never take one.
    #[arg(long, default_value_t = 3, env = "IRONPAD_MAX_CONCURRENT_BUILDS")]
    pub max_concurrent_builds: usize,

    /// GitHub OAuth app client id (PRD-0053). Sign-in is hidden when either
    /// credential is absent; the instance then runs anonymous-only.
    #[arg(long, env = "GITHUB_CLIENT_ID")]
    pub github_client_id: Option<String>,

    /// GitHub OAuth app client secret (PRD-0053).
    #[arg(long, env = "GITHUB_CLIENT_SECRET")]
    pub github_client_secret: Option<String>,

    /// Register the `/auth/test-login` endpoint (PRD-0053). e2e suites only;
    /// production must NEVER set this — it mints sessions without GitHub.
    #[arg(
        long,
        env = "IRONPAD_TEST_AUTH",
        default_value_t = false,
        num_args = 0..=1,
        default_missing_value = "true",
        value_parser = clap::builder::FalseyValueParser::new()
    )]
    pub test_auth: bool,

    /// Global cap on concurrent WebSocket guest (agent) connections.
    #[arg(long, default_value_t = 512, env = "IRONPAD_MAX_GUESTS")]
    pub max_guests: usize,

    /// Idle timeout (seconds) after which a silent guest connection is reaped.
    #[arg(long, default_value_t = 1800, env = "IRONPAD_GUEST_IDLE_TIMEOUT_SECS")]
    pub guest_idle_timeout_secs: u64,
}

impl From<CliArgs> for AppConfig {
    fn from(args: CliArgs) -> Self {
        // Derived from the resolved port rather than a literal clap default, so
        // `--port 8080` alone still yields a self-consistent origin.
        let public_url = args
            .public_url
            .unwrap_or_else(|| format!("http://localhost:{}", args.port));

        Self {
            data_dir: args.data_dir,
            cache_dir: args.cache_dir,
            port: args.port,
            ironpad_cell_path: args.ironpad_cell_path,
            compilation_proxy: args.compilation_proxy,
            public_url,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_values() {
        let args = CliArgs::parse_from(["ironpad"]);

        assert_eq!(args.data_dir, PathBuf::from("./data"));
        assert_eq!(args.cache_dir, PathBuf::from("./cache"));
        assert_eq!(args.port, 3111);
        assert_eq!(
            args.ironpad_cell_path,
            PathBuf::from("./crates/ironpad-cell")
        );
        assert_eq!(args.compilation_proxy, None);
        assert_eq!(args.public_url, None);
        assert_eq!(args.max_guests, 512);
        assert_eq!(args.guest_idle_timeout_secs, 1800);
        // Auth defaults (PRD-0053): no credentials, and — critically — the
        // test-login gate closed.
        assert_eq!(args.github_client_id, None);
        assert_eq!(args.github_client_secret, None);
        assert!(!args.test_auth);
    }

    #[test]
    fn auth_args_parse() {
        let args = CliArgs::parse_from([
            "ironpad",
            "--github-client-id",
            "Ov23liEXAMPLE",
            "--github-client-secret",
            "s3cr3t",
            "--test-auth",
        ]);
        assert_eq!(args.github_client_id.as_deref(), Some("Ov23liEXAMPLE"));
        assert_eq!(args.github_client_secret.as_deref(), Some("s3cr3t"));
        assert!(args.test_auth);

        // The falsey parser accepts the `IRONPAD_TEST_AUTH=1` env spelling.
        let args = CliArgs::parse_from(["ironpad", "--test-auth", "1"]);
        assert!(args.test_auth);
        let args = CliArgs::parse_from(["ironpad", "--test-auth", "0"]);
        assert!(!args.test_auth);
    }

    #[test]
    fn public_url_defaults_to_the_resolved_port_not_a_literal() {
        let config: AppConfig = CliArgs::parse_from(["ironpad", "--port", "8080"]).into();
        assert_eq!(config.public_url, "http://localhost:8080");
    }

    #[test]
    fn public_url_overrides_the_derived_default() {
        let config: AppConfig =
            CliArgs::parse_from(["ironpad", "--public-url", "https://ironpad.twitchax.com"]).into();
        assert_eq!(config.public_url, "https://ironpad.twitchax.com");
        assert_eq!(
            config.absolute_url("/og/public/cannon.png"),
            "https://ironpad.twitchax.com/og/public/cannon.png"
        );
    }

    #[test]
    fn relay_knobs_override() {
        let args = CliArgs::parse_from([
            "ironpad",
            "--max-guests",
            "64",
            "--guest-idle-timeout-secs",
            "300",
        ]);
        assert_eq!(args.max_guests, 64);
        assert_eq!(args.guest_idle_timeout_secs, 300);
    }

    #[test]
    fn cli_args_override() {
        let args = CliArgs::parse_from([
            "ironpad",
            "--data-dir",
            "/tmp/ironpad-data",
            "--cache-dir",
            "/tmp/ironpad-cache",
            "--port",
            "8080",
            "--ironpad-cell-path",
            "/opt/ironpad-cell",
            "--compilation-proxy",
            "http://127.0.0.1:3112",
        ]);

        assert_eq!(args.data_dir, PathBuf::from("/tmp/ironpad-data"));
        assert_eq!(args.cache_dir, PathBuf::from("/tmp/ironpad-cache"));
        assert_eq!(args.port, 8080);
        assert_eq!(args.ironpad_cell_path, PathBuf::from("/opt/ironpad-cell"));
        assert_eq!(
            args.compilation_proxy,
            Some("http://127.0.0.1:3112".to_string())
        );
    }

    #[test]
    fn conversion_to_app_config() {
        let args = CliArgs::parse_from(["ironpad", "--data-dir", "/data", "--port", "9090"]);
        let config: AppConfig = args.into();

        assert_eq!(config.data_dir, PathBuf::from("/data"));
        assert_eq!(config.cache_dir, PathBuf::from("./cache"));
        assert_eq!(config.port, 9090);
        assert_eq!(
            config.ironpad_cell_path,
            PathBuf::from("./crates/ironpad-cell")
        );
        assert_eq!(config.compilation_proxy, None);
    }
}
