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

    /// GitHub login of this instance's single administrator (PRD-0063).
    ///
    /// Unset (the default) means the admin surface does not exist: `/admin`
    /// is an ordinary not-found and no admin server fn is reachable. It names
    /// who is privileged, never how to authenticate; a matching login still
    /// needs a real signed-in session.
    #[arg(long, env = "IRONPAD_ADMIN_LOGIN")]
    pub admin_login: Option<String>,

    /// `BrowserPod` API key, from `BROWSERPOD_KEY` (PRD-0066).
    ///
    /// Unset (the default) means Linux cells cannot run on this instance and
    /// say so on the cell, rather than booting a pod against nothing. Every
    /// contributor checkout and CI run gets that: a pod boot spends a metered
    /// token on the owner's allowance, so an automated suite must never hold
    /// a key by accident.
    #[arg(long, env = "BROWSERPOD_KEY")]
    pub browserpod_key: Option<String>,

    /// Global cap on concurrent WebSocket guest (agent) connections.
    #[arg(
        long,
        default_value_t = ironpad_server::state::DEFAULT_MAX_GUESTS,
        env = "IRONPAD_MAX_GUESTS"
    )]
    pub max_guests: usize,

    /// Idle timeout (seconds) after which a silent guest connection is reaped.
    #[arg(
        long,
        default_value_t = ironpad_server::state::DEFAULT_GUEST_IDLE_TIMEOUT_SECS,
        env = "IRONPAD_GUEST_IDLE_TIMEOUT_SECS"
    )]
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
            admin_login: args.admin_login,
            browserpod_key: args.browserpod_key,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_login_defaults_to_absent() {
        // The default has to be "no admin surface": an instance that did not
        // opt in must not have one, and that includes every contributor
        // checkout and CI run.
        let config: AppConfig = CliArgs::parse_from(["ironpad"]).into();
        assert_eq!(config.admin_login, None);
    }

    #[test]
    fn admin_login_flows_through_to_app_config() {
        let config: AppConfig =
            CliArgs::parse_from(["ironpad", "--admin-login", "twitchax"]).into();
        assert_eq!(config.admin_login.as_deref(), Some("twitchax"));
    }

    #[test]
    fn browserpod_key_defaults_to_absent() {
        // The invariant that protects the owner's metered allowance. A pod
        // boot costs a token off ~1,000 a month, and the test suite is the
        // dominant consumer if it can boot at all (PRD-0066 T-014), so every
        // contributor checkout and every CI run must come up with no key and
        // therefore no way to reach BrowserPod.
        //
        // Reads the real environment, like the `admin_login` test above it: a
        // shell that has sourced `.hidden/dev.env` will fail this locally, and
        // that is the honest answer rather than a flake, because such a shell
        // really can boot pods.
        let config: AppConfig = CliArgs::parse_from(["ironpad"]).into();
        assert_eq!(config.browserpod_key, None);
    }

    #[test]
    fn browserpod_key_flows_through_to_app_config() {
        let config: AppConfig =
            CliArgs::parse_from(["ironpad", "--browserpod-key", "bp-test"]).into();
        assert_eq!(config.browserpod_key.as_deref(), Some("bp-test"));
    }

    #[test]
    fn test_auth_and_admin_login_may_coexist() {
        // Deliberately allowed, so the e2e suite can drive the admin panel on
        // the same server as everything else.
        //
        // An instance with IRONPAD_TEST_AUTH is already fully compromised:
        // /auth/test-login mints a session for ANY user, so a visitor can
        // already be any existing account and rewrite or delete its notebooks.
        // Admin adds cache clearing on top of that, which is an escalation of
        // something already total. The invariant that protects production is
        // "prod never sets IRONPAD_TEST_AUTH", which is asserted separately by
        // auth::tests::test_login_route_is_env_gated; a second, weaker
        // invariant here bought nothing and cost the admin panel its coverage
        // in the shared suite.
        let config: AppConfig = CliArgs::parse_from([
            "ironpad",
            "--admin-login",
            "twitchax",
            "--test-auth",
            "true",
        ])
        .into();
        assert_eq!(config.admin_login.as_deref(), Some("twitchax"));
    }

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
