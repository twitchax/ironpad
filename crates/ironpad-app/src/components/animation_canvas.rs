// ── Animation / Simulation canvas components ────────────────────────────────
//
// Shared canvas-based rendering for `DisplayPanel::Animation` (precomputed
// frame sequences) and `DisplayPanel::Simulation` (live tick-driven frames).
// Both are used from `cell_output.rs` (editor) and `view_only_notebook.rs`.

use crate::components::icon::Icon;
use crate::components::icons;
#[cfg(feature = "hydrate")]
use crate::components::output_render::sim_bus_js;
use leptos::prelude::*;

/// Mirror of `ironpad_cell::SimSliderMeta` for use within the app rendering pipeline.
///
/// Defined here to avoid introducing `ironpad-cell` as a direct dependency of `ironpad-app`.
/// Serializes/deserializes identically to the cell-side type, which
/// `components/cell_contract_tests.rs` checks through the `Simulation` panel.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SimSliderMeta {
    pub key: String,
    pub min: f64,
    pub max: f64,
    pub step: f64,
    pub label: String,
    pub default: f64,
}

// ── JS-side helpers (hydrate-only) ──────────────────────────────────────────
//
// All heavy pixel work (base64 decode, RGB→RGBA expansion, ImageData creation)
// happens JS-side, so bulk pixel data never crosses the WASM boundary. One
// `inline_js` module compiled once at load: these run once per animation
// frame and per simulation tick, which is too hot for `new Function(..)`.
// Every entry `catch`es so a malformed frame (a length that does not match
// its dimensions) is skipped rather than thrown through the rAF loop.

#[cfg(feature = "hydrate")]
mod draw {
    use wasm_bindgen::prelude::*;

    #[wasm_bindgen(inline_js = "
        export function draw_rgba(ctx, a, w, h) {
            ctx.putImageData(new ImageData(a, w, h), 0, 0);
        }
        export function draw_rgb(ctx, rgb, w, h) {
            var n = rgb.length, a = new Uint8ClampedArray(n / 3 * 4);
            for (var i = 0, j = 0; i < n; i += 3, j += 4) {
                a[j] = rgb[i]; a[j + 1] = rgb[i + 1]; a[j + 2] = rgb[i + 2]; a[j + 3] = 255;
            }
            ctx.putImageData(new ImageData(a, w, h), 0, 0);
        }
        export function draw_b64_rgb(ctx, b64, w, h) {
            var s = atob(b64), n = s.length, a = new Uint8ClampedArray(n / 3 * 4);
            for (var i = 0, j = 0; i < n; i += 3, j += 4) {
                a[j] = s.charCodeAt(i); a[j + 1] = s.charCodeAt(i + 1);
                a[j + 2] = s.charCodeAt(i + 2); a[j + 3] = 255;
            }
            ctx.putImageData(new ImageData(a, w, h), 0, 0);
        }
        export function decode_frames(b64, fsz, fc) {
            var s = atob(b64), out = [];
            for (var f = 0; f < fc; f++) {
                var off = f * fsz, a = new Uint8ClampedArray(fsz / 3 * 4);
                for (var i = 0, j = 0; i < fsz; i += 3, j += 4) {
                    var p = off + i;
                    a[j] = s.charCodeAt(p); a[j + 1] = s.charCodeAt(p + 1);
                    a[j + 2] = s.charCodeAt(p + 2); a[j + 3] = 255;
                }
                out.push(a);
            }
            return out;
        }
    ")]
    extern "C" {
        /// Draw a pre-decoded RGBA `Uint8ClampedArray` frame.
        #[wasm_bindgen(catch)]
        pub fn draw_rgba(
            ctx: &web_sys::CanvasRenderingContext2d,
            rgba: &JsValue,
            width: u32,
            height: u32,
        ) -> Result<(), JsValue>;

        /// Expand an RGB frame that already lives in JS memory (a simulation
        /// tick's `rgbBytes`) to RGBA and draw it.
        #[wasm_bindgen(catch)]
        pub fn draw_rgb(
            ctx: &web_sys::CanvasRenderingContext2d,
            rgb: &js_sys::Uint8Array,
            width: u32,
            height: u32,
        ) -> Result<(), JsValue>;

        /// Decode a base64 RGB frame, expand it to RGBA and draw it.
        #[wasm_bindgen(catch)]
        pub fn draw_b64_rgb(
            ctx: &web_sys::CanvasRenderingContext2d,
            b64: &str,
            width: u32,
            height: u32,
        ) -> Result<(), JsValue>;

        /// Decode a base64 blob of concatenated RGB frames into one RGBA
        /// `Uint8ClampedArray` per frame.
        #[wasm_bindgen(catch)]
        pub fn decode_frames(
            b64: &str,
            frame_rgb_size: u32,
            frame_count: u32,
        ) -> Result<js_sys::Array, JsValue>;
    }
}

