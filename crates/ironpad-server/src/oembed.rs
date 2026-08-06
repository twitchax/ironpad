//! oEmbed provider endpoint: `GET /oembed?url=…` (PRD-0051).
//!
//! Open Graph gets a link a *picture*. oEmbed gets it the actual notebook:
//! a consumer that supports discovery (`WordPress`, Ghost, Notion, Confluence,
//! and friends) sees the `<link rel="alternate" type="application/json+oembed">`
//! this pairs with, fetches this endpoint, and embeds the returned iframe
//! rather than a static card. The `/embed/*` routes from PRD-0039 already
//! render exactly that iframe, so this is a mapping from a canonical notebook
//! URL to an embed URL, plus the notebook's title.
//!
//! One limit worth knowing: a `/local` URL is private by construction, so it
//! resolves to nothing here rather than to a broken frame. And the big social
//! consumers (X, Reddit, Slack) use their own allowlists rather than
//! discovery, so this changes nothing about how a link looks *there*, which is
//! what the Open Graph tags are for.

use axum::extract::{Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use ironpad_common::absolute_url;
use serde::{Deserialize, Serialize};

use crate::state::AppState;

/// Default iframe height, matching the Embed snippet the view-only toolbar
/// copies. Notebooks self-report their height to the parent frame via
/// `embed-frame.js`, so this is only the size before that first message.
const DEFAULT_HEIGHT: u32 = 600;
const DEFAULT_WIDTH: u32 = 800;

/// Clamp on consumer-supplied `maxheight`, so a hostile or careless value
/// cannot produce an absurd frame.
const MIN_HEIGHT: u32 = 150;
const MAX_HEIGHT: u32 = 4000;

/// How long a consumer may cache this response, in seconds. Public notebooks
/// change only at deploy; shared ones are immutable.
const CACHE_AGE: u32 = 86_400;

// ── Request ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct OembedQuery {
    url: String,
    /// `json` (the default) or `xml`. The spec requires a 501 for a format the
    /// provider does not implement, rather than silently returning JSON.
    format: Option<String>,
    maxheight: Option<u32>,
}

// ── Response ────────────────────────────────────────────────────────────────

/// An oEmbed `rich` response, per the 1.0 spec.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct Oembed {
    version: &'static str,
    #[serde(rename = "type")]
    kind: &'static str,
    provider_name: &'static str,
    provider_url: String,
    title: String,
    html: String,
    width: u32,
    height: u32,
    cache_age: u32,
}

// ── URL → embed mapping ─────────────────────────────────────────────────────

/// A notebook this provider can hand back an embed for.
#[derive(Debug, PartialEq, Eq)]
pub struct EmbedTarget {
    /// Storage class segment: `public`, `shared`, or `mutable` (PRD-0057).
    pub class: &'static str,
    /// The notebook's id within that class.
    pub id: String,
}

impl EmbedTarget {
    /// The `/embed/…` path that renders this notebook chrome-less.
    fn embed_path(&self) -> String {
        format!("/embed/{}/{}", self.class, self.id)
    }
}

/// Maps a canonical notebook URL on this origin to an embeddable target.
///
/// Rejects anything not on `public_url`. A provider that embedded arbitrary
/// URLs would be an open redirect wearing an iframe: a consumer trusts the
/// returned HTML because it trusts *this* provider.
#[must_use]
pub fn embed_target(public_url: &str, url: &str) -> Option<EmbedTarget> {
    let origin = public_url.trim_end_matches('/');
    let path = url.strip_prefix(origin)?;
    // Strip the query and fragment before matching, so `?x=1` doesn't become
    // part of a notebook name.
    let path = path.split(['?', '#']).next()?;

    let (class, id) = path
        .strip_prefix("/public/")
        .map(|id| ("public", id))
        .or_else(|| path.strip_prefix("/shared/").map(|id| ("shared", id)))
        .or_else(|| path.strip_prefix("/mutable/").map(|id| ("mutable", id)))?;

    // The canonical public route is extension-less (PRD-0048), but the legacy
    // form still resolves and people paste what their address bar shows.
    let id = id.strip_suffix(".ironpad").unwrap_or(id);

    if id.is_empty() || id.contains('/') || id.contains('\\') || id.contains("..") {
        return None;
    }

    Some(EmbedTarget {
        class,
        id: id.to_string(),
    })
}

