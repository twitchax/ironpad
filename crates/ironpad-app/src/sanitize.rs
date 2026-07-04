//! HTML/SVG sanitization for untrusted cell output.
//!
//! Cell code is arbitrary and its rendered `Html`/`Svg` output is injected into
//! the page via `inner_html`. In shared/public notebooks that output is viewed
//! by *other* people, and view mode auto-runs every cell — so a cell emitting
//! `Html("<img src=x onerror=steal()>")` would run script in the viewer's
//! origin (stored XSS). These helpers strip script, event handlers, and other
//! active content with an allowlist while preserving the visual markup.

use std::sync::LazyLock;

use ammonia::Builder;

/// Builder for all sanitized cell output — `Html` and `Svg` panels, rendered
/// markdown, and `LiveView` output. Starts
/// from ammonia's default HTML allowlist (which strips `<script>`, `on*`
/// handlers, `javascript:` URLs, `<iframe>`/`<object>`) and adds the safe,
/// script-free presentation that rich cell output relies on: inline SVG drawing
/// (the same [`SVG_TAGS`]/[`SVG_ATTRS`] vetted for `Svg` panels — so no
/// `script`/`foreignObject`/`a`/`use`/`image`) plus `style` and `class`, both
/// carried by [`SVG_ATTRS`].
///
/// `class` also preserves pulldown-cmark's `<span class="math …">` markers so
/// the client-side `KaTeX` bridge can find and render them. None of this adds a
/// script vector; the worst `style` can do is reference an external URL — the
/// same exposure as the already-allowed `<img src>`.
static HTML_BUILDER: LazyLock<Builder<'static>> = LazyLock::new(|| {
    let mut builder = Builder::default();
    builder
        .add_tags(SVG_TAGS.iter().copied())
        .add_generic_attributes(SVG_ATTRS.iter().copied());
    builder
});

/// Sanitize markup with the shared cell-output allowlist. See [`HTML_BUILDER`].
fn clean_html(html: &str) -> String {
    HTML_BUILDER.clean(html).to_string()
}

/// SVG element names allowed in sanitized SVG output. Deliberately excludes
/// `script`, `foreignObject`, `a`, `use`, and `image` (script execution or
/// URL-bearing elements). Gradient/marker references still work — they are
/// carried by presentation attribute *values* like `fill="url(#id)"`.
const SVG_TAGS: &[&str] = &[
    "svg",
    "g",
    "path",
    "rect",
    "circle",
    "ellipse",
    "line",
    "polyline",
    "polygon",
    "text",
    "tspan",
    "defs",
    "clipPath",
    "linearGradient",
    "radialGradient",
    "stop",
    "marker",
    "title",
    "desc",
    "symbol",
    "pattern",
    "mask",
];

/// Presentation/geometry attributes allowed on sanitized SVG. None can execute
/// script (no `on*`, no URL schemes). `href`/`xlink:href` are excluded so no
/// element can point at a `javascript:` or external resource.
const SVG_ATTRS: &[&str] = &[
    "d",
    "fill",
    "stroke",
    "stroke-width",
    "stroke-dasharray",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "x",
    "y",
    "x1",
    "y1",
    "x2",
    "y2",
    "cx",
    "cy",
    "r",
    "rx",
    "ry",
    "dx",
    "dy",
    "width",
    "height",
    "viewBox",
    "transform",
    "gradientTransform",
    "points",
    "offset",
    "stop-color",
    "stop-opacity",
    "opacity",
    "fill-opacity",
    "fill-rule",
    "stroke-opacity",
    "font-size",
    "font-family",
    "font-weight",
    "font-style",
    "text-anchor",
    "dominant-baseline",
    "alignment-baseline",
    "class",
    "id",
    "style",
    "gradientUnits",
    "spreadMethod",
    "clip-path",
    "clip-rule",
    "preserveAspectRatio",
    "marker-end",
    "marker-start",
    "marker-mid",
    "markerWidth",
    "markerHeight",
    "refX",
    "refY",
    "orient",
    "patternUnits",
    "maskUnits",
    "xmlns",
];

/// Sanitize a cell's `Html` output before it is injected via `inner_html`.
pub fn sanitize_html(html: &str) -> String {
    clean_html(html)
}

