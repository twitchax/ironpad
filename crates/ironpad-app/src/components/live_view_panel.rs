// ── Live View panel component ────────────────────────────────────────────────
//
// Renders tick-driven live content (Text / HTML / Markdown) from a LiveView
// cell.  Mirrors the `SimulationCanvas` pattern in `animation_canvas.rs` but
// writes string content into a DOM element instead of pixel data onto a canvas.

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

// ── Markdown rendering (hydrate-only) ───────────────────────────────────────

#[cfg(feature = "hydrate")]
fn render_markdown(source: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};

    let mut opts = Options::empty();
    opts.insert(Options::ENABLE_TABLES);
    opts.insert(Options::ENABLE_STRIKETHROUGH);
    opts.insert(Options::ENABLE_MATH);
    let parser = Parser::new_ext(source, opts);
    let mut html_out = String::new();
    html::push_html(&mut html_out, parser);
    html_out
}

// ── Component ───────────────────────────────────────────────────────────────

/// Cancel a `requestAnimationFrame` by ID (if present).
#[cfg(feature = "hydrate")]
fn cancel_raf(id: Option<i32>) {
    if let Some(id) = id {
        let _ = web_sys::window().unwrap().cancel_animation_frame(id);
    }
}

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
        use std::cell::RefCell;
        use std::rc::Rc;

        use wasm_bindgen::prelude::*;

        type RafClosure = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;

        let content_ref = NodeRef::<leptos::html::Div>::new();
        let playing = RwSignal::new(true);
        let frame_number = RwSignal::new(0u32);
        let raf_id_signal = RwSignal::new(Option::<i32>::None);
        let tick_in_flight: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));

        let kind_rc: Rc<str> = Rc::from(kind.as_str());

        let cell_id_loop = cell_id.clone();
        let cell_id_step = cell_id.clone();

        // Apply content to the DOM element based on the content kind.
        let apply_content = {
            let kind_inner = kind_rc.clone();
            Rc::new(
                move |el: &web_sys::HtmlElement, kind_override: Option<&str>, text: &str| {
                    let k = kind_override.unwrap_or(&kind_inner);
                    match k {
                        "html" => {
                            el.set_inner_html(text);
                        }
                        "markdown" => {
                            let html = render_markdown(text);
                            el.set_inner_html(&html);
                            let _ = js::render_math_in(el);
                        }
                        // "text" and anything else: plain text.
                        _ => {
                            el.set_text_content(Some(text));
                        }
                    }
                },
            )
        };

        // Render initial content once mounted & start the rAF loop.
        let tick_in_flight_effect = tick_in_flight.clone();
        let apply_init = apply_content.clone();
        let apply_step = apply_content.clone();
        let initial_content = content;
        let kind_for_init = kind_rc.clone();

        Effect::new(move |_| {
            let Some(el) = content_ref.get() else {
                return;
            };
            let el: &web_sys::HtmlElement = &el;

            // Draw initial content.
            apply_init(el, Some(&kind_for_init), &initial_content);

            let frame_interval_ms = if fps > 0 {
                1000.0 / f64::from(fps)
            } else {
                1000.0
            };

            let last_time: Rc<RefCell<f64>> = Rc::new(RefCell::new(0.0));
            let cell_id_inner = cell_id_loop.clone();
            let tick_guard = tick_in_flight_effect.clone();
            let content_ref_loop = content_ref;
            let apply_loop = apply_content.clone();

            let cb: RafClosure = Rc::new(RefCell::new(None));
            let cb_clone = cb.clone();

            *cb.borrow_mut() = Some(Closure::new(move |timestamp: f64| {
                if !playing.get_untracked() {
                    *last_time.borrow_mut() = timestamp;
                    if let Some(ref closure) = *cb_clone.borrow() {
                        let id = web_sys::window()
                            .unwrap()
                            .request_animation_frame(closure.as_ref().unchecked_ref())
                            .unwrap();
                        raf_id_signal.set(Some(id));
                    }
                    return;
                }

                let dt = timestamp - *last_time.borrow();
                if dt >= frame_interval_ms && !*tick_guard.borrow() {
                    *last_time.borrow_mut() = timestamp;
                    *tick_guard.borrow_mut() = true;

                    let cid = cell_id_inner.clone();
                    let guard = tick_guard.clone();
                    let content_ref_tick = content_ref_loop;
                    let apply_tick = apply_loop.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        match crate::components::executor::tick_live_cell(&cid).await {
                            Ok(live_result) => {
                                let kind_str = match live_result.kind {
                                    1 => "html",
                                    2 => "markdown",
                                    // 0 and anything else: plain text.
                                    _ => "text",
                                };
                                if let Some(el) = content_ref_tick.get() {
                                    let el: &web_sys::HtmlElement = &el;
                                    apply_tick(el, Some(kind_str), &live_result.content);
                                }
                                frame_number.update(|n| *n += 1);
                            }
                            Err(_e) => {}
                        }
                        *guard.borrow_mut() = false;
                    });
                }

                if let Some(ref closure) = *cb_clone.borrow() {
                    let id = web_sys::window()
                        .unwrap()
                        .request_animation_frame(closure.as_ref().unchecked_ref())
                        .unwrap();
                    raf_id_signal.set(Some(id));
                }
            }));

            if let Some(ref closure) = *cb.borrow() {
                let id = web_sys::window()
                    .unwrap()
                    .request_animation_frame(closure.as_ref().unchecked_ref())
                    .unwrap();
                raf_id_signal.set(Some(id));
            };
        });

        // Cancel rAF on cleanup.
        on_cleanup(move || {
            cancel_raf(raf_id_signal.get_untracked());
            raf_id_signal.set(None);
        });

        let toggle_play = move |_| {
            playing.update(|p| *p = !*p);
        };

        let tick_in_flight_step = tick_in_flight;
        let step = move |_| {
            if *tick_in_flight_step.borrow() {
                return;
            }
            *tick_in_flight_step.borrow_mut() = true;

            let cid = cell_id_step.clone();
            let guard = tick_in_flight_step.clone();
            let content_ref_step = content_ref;
            let apply_s = apply_step.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match crate::components::executor::tick_live_cell(&cid).await {
                    Ok(live_result) => {
                        let kind_str = match live_result.kind {
                            1 => "html",
                            2 => "markdown",
                            // 0 and anything else: plain text.
                            _ => "text",
                        };
                        if let Some(el) = content_ref_step.get() {
                            let el: &web_sys::HtmlElement = &el;
                            apply_s(el, Some(kind_str), &live_result.content);
                        }
                        frame_number.update(|n| *n += 1);
                    }
                    Err(_e) => {}
                }
                *guard.borrow_mut() = false;
            });
        };

        view! {
            <div class="live-view-container">
                <div node_ref=content_ref class="live-view-content" />
                <div class="animation-controls">
                    <button class="animation-control-btn" on:click=toggle_play>
                        {move || if playing.get() { "⏸" } else { "▶" }}
                    </button>
                    <button class="animation-control-btn" on:click=step>
                        "⏭"
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