/// Builds the response body for a resolved target.
#[must_use]
pub fn response_for(
    public_url: &str,
    target: &EmbedTarget,
    title: String,
    maxheight: Option<u32>,
) -> Oembed {
    let height = maxheight
        .unwrap_or(DEFAULT_HEIGHT)
        .clamp(MIN_HEIGHT, MAX_HEIGHT);
    let src = absolute_url(public_url, &target.embed_path());

    Oembed {
        version: "1.0",
        kind: "rich",
        provider_name: "ironpad",
        provider_url: absolute_url(public_url, "/"),
        // Escaped because a shared notebook's title is attacker-controlled and
        // this string is embedded verbatim into the consumer's page.
        html: format!(
            r#"<iframe src="{}" style="width:100%;border:0;" height="{height}" loading="lazy" title="{}"></iframe>"#,
            escape_attr(&src),
            escape_attr(&title),
        ),
        title,
        width: DEFAULT_WIDTH,
        height,
        cache_age: CACHE_AGE,
    }
}

/// Escapes a string for use inside a double-quoted HTML attribute.
fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

// ── Handler ─────────────────────────────────────────────────────────────────

/// `GET /oembed?url=…&format=json&maxheight=…`
pub async fn oembed_handler(
    State(state): State<AppState>,
    axum::Extension(db): axum::Extension<ironpad_app::db::Db>,
    Query(query): Query<OembedQuery>,
) -> Response {
    // The spec is explicit that an unsupported format is a 501, not a
    // best-effort JSON body.
    if let Some(format) = &query.format {
        if !format.eq_ignore_ascii_case("json") {
            return (StatusCode::NOT_IMPLEMENTED, "only json is supported").into_response();
        }
    }

    let Some(target) = embed_target(&state.config.public_url, &query.url) else {
        return (StatusCode::NOT_FOUND, "not an embeddable ironpad URL").into_response();
    };

    let notebook = match target.class {
        "public" => {
            let site_root =
                std::path::Path::new(state.leptos_options.site_root.as_ref()).to_path_buf();
            ironpad_app::server_fns::get_public_notebook_core(&site_root, &target.id).await
        }
        // The reader resolve: published only, drafts never (PRD-0057).
        "mutable" => ironpad_app::server_fns::get_mutable_notebook_core(&db, &target.id)
            .await
            .and_then(|nb| nb.ok_or_else(|| anyhow::anyhow!("no such notebook"))),
        _ => {
            ironpad_app::server_fns::get_shared_notebook_core(&state.config.data_dir, &target.id)
                .await
        }
    };

    let Ok(notebook) = notebook else {
        return (StatusCode::NOT_FOUND, "no such notebook").into_response();
    };

    let body = response_for(
        &state.config.public_url,
        &target,
        notebook.title,
        query.maxheight,
    );

    (
        [
            (header::CONTENT_TYPE, "application/json; charset=utf-8"),
            (header::CACHE_CONTROL, "public, max-age=3600"),
        ],
        axum::Json(body),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORIGIN: &str = "https://ironpad.twitchax.com";

    fn target(url: &str) -> Option<EmbedTarget> {
        embed_target(ORIGIN, url)
    }

    #[test]
    fn maps_the_canonical_notebook_routes() {
        assert_eq!(
            target("https://ironpad.twitchax.com/public/cannon"),
            Some(EmbedTarget {
                class: "public",
                id: "cannon".into()
            })
        );
        assert_eq!(
            target("https://ironpad.twitchax.com/shared/a1b2c3d4e5f60718"),
            Some(EmbedTarget {
                class: "shared",
                id: "a1b2c3d4e5f60718".into()
            })
        );
        // Published notebooks are embeddable too (PRD-0057).
        assert_eq!(
            target("https://ironpad.twitchax.com/mutable/a1b2c3d4e5f60718"),
            Some(EmbedTarget {
                class: "mutable",
                id: "a1b2c3d4e5f60718".into()
            })
        );
        assert_eq!(
            target("https://ironpad.twitchax.com/mutable/../etc"),
            None,
            "traversal in a mutable id is rejected like the other classes"
        );
    }

    #[test]
    fn the_legacy_extension_form_still_resolves() {
        // It redirects in a browser, so it is what a person's address bar
        // shows when they arrive from an old link, and what they then paste.
        assert_eq!(
            target("https://ironpad.twitchax.com/public/cannon.ironpad"),
            Some(EmbedTarget {
                class: "public",
                id: "cannon".into()
            })
        );
    }

    #[test]
    fn a_query_or_fragment_is_not_part_of_the_name() {
        assert_eq!(
            target("https://ironpad.twitchax.com/public/cannon?utm_source=reddit"),
            Some(EmbedTarget {
                class: "public",
                id: "cannon".into()
            })
        );
        assert_eq!(
            target("https://ironpad.twitchax.com/public/cannon#cell-3"),
            Some(EmbedTarget {
                class: "public",
                id: "cannon".into()
            })
        );
    }

    #[test]
    fn refuses_urls_that_are_not_ours() {
        // A provider that embedded any URL would be an open redirect wearing
        // an iframe: the consumer trusts the HTML because it trusts us.
        for hostile in [
            "https://evil.example/public/cannon",
            "https://ironpad.twitchax.com.evil.example/public/cannon",
            "http://ironpad.twitchax.com/public/cannon",
            "//ironpad.twitchax.com/public/cannon",
            "/public/cannon",
        ] {
            assert_eq!(target(hostile), None, "should have refused {hostile}");
        }
    }

    #[test]
    fn refuses_classes_without_an_embed_route() {
        // Private notebooks live in one browser's IndexedDB; the home page
        // is not a notebook. (Mutable became embeddable in PRD-0057 and is
        // asserted in `maps_the_canonical_notebook_routes`.)
        assert_eq!(target("https://ironpad.twitchax.com/local/some-uuid"), None);
        assert_eq!(target("https://ironpad.twitchax.com/"), None);
    }

    #[test]
    fn refuses_traversal_and_empty_names() {
        for hostile in [
            "https://ironpad.twitchax.com/public/",
            "https://ironpad.twitchax.com/public/../../etc/passwd",
            "https://ironpad.twitchax.com/public/a/b",
            "https://ironpad.twitchax.com/public/a\\b",
        ] {
            assert_eq!(target(hostile), None, "should have refused {hostile}");
        }
    }

    #[test]
    fn tolerates_a_trailing_slash_on_the_configured_origin() {
        assert_eq!(
            embed_target(
                "https://ironpad.twitchax.com/",
                "https://ironpad.twitchax.com/public/cannon"
            ),
            Some(EmbedTarget {
                class: "public",
                id: "cannon".into()
            })
        );
    }

    #[test]
    fn the_response_embeds_the_chrome_less_route() {
        let t = target("https://ironpad.twitchax.com/public/cannon").unwrap();
        let res = response_for(ORIGIN, &t, "The compiler fires a cannon".into(), None);

        assert_eq!(res.version, "1.0");
        assert_eq!(res.kind, "rich");
        assert_eq!(res.height, DEFAULT_HEIGHT);
        assert!(res
            .html
            .contains("https://ironpad.twitchax.com/embed/public/cannon"));
        assert!(res.html.starts_with("<iframe "));
    }

    #[test]
    fn a_hostile_title_cannot_break_out_of_the_iframe_tag() {
        // A shared notebook's title is attacker-controlled, and this HTML is
        // pasted verbatim into someone else's page.
        let t = target("https://ironpad.twitchax.com/shared/a1b2c3d4e5f60718").unwrap();
        let res = response_for(ORIGIN, &t, r#""><script>alert(1)</script>"#.into(), None);

        assert!(!res.html.contains("<script>"), "{}", res.html);
        assert!(
            res.html.ends_with("></iframe>"),
            "the tag must still close cleanly: {}",
            res.html
        );
        // The title field itself is JSON, which serde escapes; only the HTML
        // needs entity encoding.
        assert!(res.title.contains("<script>"));
    }

    #[test]
    fn maxheight_is_honoured_but_clamped() {
        let t = target("https://ironpad.twitchax.com/public/cannon").unwrap();
        let height = |max: Option<u32>| response_for(ORIGIN, &t, "t".into(), max).height;

        assert_eq!(height(Some(900)), 900);
        assert_eq!(height(None), DEFAULT_HEIGHT);
        assert_eq!(height(Some(0)), MIN_HEIGHT);
        assert_eq!(height(Some(u32::MAX)), MAX_HEIGHT);
    }
}
