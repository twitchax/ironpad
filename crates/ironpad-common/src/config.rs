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
    /// GitHub login of this instance's single administrator (PRD-0063), from
    /// `IRONPAD_ADMIN_LOGIN`.
    ///
    /// Names WHO is privileged, never HOW to authenticate: the admin surface
    /// still requires an ordinary signed-in session, and this is only compared
    /// against the identity that session already proved. `None` means the
    /// admin surface does not exist on this instance, which is the default and
    /// what every contributor and CI run gets.
    pub admin_login: Option<String>,
    /// `BrowserPod` API key backing Linux cells (PRD-0066), from
    /// `BROWSERPOD_KEY`.
    ///
    /// Unavoidably client-visible — a pod boots in the browser and the SDK
    /// takes the key as a boot argument — so this is deployment identity
    /// rather than a secret in the auth sense. `None` means this instance
    /// runs no Linux cells, which is what every contributor checkout and CI
    /// run gets, and what the cell says on its face instead of failing
    /// somewhere inside a vendor SDK.
    pub browserpod_key: Option<String>,
}

impl AppConfig {
    /// Absolute URL for a root-relative `path` (which must start with `/`).
    #[must_use]
    pub fn absolute_url(&self, path: &str) -> String {
        absolute_url(&self.public_url, path)
    }
}

/// Joins an origin and a root-relative `path`.
///
/// Tolerates a trailing slash on `origin` rather than trusting callers to have
/// normalized it: an operator typo would otherwise ship `//og/...` into every
/// crawler's cache, where it is expensive to walk back.
///
/// Exists as a free function as well as an [`AppConfig`] method because the
/// crawler files are built by pure functions that take an origin string and
/// have no config to hand; both spellings must round to the same bytes, since
/// a sitemap that disagrees with `og:url` about a trailing slash is two URLs
/// to a crawler.
#[must_use]
pub fn absolute_url(origin: &str, path: &str) -> String {
    format!("{}{}", origin.trim_end_matches('/'), path)
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
            admin_login: None,
            browserpod_key: None,
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

    #[test]
    fn the_method_and_the_free_function_agree() {
        // They serve the meta tags and the sitemap respectively, and a crawler
        // treats a trailing-slash difference between them as two distinct URLs.
        for origin in [
            "https://ironpad.twitchax.com",
            "https://ironpad.twitchax.com/",
            "http://localhost:3111",
        ] {
            for path in ["/", "/public/cannon", "/og/ironpad.png"] {
                assert_eq!(
                    config(origin).absolute_url(path),
                    super::absolute_url(origin, path),
                    "{origin} + {path}"
                );
            }
        }
    }
}