// ── AnimationCanvas ─────────────────────────────────────────────────────────

/// Renders a precomputed multi-frame animation on a `<canvas>` element.
///
/// Decodes base64-encoded concatenated RGB frames, then drives a
/// `requestAnimationFrame` loop at the target `fps`.  Provides play/pause
/// toggle and a frame counter.
#[allow(clippy::needless_pass_by_value)]
#[component]
pub fn AnimationCanvas(
    width: u32,
    height: u32,
    fps: u32,
    frame_count: u32,
    data: String,
) -> impl IntoView {
    #[cfg(feature = "hydrate")]
    {
        use std::cell::RefCell;
        use std::rc::Rc;

        use wasm_bindgen::JsCast as _;

        use crate::components::raf_loop::RafLoop;

        let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
        let playing = RwSignal::new(true);
        let current_frame = RwSignal::new(0u32);

        // Decode all frames to RGBA entirely in JS — never copies bulk pixel
        // data into WASM linear memory.
        let frame_rgb_size = width * height * 3;
        let frames: Rc<js_sys::Array> = Rc::new(
            draw::decode_frames(&data, frame_rgb_size, frame_count)
                .unwrap_or_else(|_| js_sys::Array::new()),
        );
        // Filled once the canvas mounts; the loop draws nothing before that.
        let ctx_cell: Rc<RefCell<Option<web_sys::CanvasRenderingContext2d>>> =
            Rc::new(RefCell::new(None));

        let raf = {
            let frames = frames.clone();
            let ctx_cell = ctx_cell.clone();
            let total = frames.length();
            RafLoop::new(fps, playing, move || {
                if let Some(ref ctx) = *ctx_cell.borrow() {
                    let idx = current_frame.get_untracked();
                    let _ = draw::draw_rgba(ctx, &frames.get(idx), width, height);
                    current_frame.set((idx + 1) % total.max(1));
                }
                true
            })
        };

        Effect::new(move |_| {
            let Some(canvas) = canvas_ref.get() else {
                return;
            };
            let canvas: &web_sys::HtmlCanvasElement = &canvas;
            canvas.set_width(width);
            canvas.set_height(height);

            let ctx = canvas
                .get_context("2d")
                .ok()
                .flatten()
                .expect("2d context")
                .dyn_into::<web_sys::CanvasRenderingContext2d>()
                .expect("cast to CanvasRenderingContext2d");

            // Draw the first frame immediately.
            if frames.length() > 0 {
                let _ = draw::draw_rgba(&ctx, &frames.get(0), width, height);
            }
            *ctx_cell.borrow_mut() = Some(ctx);

            // Kick off the loop (playing starts true).
            raf.start();
        });

        let toggle_play = move |_| raf.toggle(playing);

        view! {
            <div class="animation-canvas-container">
                <canvas
                    node_ref=canvas_ref
                    width=width
                    height=height
                    style="image-rendering: pixelated;"
                />
                <div class="animation-controls">
                    <button class="animation-control-btn" on:click=toggle_play>
                        {move || if playing.get() {
                    view! { <Icon icon=icons::PAUSE/> }
                } else {
                    view! { <Icon icon=icons::RUN/> }
                }}
                    </button>
                    <span class="animation-frame-counter">
                        {move || format!("Frame {}/{}", current_frame.get() + 1, frame_count)}
                    </span>
                </div>
            </div>
        }
        .into_any()
    }

    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (width, height, fps, frame_count, data);
        view! {
            <div class="animation-canvas-container">
                <div>{format!("Animation: {frame_count} frames at {fps} fps ({width}×{height})")}</div>
            </div>
        }
        .into_any()
    }
}

// ── SimulationCanvas ────────────────────────────────────────────────────────

