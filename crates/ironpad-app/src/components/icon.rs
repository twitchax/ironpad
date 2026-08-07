//! The one icon rendering seam (PRD-0062).
//!
//! ironpad used to draw its affordances with Unicode characters, which meant
//! the browser picked the shape from whatever font on the user's machine
//! covered the codepoint: five glyphs rendered as full-colour emoji, nine more
//! coloured themselves on some Windows/Android configurations (the run button
//! among them), and the remaining thirty-three came from `DejaVu` Sans,
//! Segoe UI Symbol, or Apple Symbols depending on platform. Consistency is only
//! achievable by shipping the geometry, so we ship it.
//!
//! Two consumers, ONE definition of the wrapper markup:
//!
//! - [`IconLabel`] / [`Icon`] for `view!` call sites, and
//! - [`icon_svg_markup`] for the Export-HTML path, which builds a `String`.
//!
//! That is deliberate. This codebase has a documented history of one policy
//! growing two hand-maintained copies (the editor/viewer run paths, the two
//! capture-manifest recipes), and a forked SVG wrapper would mean exported
//! files silently drifting from what the app renders.

use leptos::prelude::*;

/// Re-exported so call sites and [`crate::components::icons`] share one name
/// for the icon type without every file depending on `icondata_core`.
pub type IconData = &'static icondata_core::IconData;

/// Class on every rendered icon. Styling (1em box, alignment, no shrink)
/// lives in `style/main.scss` next to the rest of the design tokens.
const ICON_CLASS: &str = "ironpad-icon";

/// Build the `<svg>` markup for `icon`.
///
/// THE wrapper definition: attributes come from the icon's own `IconData`
/// (viewBox, stroke width, caps/joins), the box is `1em` square so the icon
/// inherits the surrounding font-size, and paint is `currentColor` so it
/// inherits the theme. `aria-hidden` is unconditional — an icon beside a
/// label is decorative, and an icon-only control is named by its button's
/// `title`/`aria-label`, never by the svg.
///
/// The path data is interpolated verbatim. It is a compile-time constant
/// from the icon crate and is never user input; nothing here may be handed
/// caller-supplied strings.
#[must_use]
pub fn icon_svg_markup(icon: IconData) -> String {
    let view_box = icon.view_box.unwrap_or("0 0 24 24");
    let stroke = icon.stroke.unwrap_or("currentColor");
    let fill = icon.fill.unwrap_or("none");
    let width = icon.stroke_width.unwrap_or("2");
    let cap = icon.stroke_linecap.unwrap_or("round");
    let join = icon.stroke_linejoin.unwrap_or("round");
    format!(
        "<svg class=\"{ICON_CLASS}\" xmlns=\"http://www.w3.org/2000/svg\" \
         viewBox=\"{view_box}\" fill=\"{fill}\" stroke=\"{stroke}\" \
         stroke-width=\"{width}\" stroke-linecap=\"{cap}\" stroke-linejoin=\"{join}\" \
         aria-hidden=\"true\" focusable=\"false\">{}</svg>",
        icon.data
    )
}

/// An icon on its own, for controls that carry their name elsewhere (a
/// button `title`, an `aria-label`). Pairs with the existing Playwright
/// selectors, which target those titles rather than glyph text.
#[component]
pub fn Icon(icon: IconData) -> impl IntoView {
    // `inner_html` rather than a parsed child tree: the data is a static
    // fragment of one or more <path> elements, and re-parsing it per render
    // would buy nothing. See the module docs on why this is not an XSS seam.
    view! { <span class="ironpad-icon-slot" inner_html=icon_svg_markup(icon) /> }
}

/// An icon followed by its label — the common affordance shape (menu items,
/// status badges, buttons with visible text).
///
/// `label` is a plain `String`, not a signal, on purpose: every reactive
/// call site already sits inside a `move ||` closure in a `view!`, so the
/// closure rebuilds this component and a signal prop would add ceremony
/// without adding reactivity.
/// A disclosure chevron: right when collapsed, down when expanded.
///
/// Its own component because the `▸`/`▾` pair appeared at eight sites as a
/// hand-rolled conditional, and eight copies of one policy is how the
/// disclosure affordance drifts between the editor, the viewer, and the
/// panels.
#[component]
pub fn Chevron(#[prop(into)] expanded: Signal<bool>) -> impl IntoView {
    view! {
        <span class="ironpad-icon-slot" inner_html=move || {
            icon_svg_markup(if expanded.get() {
                super::icons::CHEVRON_DOWN
            } else {
                super::icons::CHEVRON_RIGHT
            })
        } />
    }
}

#[component]
pub fn IconLabel(icon: IconData, #[prop(into)] label: String) -> impl IntoView {
    view! {
        <span class="ironpad-icon-label">
            <span class="ironpad-icon-slot" inner_html=icon_svg_markup(icon) />
            <span>{label}</span>
        </span>
    }
}

#[cfg(test)]
mod tests {
    use super::{icon_svg_markup, ICON_CLASS};
    use crate::components::icons;

    #[test]
    fn markup_carries_the_icon_geometry_and_theming_hooks() {
        let svg = icon_svg_markup(icons::HISTORY);
        assert!(svg.contains("viewBox=\"0 0 24 24\""), "{svg}");
        assert!(svg.contains(&format!("class=\"{ICON_CLASS}\"")), "{svg}");
        // currentColor + no fill is what makes the dark theme work for free.
        assert!(svg.contains("stroke=\"currentColor\""), "{svg}");
        assert!(svg.contains("fill=\"none\""), "{svg}");
        // Decorative: the accessible name comes from the label or the button.
        assert!(svg.contains("aria-hidden=\"true\""), "{svg}");
        assert!(svg.contains("focusable=\"false\""), "{svg}");
        // The actual path data made it in.
        assert!(svg.contains("<path"), "{svg}");
    }

    #[test]
    fn distinct_roles_render_distinct_geometry() {
        // Guards the mapping table against a copy-paste that points two roles
        // at one icon — the failure mode a human eye skips right over.
        let run = icon_svg_markup(icons::RUN);
        let stop = icon_svg_markup(icons::STOP);
        let history = icon_svg_markup(icons::HISTORY);
        assert_ne!(run, stop);
        assert_ne!(run, history);
        assert_ne!(stop, history);
    }

    #[test]
    fn every_mapped_role_has_renderable_data() {
        // Shape-agnostic on purpose: Lucide draws with path, circle, line,
        // polygon, polyline, and rect depending on the icon (LuPlay is a
        // polygon), so assert there IS geometry rather than which kind.
        for (name, icon) in icons::ALL {
            assert!(
                !icon.data.trim().is_empty(),
                "{name} has empty icon data — the mapping points at nothing"
            );
            let svg = icon_svg_markup(icon);
            assert!(
                svg.contains(&format!(">{}", icon.data)),
                "{name} lost its geometry in the wrapper: {svg}"
            );
        }
    }
}
