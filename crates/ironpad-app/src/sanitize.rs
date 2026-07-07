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
/// markdown, and `LiveView` output. Starts from ammonia's default HTML
/// allowlist (which strips `<script>`, `on*` handlers, `javascript:` URLs,
/// `<iframe>`/`<object>`) and adds the safe, script-free presentation that rich
/// cell output relies on: inline SVG drawing (the [`SVG_TAGS`]/[`SVG_ATTRS`]
/// vetted for `Svg` panels — so no `script`/`foreignObject`/`a`/`use`/`image`)
/// plus `class` and a CSS-filtered `style` (both carried by [`SVG_ATTRS`]).
///
/// `class` preserves pulldown-cmark's `<span class="math …">` markers so the
/// client-side `KaTeX` bridge can find and render them (`KaTeX` emits its own
/// markup *after* sanitization, so its inline styles never pass through here).
///
/// The `style` attribute is CSS-filtered via [`filter_style_properties`] down to
/// the presentation-only [`STYLE_PROPERTIES`] allowlist. Ammonia does **no** CSS
/// filtering unless that method is called — the whole declaration list would
/// otherwise survive verbatim, letting untrusted output paint a
/// `position:fixed; inset:0; z-index:9999` overlay across the viewer to hijack
/// clicks (clickjacking) or beacon home in an auto-running shared/public
/// notebook. The allowlist keeps paint/text/box/flow presentation and drops
/// every out-of-flow positioning/stacking primitive (`position`, `z-index`,
/// `inset`, `top`/`right`/`bottom`/`left`), `content`, and anything not listed
/// (`-moz-binding`, `pointer-events`, …). The worst a surviving declaration can
/// do is fetch an external URL via `background:url()` — the same exposure as the
/// already-allowed `<img src>`.
///
/// `id` is *not* allowed generically (a DOM-clobbering surface); it is scoped to
/// the [`ID_SCOPED_SVG_TAGS`] containers that `url(#id)` references need.
///
/// [`filter_style_properties`]: ammonia::Builder::filter_style_properties
static HTML_BUILDER: LazyLock<Builder<'static>> = LazyLock::new(|| {
    let mut builder = Builder::default();
    builder
        .add_tags(SVG_TAGS.iter().copied())
        .add_generic_attributes(SVG_ATTRS.iter().copied())
        .filter_style_properties(STYLE_PROPERTIES.iter().copied().collect());
    // Scope `id` to the SVG containers addressed by `url(#id)` (gradients, clip
    // paths, markers, masks, patterns) rather than allowing it on every element.
    for tag in ID_SCOPED_SVG_TAGS {
        builder.add_tag_attributes(*tag, ["id"]);
    }
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

/// SVG container tags addressed by fragment references (`fill="url(#id)"`,
/// `clip-path="url(#id)"`, `marker-end="url(#id)"`, `mask="url(#id)"`). They are
/// the only elements that need an `id`, so `id` is scoped to just these rather
/// than allowed generically — a generic `id` is a DOM-clobbering surface.
const ID_SCOPED_SVG_TAGS: &[&str] = &[
    "linearGradient",
    "radialGradient",
    "clipPath",
    "marker",
    "mask",
    "pattern",
];

/// Presentation/geometry attributes allowed on sanitized SVG. None can execute
/// script (no `on*`, no URL schemes). `href`/`xlink:href` are excluded so no
/// element can point at a `javascript:` or external resource. `style` is kept
/// but CSS-filtered to [`STYLE_PROPERTIES`]; `id` is not here (scoped per-tag to
/// [`ID_SCOPED_SVG_TAGS`]) and `xmlns` is unnecessary (inline SVG injected via
/// `inner_html` is placed in the SVG namespace by the parser without it).
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
];