/// Renders a live simulation on a `<canvas>` element by calling `tick_cell()`
/// each frame at the target `fps`.
///
/// Draws the initial `first_frame_data` immediately, then starts a
/// `requestAnimationFrame` loop that fetches new frames from the executor.
/// Provides play/pause, step, and frame counter controls.
#[allow(clippy::needless_pass_by_value)]
#[component]
pub fn SimulationCanvas(
    width: u32,
    height: u32,
    fps: u32,
    first_frame_data: String,
    #[prop(into)] cell_id: String,
    #[prop(default = vec![])] sliders: Vec<SimSliderMeta>,
) -> impl IntoView {
    #[cfg(feature = "hydrate")]
    {
        use std::cell::{Cell, RefCell};
        use std::rc::Rc;

        use wasm_bindgen::JsCast as _;

        use crate::components::raf_loop::RafLoop;

        let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
        let playing = RwSignal::new(true);
        let frame_number = RwSignal::new(0u32);
        // Filled once the canvas mounts; a tick before that draws nothing.
        let ctx_cell: Rc<RefCell<Option<web_sys::CanvasRenderingContext2d>>> =
            Rc::new(RefCell::new(None));

        // One tick: fetch a frame from the executor and draw it. Shared by
        // the loop and the Step button, and refused while a tick is still in
        // flight (returning `false` so the loop retries on the next frame).
        let tick_once: Rc<dyn Fn() -> bool> = {
            let ctx_cell = ctx_cell.clone();
            let in_flight = Rc::new(Cell::new(false));
            Rc::new(move || {
                if in_flight.replace(true) {
                    return false;
                }
                let ctx_cell = ctx_cell.clone();
                let in_flight = in_flight.clone();
                let cid = cell_id.clone();
                wasm_bindgen_futures::spawn_local(async move {
                    if let Ok(tick) = crate::components::executor::tick_cell(&cid).await {
                        if let Some(ref ctx) = *ctx_cell.borrow() {
                            let _ = draw::draw_rgb(ctx, &tick.rgb_bytes, tick.width, tick.height);
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

        Effect::new(move |_| {
            let Some(canvas) = canvas_ref.get() else {
                return;
            };
            let canvas: &web_sys::HtmlCanvasElement = &canvas;
            canvas.set_width(width);
            canvas.set_height(height);

            let ctx = canvas
                .get_context("2d")
                .ok()
                .flatten()
                .expect("2d context")
                .dyn_into::<web_sys::CanvasRenderingContext2d>()
                .expect("cast to CanvasRenderingContext2d");

            // Draw first frame entirely in JS (base64 → RGB → RGBA → putImageData).
            let _ = draw::draw_b64_rgb(&ctx, &first_frame_data, width, height);
            *ctx_cell.borrow_mut() = Some(ctx);

            // Kick off the loop (playing starts true).
            raf.start();
        });

        let toggle_play = move |_| raf.toggle(playing);
        let step = move |_| {
            tick_once();
        };

        // Create a signal for each slider's current value.
        let slider_signals: Vec<(SimSliderMeta, RwSignal<f64>)> = sliders
            .iter()
            .map(|s| (s.clone(), RwSignal::new(s.default)))
            .collect();

        // Emit default values to the sim bus once on mount so the bus has
        // initial values before the first user interaction.
        let defaults: Vec<(String, f64)> =
            sliders.iter().map(|s| (s.key.clone(), s.default)).collect();
        Effect::new(move |_| {
            for (key, val) in &defaults {
                sim_bus_js::sim_bus_write_f64(key, *val);
            }
        });

        view! {
            <div class="animation-canvas-container">
                <canvas
                    node_ref=canvas_ref
                    width=width
                    height=height
                    style="image-rendering: pixelated;"
                />
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
                <div class="ironpad-sim-sliders">
                    {slider_signals.into_iter().map(|(slider, sig)| {
                        let key = slider.key.clone();
                        let default = slider.default;
                        view! {
                            <div class="ironpad-sim-slider">
                                <label>
                                    {format!("{}: ", slider.label)}
                                    <span class="ironpad-sim-slider-value">
                                        {move || sig.get().to_string()}
                                    </span>
                                </label>
                                <input
                                    type="range"
                                    min=slider.min.to_string()
                                    max=slider.max.to_string()
                                    step=slider.step.to_string()
                                    prop:value=move || sig.get().to_string()
                                    on:input=move |ev| {
                                        let v: f64 = event_target_value(&ev)
                                            .parse()
                                            .unwrap_or(default);
                                        sig.set(v);
                                        sim_bus_js::sim_bus_write_f64(&key, v);
                                    }
                                />
                            </div>
                        }
                    }).collect::<Vec<_>>()}
                </div>
            </div>
        }
        .into_any()
    }

    #[cfg(not(feature = "hydrate"))]
    {
        let _ = (width, height, fps, first_frame_data, cell_id, sliders);
        view! {
            <div class="animation-canvas-container">
                <div>{format!("Simulation at {fps} fps ({width}×{height})")}</div>
            </div>
        }
        .into_any()
    }
}
