// ── Live View panel component ────────────────────────────────────────────────
//
// Renders tick-driven live content (Text / HTML / Markdown) from a LiveView
// cell.  Mirrors the `SimulationCanvas` pattern in `animation_canvas.rs` but
// writes string content into a DOM element instead of pixel data onto a canvas.

use crate::components::icon::Icon;
use crate::components::icons;
use leptos::prelude::*;

// ── KaTeX JS interop (hydrate-only) ─────────────────────────────────────────

#[cfg(feature = "hydrate")]
mod js {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen]
    extern "C" {
        /// Ask the KaTeX bridge to render maths inside the given root element.
        #[wasm_bindgen(js_namespace = IronpadKaTeX, js_name = "renderMathIn", catch)]
        pub fn render_math_in(root: &web_sys::HtmlElement) -> Result<(), JsValue>;
    }
}

// ── Live content rendering ──────────────────────────────────────────────────

/// How a `LiveView` cell's content should reach the DOM.
pub(crate) enum LiveContent<'a> {
    /// Sanitized HTML, safe to inject via `inner_html`.
    Html(String),
    /// Plain text, set via `text_content` (no HTML interpretation).
    Text(&'a str),
}

/// Decide how a `LiveView` cell's content renders, by kind.
///
/// `html` and `markdown` are sanitized: `LiveView` output is untrusted and, in a
/// shared or public notebook, viewed by others (the same stored-XSS threat the
/// sanitizer guards against for `Html`/`Svg` panels). Markdown routes through
/// the shared `markdown_cell::render_markdown` so `KaTeX` math classes survive
/// and there is a single markdown code path. No DOM calls, so the sanitization
/// stays unit-testable.
pub(crate) fn render_live_content<'a>(kind: &str, text: &'a str) -> LiveContent<'a> {
    match kind {
        "html" => LiveContent::Html(crate::sanitize::sanitize_html(text)),
        "markdown" => LiveContent::Html(crate::components::markdown_cell::render_markdown(text)),
        // "text" and anything else: plain text.
        _ => LiveContent::Text(text),
    }
}

/// Decode a `LiveTickResult` kind code (`ironpad_cell::LiveTickResult`) into
/// the kind string [`render_live_content`] and the initial `LiveView` panel use.
#[cfg_attr(not(feature = "hydrate"), allow(dead_code))]
pub(crate) fn live_kind_str(code: u32) -> &'static str {
    match code {
        1 => "html",
        2 => "markdown",
        // 0 and anything else: plain text.
        _ => "text",
    }
}

/// Write a `LiveView` cell's content into its element, by kind, then let
/// `KaTeX` render any maths an HTML or markdown kind brought in.
#[cfg(feature = "hydrate")]
fn apply_live_content(el: &web_sys::HtmlElement, kind: &str, text: &str) {
    match render_live_content(kind, text) {
        LiveContent::Html(html) => {
            el.set_inner_html(&html);
            let _ = js::render_math_in(el);
        }
        LiveContent::Text(t) => {
            el.set_text_content(Some(t));
        }
    }
}

// ── Component ───────────────────────────────────────────────────────────────