/// Sanitize a cell's `Svg` output before it is injected via `inner_html`.
///
/// Uses the same allowlist as [`sanitize_html`] (which already permits inline
/// SVG), so both entry points share one builder.
pub fn sanitize_svg(svg: &str) -> String {
    clean_html(svg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_strips_script_and_handlers() {
        let dirty =
            r#"<p onclick="steal()">hi</p><script>steal()</script><img src=x onerror="steal()">"#;
        let clean = sanitize_html(dirty);
        assert!(clean.contains("<p"), "kept the paragraph: {clean}");
        assert!(clean.contains("hi"), "kept the text: {clean}");
        assert!(!clean.contains("<script"), "stripped script: {clean}");
        assert!(!clean.contains("onclick"), "stripped onclick: {clean}");
        assert!(!clean.contains("onerror"), "stripped onerror: {clean}");
        assert!(!clean.contains("steal"), "stripped the payload: {clean}");
    }

    #[test]
    fn html_strips_javascript_url() {
        let clean = sanitize_html(r#"<a href="javascript:steal()">x</a>"#);
        assert!(!clean.contains("javascript:"), "stripped js url: {clean}");
    }

    #[test]
    fn html_keeps_inline_svg_and_styles() {
        // Rich cell output — LiveView dashboards, inline-SVG plots — relies on
        // `style=` and inline `<svg>`. Sanitization must keep that presentation
        // while still stripping active content. (Regression: routing LiveView
        // output through the HTML sanitizer stripped both.)
        let dirty = concat!(
            r#"<div style="background: linear-gradient(#000,#111);" class="dash">"#,
            r#"<svg viewBox="0 0 10 10"><path d="M0 0 L10 10" stroke="red"/>"#,
            r#"<circle cx="5" cy="5" r="2"/></svg>"#,
            r#"<script>steal()</script><span onclick="steal()">x</span></div>"#,
        );
        let clean = sanitize_html(dirty);
        // Presentation survives.
        assert!(
            clean.contains("linear-gradient"),
            "kept inline style: {clean}"
        );
        assert!(clean.contains("<svg"), "kept svg: {clean}");
        assert!(clean.contains("<path"), "kept svg path: {clean}");
        assert!(clean.contains("<circle"), "kept svg circle: {clean}");
        assert!(clean.contains("dash"), "kept class: {clean}");
        // Active content is still stripped.
        assert!(!clean.contains("<script"), "stripped script: {clean}");
        assert!(!clean.contains("onclick"), "stripped handler: {clean}");
        assert!(!clean.contains("steal"), "stripped payload: {clean}");
    }

    #[test]
    fn svg_keeps_drawing_elements() {
        let svg = r#"<svg viewBox="0 0 10 10"><path d="M0 0 L10 10" stroke="red" fill="none"/><circle cx="5" cy="5" r="2" fill="url(#g)"/></svg>"#;
        let clean = sanitize_svg(svg);
        assert!(clean.contains("<svg"), "kept svg: {clean}");
        assert!(clean.contains("<path"), "kept path: {clean}");
        assert!(
            clean.contains(r#"d="M0 0 L10 10""#),
            "kept path data: {clean}"
        );
        assert!(clean.contains("<circle"), "kept circle: {clean}");
        assert!(clean.contains("url(#g)"), "kept gradient ref: {clean}");
    }

    #[test]
    fn svg_strips_script_and_handlers() {
        let dirty = r#"<svg><script>steal()</script><rect onload="steal()" x="0" y="0" width="5" height="5"/><a href="javascript:steal()"><text>x</text></a></svg>"#;
        let clean = sanitize_svg(dirty);
        assert!(!clean.contains("<script"), "stripped svg script: {clean}");
        assert!(!clean.contains("onload"), "stripped onload: {clean}");
        assert!(!clean.contains("javascript:"), "stripped js url: {clean}");
        assert!(!clean.contains("steal"), "stripped payload: {clean}");
        // The safe rect geometry survives.
        assert!(clean.contains("<rect"), "kept rect: {clean}");
        assert!(clean.contains(r#"width="5""#), "kept rect width: {clean}");
    }
}
