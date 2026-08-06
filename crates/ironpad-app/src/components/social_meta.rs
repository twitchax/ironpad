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

/// Resolves a root-relative `path` against this instance's public origin.
///
/// Server-side the origin is the configured `IRONPAD_PUBLIC_URL`, because a
/// crawler has no other way to learn it. Client-side it is the live origin,
/// which matches the configured value on any correctly-deployed instance and
/// keeps the hydrated tags identical to the rendered ones.
///
/// Falls back to `path` unchanged when no origin is available, which is a
/// root-relative URL: wrong for a crawler but harmless in a browser, and
/// strictly better than emitting a bare path glued to an empty string.
/// Takes `path` by value so the no-origin fallback returns it without a copy.
#[cfg(feature = "ssr")]
fn absolute(path: String) -> String {
    // `use_context` rather than `expect_context`: `generate_route_list` walks
    // the component tree at startup with no context provided, and a panic
    // there would take down the server before it ever binds a port.
    match use_context::<ironpad_common::AppConfig>() {
        Some(config) => config.absolute_url(&path),
        None => path,
    }
}

#[cfg(not(feature = "ssr"))]
fn absolute(path: String) -> String {
    match web_sys::window().and_then(|w| w.location().origin().ok()) {
        Some(origin) => ironpad_common::absolute_url(&origin, &path),
        None => path,
    }
}

/// Card size the server renders, declared so unfurlers can lay out the
/// preview before the image finishes downloading. Single-sourced with the
/// size the server rasterizes the card at (`ironpad_common::OG_CARD_*`, which
/// `ironpad_server::og::svg` also reads) so the announced and rendered
/// dimensions can never drift.
///
/// Only correct for the *generated* card. A notebook overriding `og_image`
/// with a screenshot of another shape passes its own size through the
/// `image_size` prop; before that existed, a tall image was still announced as
/// 1200x630 and came out letterboxed or cropped in the feed.
const IMAGE_WIDTH: u32 = ironpad_common::OG_CARD_WIDTH;
const IMAGE_HEIGHT: u32 = ironpad_common::OG_CARD_HEIGHT;

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
    /// Pixel size of `image`, when it is an override of a non-standard shape.
    /// `None` means the generated card, which is always [`IMAGE_WIDTH`] x
    /// [`IMAGE_HEIGHT`].
    #[prop(optional_no_strip)]
    image_size: Option<(u32, u32)>,
    /// `og:type`: `"website"` for the home page, `"article"` for a notebook.
    #[prop(into, default = "article".to_string())]
    kind: String,
    /// Advertise an oEmbed endpoint for this page (PRD-0051).
    ///
    /// Set on the routes that have a matching `/embed/*` renderer, which is
    /// `/public` and `/shared`. A consumer that follows this link embeds the
    /// running notebook rather than a picture of it.
    #[prop(optional)]
    oembed: bool,
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
    let url = absolute(path);
    let image_url = absolute(image);
    let (image_width, image_height) = image_size.unwrap_or((IMAGE_WIDTH, IMAGE_HEIGHT));
    let title = sanitize_title(title);
    // The consumer re-fetches this URL, so the page URL travels as a query
    // parameter and must be percent-encoded.
    let oembed_href =
        oembed.then(|| absolute(format!("/oembed?url={}&format=json", encode_query(&url))));
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
        <Meta property="og:image:width" content=image_width.to_string()/>
        <Meta property="og:image:height" content=image_height.to_string()/>
        <Meta property="og:image:alt" content=alt/>

        // Without an explicit card type, X renders a small square thumbnail
        // instead of the wide card the 1200x630 image is drawn for.
        <Meta name="twitter:card" content="summary_large_image"/>
        <Meta name="twitter:title" content=title/>
        <Meta name="twitter:description" content=description/>
        <Meta name="twitter:image" content=image_url/>

        {oembed_href
            .map(|href| {
                view! {
                    <Link
                        rel="alternate"
                        type_="application/json+oembed"
                        href=href
                        title="ironpad notebook"
                    />
                }
            })}

        {noindex.then(|| view! { <Meta name="robots" content="noindex, follow"/> })}
    }
}

