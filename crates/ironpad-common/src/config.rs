//! Shared application configuration.
//!
//! [`AppConfig`] is derived from the server's CLI/env and provided via Leptos
//! context so `#[server]` functions can read it with `expect_context`.

use std::path::PathBuf;

/// Application configuration, derived from CLI arguments and environment variables.
///
/// Provided via Leptos context on the server side so that `#[server]` functions
/// can access it with `expect_context::<AppConfig>()`.
#[derive(Clone, Debug)]
pub struct AppConfig {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub port: u16,
    pub ironpad_cell_path: PathBuf,
    /// Optional HTTPS proxy URL for cargo builds (e.g., `http://127.0.0.1:3112`).
    /// When set, user cell compilations route through this proxy for domain filtering.
    pub compilation_proxy: Option<String>,
    /// Origin this instance is reachable at from the public internet, e.g.
    /// `https://ironpad.twitchax.com`. Defaults to `http://localhost:{port}`.
    ///
    /// Social-preview crawlers resolve `og:image` and `og:url` without a
    /// document base, so those tags must carry absolute URLs; the sitemap has
    /// the same requirement. Nothing else in the app needs it, which is why
    /// this is the only piece of deployment identity the server knows about.
    pub public_url: String,
}

impl AppConfig {
    /// Absolute URL for a root-relative `path` (which must start with `/`).
    ///
    /// Tolerates a trailing slash on `public_url` rather than trusting callers
    /// to have normalized it: an operator typo would otherwise ship `//og/...`
    /// into every crawler's cache, where it is expensive to walk back.
    #[must_use]
    pub fn absolute_url(&self, path: &str) -> String {
        format!("{}{}", self.public_url.trim_end_matches('/'), path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(public_url: &str) -> AppConfig {
        AppConfig {
            data_dir: PathBuf::from("/data"),
            cache_dir: PathBuf::from("/cache"),
            port: 3111,
            ironpad_cell_path: PathBuf::from("/cell"),
            compilation_proxy: None,
            public_url: public_url.to_string(),
        }
    }

    #[test]
    fn absolute_url_joins_origin_and_path() {
        assert_eq!(
            config("https://ironpad.twitchax.com").absolute_url("/public/cannon"),
            "https://ironpad.twitchax.com/public/cannon"
        );
    }

    #[test]
    fn absolute_url_tolerates_a_trailing_slash_on_the_origin() {
        assert_eq!(
            config("https://ironpad.twitchax.com/").absolute_url("/og/public/cannon.png"),
            "https://ironpad.twitchax.com/og/public/cannon.png"
        );
    }
}