/// Renders live, tick-driven content from a LiveView cell.
///
/// On each animation frame (throttled to `fps`), calls `tick_live_cell()` and
/// updates the DOM element with the returned content string.  Provides
/// play/pause, step, and frame counter controls matching `SimulationCanvas`.
#[allow(clippy::needless_pass_by_value)]
#[component]
pub fn LiveViewPanel(
    fps: u32,
    kind: String,
    content: String,
    #[prop(into)] cell_id: String,
) -> impl IntoView {
    #[cfg(feature = "hydrate")]
    {
        use std::cell::Cell;
        use std::rc::Rc;

        use crate::components::raf_loop::RafLoop;

        let content_ref = NodeRef::<leptos::html::Div>::new();
        let playing = RwSignal::new(true);
        let frame_number = RwSignal::new(0u32);

        // One tick: fetch the next content from the executor and render it.
        // Shared by the loop and the Step button, and refused while a tick is
        // still in flight (returning `false` so the loop retries on the next
        // frame).
        let tick_once: Rc<dyn Fn() -> bool> = {
            let in_flight = Rc::new(Cell::new(false));
            Rc::new(move || {
                if in_flight.replace(true) {
                    return false;
                }
                let in_flight = in_flight.clone();
                let cid = cell_id.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    if let Ok(live_result) = crate::components::executor::tick_live_cell(&cid).await
                    {
                        let kind_str = live_kind_str(live_result.kind);
                        // try_ read: this task resumes after a worker round
                        // trip and the panel may have been disposed (cell
                        // re-run, output collapse, navigation) — a plain
                        // get_untracked on the disposed NodeRef panics and
                        // halts hydration.
                        if let Some(el) = content_ref.try_get_untracked().flatten() {
                            let el: &web_sys::HtmlElement = &el;
                            apply_live_content(el, kind_str, &live_result.content);
                        }
                        frame_number.update(|n| *n += 1);
                    }
                    in_flight.set(false);
                });
                true
            })
        };

        let raf = {
            let tick_once = tick_once.clone();
            RafLoop::new(fps, playing, move || tick_once())
        };

        // Render initial content once mounted & start the loop.
        let initial_content = content;
        Effect::new(move |_| {
            let Some(el) = content_ref.get() else {
                return;
            };
            let el: &web_sys::HtmlElement = &el;
            apply_live_content(el, &kind, &initial_content);

            // Kick off the loop (playing starts true).
            raf.start();
        });

        let toggle_play = move |_| raf.toggle(playing);
        let step = move |_| {
            tick_once();
        };

        view! {
            <div class="live-view-container">
                <div node_ref=content_ref class="live-view-content" />
                <div class="animation-controls">
                    <button class="animation-control-btn" on:click=toggle_play>
                        {move || if playing.get() {
                    view! { <Icon icon=icons::PAUSE/> }
                } else {
                    view! { <Icon icon=icons::RUN/> }
                }}
                    </button>
                    <button class="animation-control-btn" on:click=step>
                        <Icon icon=icons::STEP/>
                    </button>
                    <span class="animation-frame-counter">
                        {move || format!("Frame {}", frame_number.get())}
                    </span>
                    <span class="animation-fps-display">
                        {format!("{fps} fps")}
                    </span>
                </div>
            </div>
        }
        .into_any()
    }

    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (fps, kind, content, cell_id);
        view! {
            <div class="live-view-container">
                <div>"LiveView (SSR placeholder)"</div>
            </div>
        }
        .into_any()
    }
}

#[cfg(test)]
mod tests {
    use super::{live_kind_str, render_live_content, LiveContent};

    #[test]
    fn live_kind_codes_decode_to_kind_strings() {
        assert_eq!(live_kind_str(0), "text");
        assert_eq!(live_kind_str(1), "html");
        assert_eq!(live_kind_str(2), "markdown");
        // An unknown code renders as plain text, never as HTML.
        assert_eq!(live_kind_str(99), "text");
    }

    #[test]
    fn html_kind_is_sanitized() {
        // LiveView `html` output is untrusted; script/handlers must be stripped
        // before it is injected via inner_html into a viewer's origin.
        let LiveContent::Html(out) = render_live_content(
            "html",
            r#"<p onclick="steal()">hi</p><script>steal()</script>"#,
        ) else {
            panic!("html kind should produce Html");
        };
        assert!(out.contains("hi"), "kept text: {out}");
        assert!(!out.contains("<script"), "stripped script: {out}");
        assert!(!out.contains("onclick"), "stripped handler: {out}");
        assert!(!out.contains("steal"), "stripped payload: {out}");
    }

    #[test]
    fn markdown_kind_sanitizes_and_keeps_math() {
        // Markdown routes through the shared sanitizing renderer: raw HTML is
        // stripped, but KaTeX math span classes survive so math still renders.
        let LiveContent::Html(out) =
            render_live_content("markdown", r"$e^{i\pi}+1=0$ <script>steal()</script>")
        else {
            panic!("markdown kind should produce Html");
        };
        assert!(out.contains("math-inline"), "kept math class: {out}");
        assert!(!out.contains("<script"), "stripped script: {out}");
        assert!(!out.contains("steal"), "stripped payload: {out}");
    }

    #[test]
    fn text_kind_is_plain_not_html() {
        // Unknown/text kinds are set as text content, never interpreted as HTML.
        let LiveContent::Text(out) = render_live_content("text", "<b>not html</b>") else {
            panic!("text kind should produce Text");
        };
        assert_eq!(out, "<b>not html</b>");
    }
}