/// Percent-encodes a URL for use as a query-parameter *value*.
///
/// Deliberately conservative: everything outside the unreserved set goes,
/// including `/` and `:`, so the encoded page URL cannot be mistaken for
/// structure in the endpoint's own query string.
fn encode_query(value: &str) -> String {
    use std::fmt::Write as _;

    let mut out = String::with_capacity(value.len() + 16);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// Marks the current response as a 404.
///
/// The notebook routes render with `SsrMode::Async`, which buffers the whole
/// document before the first byte, so a status set from inside the resolved
/// `Suspend` is still applied. That was **not** true under the previous
/// streaming mode, where the shell was long gone by then: switching to Async
/// for the meta tags is what made an honest status code reachable at all.
///
/// Without it a missing notebook answers 200 with an error body, which
/// unfurlers cache as a valid preview and search engines index as a soft-404.
pub fn mark_not_found() {
    #[cfg(feature = "ssr")]
    if let Some(response) = use_context::<leptos_axum::ResponseOptions>() {
        response.set_status(http::StatusCode::NOT_FOUND);
    }
}

/// Strips characters that must never reach a notebook title.
///
/// **This is a security control, not cosmetics.** `leptos_meta` splices the
/// title into the SSR head as raw text (`buf.push_str(&title)` in its
/// `inject_meta_context`), and `<title>` is RCDATA, so an injected `</title>`
/// closes the element and anything after it becomes live markup in the head.
/// Notebook titles are attacker-controlled on `/shared` and `/mutable` (both
/// accept unauthenticated uploads), which made that a stored XSS on ironpad's
/// own origin, where `window.IronpadStorage` exposes the mutable-share user
/// key. Every other tag here is escaped by the framework's attribute writer;
/// the title is the one text node and therefore the one hole.
///
/// Removing rather than entity-escaping is deliberate: on the client
/// `leptos_meta` assigns through `document().set_title()`, a DOM property that
/// does not decode entities, so `&lt;` would render literally in the browser
/// tab after hydration.
///
/// C0 controls go too. XML 1.0 forbids them, and one in a title made
/// `usvg::Tree::from_str` reject the preview card, turning `/og/...png` into a
/// permanent 500 for that notebook.
fn sanitize_title(mut title: String) -> String {
    title.retain(|c| !matches!(c, '<' | '>') && (!c.is_control() || c == '\t'));
    title.trim().to_string()
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
    use super::{clamp, sanitize_title};

    #[test]
    fn a_title_cannot_break_out_of_the_title_element() {
        // The shipped vulnerability: leptos_meta splices the title into the
        // SSR head as raw text, and `<title>` is RCDATA, so this closed the
        // element and ran script on ironpad's own origin. Notebook titles
        // arrive through unauthenticated share uploads.
        let out = sanitize_title("</title><script>window.PWNED=1</script>".to_string());
        assert!(!out.contains('<'), "{out:?}");
        assert!(!out.contains('>'), "{out:?}");
        assert_eq!(out, "/titlescriptwindow.PWNED=1/script");
    }

    #[test]
    fn control_characters_are_stripped_but_tabs_survive() {
        assert_eq!(sanitize_title("bad\u{1}title".to_string()), "badtitle");
        assert_eq!(sanitize_title("null\u{0}byte".to_string()), "nullbyte");
        assert_eq!(sanitize_title("a\tb".to_string()), "a\tb");
        // Leading and trailing whitespace goes; a title is a display string.
        assert_eq!(sanitize_title("  spaced  ".to_string()), "spaced");
    }

    #[test]
    fn ordinary_titles_pass_through_untouched() {
        // Including the non-ASCII that legitimate notebooks carry.
        for title in [
            "The compiler fires a cannon",
            "Pin, coroutines, and structs that point at themselves",
            "Conformal maps: z \u{21a6} z\u{b2}",
        ] {
            assert_eq!(sanitize_title(title.to_string()), title);
        }
    }

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
