//! Open Graph / Twitter Card metadata for link unfurls (PRD-0050).
//!
//! Reddit, X, Slack, Discord, and `LinkedIn` all decide what a pasted link looks
//! like by fetching the HTML and reading these tags. **None of them run
//! JavaScript**, which drives two requirements that are easy to get wrong:
//!
//! 1. The tags must be in the SSR'd `<head>` of the *first* response. A route
//!    whose title comes from a `Resource` therefore needs
//!    `SsrMode::Async`; under the default
//!    out-of-order streaming the head is flushed before the resource resolves,
//!    and `leptos_meta` patches the tags in afterwards with a script that a
//!    crawler never executes. It looks correct in a browser and is invisible
//!    to every unfurler.
//! 2. `og:image` and `og:url` must be absolute. Crawlers have no document base
//!    to resolve against, so the origin comes from `IRONPAD_PUBLIC_URL`
//!    server-side (see `AppConfig::absolute_url`).

use leptos::prelude::*;
use leptos_meta::{Link, Meta, Title};

/// The absolute origin to resolve root-relative paths against.
///
/// Server-side this is the configured `IRONPAD_PUBLIC_URL`, because a crawler
/// has no other way to learn it. Client-side it is the live origin, which
/// matches the configured value on any correctly-deployed instance and keeps
/// the hydrated tags identical to the rendered ones.
#[cfg(feature = "ssr")]
fn origin() -> String {
    // `use_context` rather than `expect_context`: `generate_route_list` walks
    // the component tree at startup with no context provided, and a panic
    // there would take down the server before it ever binds a port.
    use_context::<ironpad_common::AppConfig>()
        .map(|c| c.public_url.trim_end_matches('/').to_string())
        .unwrap_or_default()
}

#[cfg(not(feature = "ssr"))]
fn origin() -> String {
    web_sys::window()
        .and_then(|w| w.location().origin().ok())
        .unwrap_or_default()
}

/// Card size the server renders, declared so unfurlers can lay out the
/// preview before the image finishes downloading. Must track
/// `ironpad_server::og::svg::{WIDTH, HEIGHT}`.
const IMAGE_WIDTH: &str = "1200";
const IMAGE_HEIGHT: &str = "630";

/// Longest `og:description` worth emitting. Most unfurlers truncate somewhere
/// between 200 and 300 characters; clipping here means *we* choose where the
/// sentence breaks rather than letting each platform cut mid-word.
const MAX_DESCRIPTION: usize = 200;

/// Emits the full unfurl block for one page.
///
/// `path` and `image` are root-relative (`/public/cannon`,
/// `/og/public/cannon.png`); this resolves them against the configured origin.
#[component]
pub fn SocialMeta(
    /// Page subject, used bare in `og:title` and suffixed in `<title>`.
    #[prop(into)]
    title: String,
    /// Passed through as-is; `None` falls back to a generic line.
    #[prop(optional_no_strip)]
    description: Option<String>,
    /// Canonical root-relative path for this page.
    #[prop(into)]
    path: String,
    /// Root-relative path to the preview image.
    #[prop(into)]
    image: String,
    /// `og:type`: `"website"` for the home page, `"article"` for a notebook.
    #[prop(into, default = "article".to_string())]
    kind: String,
    /// Ask search engines not to index this page.
    ///
    /// Used for `/shared` and `/mutable`, which are unlisted by construction:
    /// someone sending a colleague a link did not ask to be in Google. This is
    /// deliberately *not* done with `robots.txt`, because several unfurlers
    /// honour that file and would then decline to build a preview at all,
    /// whereas `noindex` is read by search engines and ignored by unfurlers.
    #[prop(optional)]
    noindex: bool,
) -> impl IntoView {
    let origin = origin();
    // Prefixing in place consumes the props rather than borrowing them, which
    // is both what clippy wants and one allocation fewer than `format!`.
    let mut url = path;
    url.insert_str(0, &origin);
    let mut image_url = image;
    image_url.insert_str(0, &origin);
    let page_title = format!("{title} \u{b7} ironpad");
    let description = description.map_or_else(
        || "An interactive Rust notebook on ironpad.".to_string(),
        |d| clamp(&d, MAX_DESCRIPTION),
    );
    let alt = format!("Preview card for the ironpad notebook \"{title}\"");

    view! {
        <Title text=page_title/>
        <Link rel="canonical" href=url.clone()/>
        <Meta name="description" content=description.clone()/>

        <Meta property="og:type" content=kind/>
        <Meta property="og:site_name" content="ironpad"/>
        <Meta property="og:title" content=title.clone()/>
        <Meta property="og:description" content=description.clone()/>
        <Meta property="og:url" content=url/>
        <Meta property="og:image" content=image_url.clone()/>
        <Meta property="og:image:type" content="image/png"/>
        <Meta property="og:image:width" content=IMAGE_WIDTH/>
        <Meta property="og:image:height" content=IMAGE_HEIGHT/>
        <Meta property="og:image:alt" content=alt/>

        // Without an explicit card type, X renders a small square thumbnail
        // instead of the wide card the 1200x630 image is drawn for.
        <Meta name="twitter:card" content="summary_large_image"/>
        <Meta name="twitter:title" content=title/>
        <Meta name="twitter:description" content=description/>
        <Meta name="twitter:image" content=image_url/>

        {noindex.then(|| view! { <Meta name="robots" content="noindex, follow"/> })}
    }
}

/// Truncates on a word boundary and marks the cut, so a clipped description
/// does not end mid-word.
fn clamp(text: &str, max: usize) -> String {
    let text = text.trim();
    if text.chars().count() <= max {
        return text.to_string();
    }

    let truncated: String = text.chars().take(max).collect();
    let cut = truncated
        .rfind(char::is_whitespace)
        .map_or(truncated.as_str(), |i| &truncated[..i]);

    format!(
        "{}\u{2026}",
        cut.trim_end_matches(['.', ',', ';', ':', ' '])
    )
}

#[cfg(test)]
mod tests {
    use super::clamp;

    #[test]
    fn short_text_is_untouched() {
        assert_eq!(clamp("A short description.", 200), "A short description.");
        assert_eq!(clamp("  padded  ", 200), "padded");
    }

    #[test]
    fn long_text_is_cut_on_a_word_boundary() {
        let text = "word ".repeat(100);
        let out = clamp(&text, 50);
        assert!(out.ends_with('\u{2026}'));
        assert!(out.chars().count() <= 51);
        // Cut between words, never through one.
        assert!(!out.contains("wor\u{2026}"));
    }

    #[test]
    fn dangling_punctuation_is_dropped_before_the_ellipsis() {
        let text = format!("{} tail", "a".repeat(40));
        let out = clamp(&text, 42);
        assert!(!out.contains(",\u{2026}"));
        assert!(out.ends_with('\u{2026}'));
    }

    #[test]
    fn an_unbroken_run_longer_than_the_budget_still_terminates() {
        // No whitespace to break on: must not panic and must stay bounded.
        let out = clamp(&"x".repeat(500), 20);
        assert!(out.chars().count() <= 21);
    }
}
