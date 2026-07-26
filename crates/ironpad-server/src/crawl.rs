//! `robots.txt` and `sitemap.xml`.
//!
//! Both were 404s until PRD-0050, which meant the showcase notebooks were
//! reachable only by crawling the home page's client-rendered list.
//!
//! Note what is deliberately *not* here: `Disallow` rules for `/shared` and
//! `/mutable`. Keeping unlisted notebooks out of a search index is the right
//! default, but `robots.txt` is the wrong instrument for it, because several
//! unfurlers (Twitterbot among them) honour it and would then refuse to build
//! a preview at all. Those pages carry `<meta name="robots" content="noindex">`
//! instead, which search engines obey and unfurlers ignore, so a shared link
//! still gets a card without landing in Google.

use std::fmt::Write as _;

use axum::extract::State;
use axum::http::header;
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

/// `/embed/*` renders the same notebooks as `/public` and `/shared` without
/// the surrounding chrome. Indexing both would be duplicate content, and the
/// embed variant is the one no human should land on from a search result.
const DISALLOWED: &[&str] = &["/embed/"];

/// Builds `robots.txt` for an origin.
#[must_use]
pub fn robots_txt(public_url: &str) -> String {
    let mut out = String::from("User-agent: *\n");
    for path in DISALLOWED {
        let _ = writeln!(out, "Disallow: {path}");
    }
    out.push_str("Allow: /\n\n");
    let _ = writeln!(
        out,
        "Sitemap: {}/sitemap.xml",
        public_url.trim_end_matches('/')
    );
    out
}

/// Builds `sitemap.xml` from the home page plus every public notebook.
///
/// `filenames` are as stored (`cannon.ironpad`); the canonical route is
/// extension-less (PRD-0048), so the suffix is stripped here. Only `/public`
/// appears: `/local` exists solely in one browser's `IndexedDB`, and `/shared`
/// and `/mutable` are unlisted by design.
#[must_use]
pub fn sitemap_xml(public_url: &str, filenames: &[String]) -> String {
    let origin = public_url.trim_end_matches('/');
    let mut out = String::from(r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    out.push('\n');
    out.push_str(r#"<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">"#);
    out.push('\n');
    let _ = writeln!(out, "  <url><loc>{}/</loc></url>", escape(origin));

    for filename in filenames {
        let name = filename.strip_suffix(".ironpad").unwrap_or(filename);
        let _ = writeln!(
            out,
            "  <url><loc>{}/public/{}</loc></url>",
            escape(origin),
            escape(name)
        );
    }

    out.push_str("</urlset>\n");
    out
}

/// XML-escapes a URL for inclusion in a `<loc>`.
///
/// Notebook filenames come off disk rather than from a request, but a `&` in
/// one would still produce a malformed sitemap that a crawler silently drops.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

// ── Handlers ────────────────────────────────────────────────────────────────

/// `GET /robots.txt`
#[allow(clippy::unused_async)] // axum handlers must be async to be routable.
pub async fn robots_handler(State(state): State<AppState>) -> Response {
    (
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        robots_txt(&state.config.public_url),
    )
        .into_response()
}

/// `GET /sitemap.xml`
pub async fn sitemap_handler(State(state): State<AppState>) -> Response {
    let site_root = std::path::Path::new(state.leptos_options.site_root.as_ref()).to_path_buf();
    let filenames = ironpad_app::server_fns::list_public_notebooks_core(&site_root)
        .await
        .map(|list| list.into_iter().map(|n| n.filename).collect::<Vec<_>>())
        .unwrap_or_default();

    (
        [(header::CONTENT_TYPE, "application/xml; charset=utf-8")],
        sitemap_xml(&state.config.public_url, &filenames),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "https://ironpad.twitchax.com";

    #[test]
    fn robots_points_at_the_sitemap_on_the_configured_origin() {
        let txt = robots_txt(ORIGIN);
        assert!(txt.contains("User-agent: *"));
        assert!(txt.contains("Sitemap: https://ironpad.twitchax.com/sitemap.xml"));
        assert!(txt.ends_with('\n'));
    }

    #[test]
    fn robots_tolerates_a_trailing_slash_on_the_origin() {
        assert!(robots_txt("https://ironpad.twitchax.com/")
            .contains("Sitemap: https://ironpad.twitchax.com/sitemap.xml"));
    }

    #[test]
    fn robots_hides_only_the_embed_surface() {
        let txt = robots_txt(ORIGIN);
        assert!(txt.contains("Disallow: /embed/"));
        // Blocking these here would stop Twitterbot fetching the page at all,
        // and with it the preview card the noindex approach preserves.
        assert!(!txt.contains("Disallow: /shared"));
        assert!(!txt.contains("Disallow: /mutable"));
    }

    #[test]
    fn sitemap_lists_the_home_page_and_extension_less_notebook_routes() {
        let xml = sitemap_xml(
            ORIGIN,
            &["cannon.ironpad".to_string(), "welcome.ironpad".to_string()],
        );
        assert!(xml.starts_with(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
        assert!(xml.contains("<loc>https://ironpad.twitchax.com/</loc>"));
        assert!(xml.contains("<loc>https://ironpad.twitchax.com/public/cannon</loc>"));
        assert!(xml.contains("<loc>https://ironpad.twitchax.com/public/welcome</loc>"));
        // The legacy extension-carrying form must not appear: it redirects,
        // and a sitemap full of redirects is a crawl-budget tax.
        assert!(!xml.contains(".ironpad"));
        assert!(xml.trim_end().ends_with("</urlset>"));
    }

    #[test]
    fn an_empty_site_still_produces_a_valid_sitemap() {
        let xml = sitemap_xml(ORIGIN, &[]);
        assert!(xml.contains("<loc>https://ironpad.twitchax.com/</loc>"));
        assert!(xml.trim_end().ends_with("</urlset>"));
    }

    #[test]
    fn sitemap_escapes_xml_metacharacters_in_names() {
        let xml = sitemap_xml(ORIGIN, &["a&b.ironpad".to_string()]);
        assert!(xml.contains("/public/a&amp;b"));
        assert!(!xml.contains("/public/a&b"));
    }
}