/// CSS property names kept inside a sanitized `style=` attribute; ammonia drops
/// every declaration whose property is not listed here (see [`HTML_BUILDER`]).
///
/// A *presentation-only* allowlist — paint, text, border, spacing, sizing, and
/// in-flow layout that legitimate rich output (`LiveView` dashboards, inline
/// styles) relies on. It deliberately omits every property that could lift an
/// element out of flow to overlay the viewer and hijack clicks (`position`,
/// `z-index`, `inset`/`top`/`right`/`bottom`/`left`), plus `content`; and, being
/// an allowlist, anything unlisted (`-moz-binding`, `pointer-events`, `filter`,
/// `float`, …) is dropped too. `background`/`background-image` are kept even
/// though a `url()` value can beacon — the same exposure as the already-allowed
/// `<img src>`, and required by legitimate gradient/image backgrounds.
const STYLE_PROPERTIES: &[&str] = &[
    // ── Color & paint ────────────────────────────────────────────────────
    "color",
    "opacity",
    "background",
    "background-color",
    "background-image",
    "background-position",
    "background-size",
    "background-repeat",
    "background-clip",
    "background-origin",
    "background-attachment",
    "fill",
    "fill-opacity",
    "fill-rule",
    "stroke",
    "stroke-width",
    "stroke-opacity",
    "stroke-dasharray",
    "stroke-dashoffset",
    "stroke-linecap",
    "stroke-linejoin",
    "stroke-miterlimit",
    "box-shadow",
    // ── Text & font ──────────────────────────────────────────────────────
    "font",
    "font-family",
    "font-size",
    "font-weight",
    "font-style",
    "font-variant",
    "font-stretch",
    "line-height",
    "letter-spacing",
    "word-spacing",
    "text-align",
    "text-decoration",
    "text-transform",
    "text-indent",
    "text-shadow",
    "text-overflow",
    "text-anchor",
    "white-space",
    "word-break",
    "overflow-wrap",
    "vertical-align",
    "direction",
    "writing-mode",
    "unicode-bidi",
    "dominant-baseline",
    "alignment-baseline",
    // ── Border & outline ─────────────────────────────────────────────────
    "border",
    "border-width",
    "border-style",
    "border-color",
    "border-top",
    "border-right",
    "border-bottom",
    "border-left",
    "border-radius",
    "border-collapse",
    "border-spacing",
    "outline",
    "outline-color",
    "outline-style",
    "outline-width",
    "outline-offset",
    // ── Box model & spacing ──────────────────────────────────────────────
    "margin",
    "margin-top",
    "margin-right",
    "margin-bottom",
    "margin-left",
    "padding",
    "padding-top",
    "padding-right",
    "padding-bottom",
    "padding-left",
    "width",
    "height",
    "min-width",
    "max-width",
    "min-height",
    "max-height",
    "box-sizing",
    "aspect-ratio",
    // ── In-flow layout (no out-of-flow positioning) ──────────────────────
    "display",
    "visibility",
    "overflow",
    "overflow-x",
    "overflow-y",
    "flex",
    "flex-direction",
    "flex-wrap",
    "flex-flow",
    "flex-grow",
    "flex-shrink",
    "flex-basis",
    "justify-content",
    "justify-items",
    "justify-self",
    "align-items",
    "align-content",
    "align-self",
    "place-items",
    "place-content",
    "place-self",
    "order",
    "gap",
    "row-gap",
    "column-gap",
    "grid",
    "grid-template",
    "grid-template-columns",
    "grid-template-rows",
    "grid-template-areas",
    "grid-area",
    "grid-column",
    "grid-row",
    "grid-auto-flow",
    "grid-auto-columns",
    "grid-auto-rows",
    // ── Table / list ─────────────────────────────────────────────────────
    "table-layout",
    "caption-side",
    "empty-cells",
    "list-style",
    "list-style-type",
    "list-style-position",
    // ── Object & transform (cannot overlay without the excluded props) ───
    "object-fit",
    "object-position",
    "transform",
    "transform-origin",
    "clip-path",
    "clip-rule",
    "cursor",
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

    // ── Bypass regressions (PRD-0038 T-006) ─────────────────────────────
    // These lock in the exclusions the XSS model rests on (sanitize.rs:41-71).
    // The dangerous SVG/HTML constructs below are stripped by the current
    // ammonia allowlist; the tests exist so a future allowlist change can't
    // silently re-open them.

    #[test]
    fn strips_foreign_object_and_its_active_content() {
        // <foreignObject> escapes the SVG namespace back into HTML — the classic
        // SVG-to-HTML injection vector. It is not in SVG_TAGS, so it must go, and
        // the event handler inside it must never survive.
        let dirty = r#"<svg><foreignObject><img src=x onerror="steal()"></foreignObject></svg>"#;
        let clean = sanitize_svg(dirty);
        assert!(
            !clean.contains("foreignObject"),
            "stripped foreignObject: {clean}"
        );
        assert!(!clean.contains("onerror"), "stripped handler: {clean}");
        assert!(!clean.contains("steal"), "stripped payload: {clean}");
    }

    #[test]
    fn strips_use_and_image_elements() {
        // <use xlink:href> and <image href> can pull in external/data: resources.
        // Both are excluded from SVG_TAGS.
        let dirty = concat!(
            r##"<svg><use xlink:href="data:image/svg+xml,<svg/>"/>"##,
            r#"<image href="data:text/html,evil"/></svg>"#,
        );
        let clean = sanitize_svg(dirty);
        assert!(!clean.contains("<use"), "stripped use: {clean}");
        assert!(!clean.contains("<image"), "stripped image: {clean}");
        assert!(!clean.contains("data:"), "stripped data uri: {clean}");
    }

    #[test]
    fn strips_animate_smil() {
        // SMIL <animate>/<set> can rewrite an attribute to a javascript: value at
        // runtime. Neither is in SVG_TAGS.
        let dirty = concat!(
            r#"<svg><rect x="0" y="0" width="5" height="5">"#,
            r#"<animate attributeName="href" to="javascript:steal()"/>"#,
            r#"<set attributeName="onload" to="steal()"/></rect></svg>"#,
        );
        let clean = sanitize_svg(dirty);
        assert!(!clean.contains("<animate"), "stripped animate: {clean}");
        assert!(!clean.contains("<set"), "stripped set: {clean}");
        assert!(!clean.contains("javascript:"), "stripped js url: {clean}");
        assert!(!clean.contains("steal"), "stripped payload: {clean}");
        // The safe rect still renders.
        assert!(clean.contains("<rect"), "kept rect: {clean}");
    }

    #[test]
    fn strips_data_uri_href() {
        // A data: URL in an <a href> can carry an HTML document that runs script
        // on navigation. `data` is not in ammonia's allowed URL schemes.
        let clean = sanitize_html(r#"<a href="data:text/html,<script>steal()</script>">x</a>"#);
        assert!(
            !clean.contains("data:text/html"),
            "stripped data uri href: {clean}"
        );
        assert!(!clean.contains("<script"), "no script leaked: {clean}");
        assert!(!clean.contains("steal"), "stripped payload: {clean}");
    }

    // ── CSS-filter policy (PRD-0038 T-005 / T-006) ───────────────────────
    // The `style` allowlist keeps presentation but must strip every out-of-flow
    // positioning/stacking primitive so untrusted output can't paint a
    // click-hijacking overlay in an auto-running shared/public notebook.

    #[test]
    fn style_neutralizes_positioning_overlay() {
        // A full-viewport `position:fixed` overlay is the clickjacking vector —
        // its positioning + stacking declarations must all be dropped, while the
        // legitimate paint/text presentation on the same element survives.
        let dirty = concat!(
            r#"<a href="https://evil.example/steal" "#,
            r#"style="position:fixed;inset:0;top:0;left:0;right:0;bottom:0;z-index:99999;"#,
            r#"background:#111;color:red;font-weight:bold;opacity:0.01">x</a>"#,
        );
        let clean = sanitize_html(dirty);
        // Positioning / stacking primitives are gone.
        assert!(!clean.contains("position"), "stripped position: {clean}");
        assert!(!clean.contains("z-index"), "stripped z-index: {clean}");
        assert!(!clean.contains("inset"), "stripped inset: {clean}");
        assert!(!clean.contains("top:"), "stripped top: {clean}");
        assert!(!clean.contains("left:"), "stripped left: {clean}");
        assert!(!clean.contains("right:"), "stripped right: {clean}");
        assert!(!clean.contains("bottom:"), "stripped bottom: {clean}");
        // Legitimate presentation on the same declaration list survives.
        assert!(clean.contains("color"), "kept color: {clean}");
        assert!(clean.contains("red"), "kept color value: {clean}");
        assert!(clean.contains("font-weight"), "kept font-weight: {clean}");
        assert!(clean.contains("background"), "kept background: {clean}");
        assert!(clean.contains("opacity"), "kept opacity: {clean}");
    }

    #[test]
    fn style_drops_content_and_binding() {
        // `content` (pseudo-element injection) and any `*-binding` (historical
        // script vector) are not presentation and must be dropped.
        let clean =
            sanitize_html(r#"<div style="content:'x';-moz-binding:url(#e);color:green">y</div>"#);
        assert!(!clean.contains("content"), "stripped content: {clean}");
        assert!(!clean.contains("binding"), "stripped binding: {clean}");
        assert!(clean.contains("color"), "kept color: {clean}");
    }

    #[test]
    fn style_keeps_presentation_declarations() {
        // The filter must not eat the presentation properties rich output relies
        // on: colors, typography, spacing, borders, and flow layout.
        let dirty = concat!(
            r#"<div style="background:#111;color:#eee;font-weight:bold;"#,
            r#"padding:8px;margin:4px;border:1px solid #333;border-radius:6px;"#,
            r#"display:flex;gap:8px;width:200px">ok</div>"#,
        );
        let clean = sanitize_html(dirty);
        for needle in [
            "background",
            "color",
            "font-weight",
            "padding",
            "margin",
            "border",
            "border-radius",
            "display",
            "gap",
            "width",
        ] {
            assert!(clean.contains(needle), "kept {needle}: {clean}");
        }
    }

    #[test]
    fn strips_generic_id_but_keeps_svg_gradient_id() {
        // `id` is a DOM-clobbering surface, so it is stripped from arbitrary
        // elements — but the SVG gradient/clip/marker/mask containers still need
        // it so `fill="url(#id)"` references resolve.
        let dirty = concat!(
            r#"<div id="clobber">x</div>"#,
            r#"<svg><defs><linearGradient id="g"><stop offset="0" stop-color="red"/>"#,
            r#"</linearGradient></defs>"#,
            r#"<rect id="r" x="0" y="0" width="4" height="4" fill="url(#g)"/></svg>"#,
        );
        let clean = sanitize_html(dirty);
        assert!(
            !clean.contains(r#"id="clobber""#),
            "stripped generic div id: {clean}"
        );
        assert!(
            !clean.contains(r#"id="r""#),
            "stripped generic rect id: {clean}"
        );
        assert!(clean.contains(r#"id="g""#), "kept gradient id: {clean}");
        assert!(clean.contains("url(#g)"), "kept gradient ref: {clean}");
    }

    #[test]
    fn strips_unnecessary_xmlns() {
        // Inline SVG injected via inner_html is placed in the SVG namespace by
        // the HTML parser without an explicit xmlns, so the attribute is dropped.
        let clean = sanitize_svg(
            r#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="4" height="4"/></svg>"#,
        );
        assert!(!clean.contains("xmlns"), "stripped xmlns: {clean}");
        assert!(clean.contains("<svg"), "kept svg: {clean}");
        assert!(clean.contains("<rect"), "kept rect: {clean}");
    }
}
