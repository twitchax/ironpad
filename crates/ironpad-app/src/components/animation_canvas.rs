// ── Animation / Simulation canvas components ────────────────────────────────
//
// Shared canvas-based rendering for `DisplayPanel::Animation` (precomputed
// frame sequences) and `DisplayPanel::Simulation` (live tick-driven frames).
// Both are used from `cell_output.rs` (editor) and `view_only_notebook.rs`.

use leptos::prelude::*;

// ── JS-side helpers (hydrate-only) ──────────────────────────────────────────
//
// All heavy pixel work (base64 decode, RGB→RGBA expansion, ImageData creation)
// is done in a single JS call to avoid copying large buffers across the WASM
// boundary.

/// Draw a base64-encoded RGB frame to a canvas entirely in JS.
///
/// Decodes base64, expands RGB→RGBA, builds an `ImageData`, and calls
/// `putImageData` — all without crossing the WASM boundary.
#[cfg(feature = "hydrate")]
fn draw_b64_rgb_to_canvas(
    ctx: &web_sys::CanvasRenderingContext2d,
    b64: &str,
    width: u32,
    height: u32,
) {
    use js_sys::Function;
    use wasm_bindgen::JsValue;

    let func = Function::new_with_args(
        "ctx,b64,w,h",
        "var s=atob(b64),n=s.length,a=new Uint8ClampedArray(n/3*4);\
         for(var i=0,j=0;i<n;i+=3,j+=4){a[j]=s.charCodeAt(i);a[j+1]=s.charCodeAt(i+1);a[j+2]=s.charCodeAt(i+2);a[j+3]=255;}\
         ctx.putImageData(new ImageData(a,w,h),0,0)",
    );
    let _ = func.call4(
        &JsValue::NULL,
        ctx,
        &JsValue::from_str(b64),
        &JsValue::from_f64(f64::from(width)),
        &JsValue::from_f64(f64::from(height)),
    );
}

/// Draw raw RGB bytes (already in WASM memory) to a canvas.
///
/// Expands RGB→RGBA and calls `putImageData` in one JS call.  The
/// `Uint8Array` view is zero-copy into JS.
#[cfg(feature = "hydrate")]
fn draw_rgb_to_canvas(
    ctx: &web_sys::CanvasRenderingContext2d,
    rgb: &[u8],
    width: u32,
    height: u32,
) {
    use js_sys::Function;
    use wasm_bindgen::JsValue;

    let js_rgb = js_sys::Uint8Array::from(rgb);
    let func = Function::new_with_args(
        "ctx,rgb,w,h",
        "var n=rgb.length,a=new Uint8ClampedArray(n/3*4);\
         for(var i=0,j=0;i<n;i+=3,j+=4){a[j]=rgb[i];a[j+1]=rgb[i+1];a[j+2]=rgb[i+2];a[j+3]=255;}\
         ctx.putImageData(new ImageData(a,w,h),0,0)",
    );
    let _ = func.call4(
        &JsValue::NULL,
        ctx,
        &js_rgb,
        &JsValue::from_f64(f64::from(width)),
        &JsValue::from_f64(f64::from(height)),
    );
}

/// Decode a base64 RGB blob into per-frame RGBA `Uint8Array`s entirely in JS.
///
/// Returns a `js_sys::Array` of `Uint8Array` (one per frame, already RGBA).
/// All work happens JS-side to avoid copying the full animation buffer into
/// WASM linear memory.
#[cfg(feature = "hydrate")]
fn decode_animation_frames(b64: &str, frame_rgb_size: u32, frame_count: u32) -> js_sys::Array {
    use js_sys::Function;
    use wasm_bindgen::JsValue;

    let func = Function::new_with_args(
        "b64,fsz,fc",
        "var s=atob(b64),out=[];\
         for(var f=0;f<fc;f++){\
           var off=f*fsz,a=new Uint8ClampedArray(fsz/3*4);\
           for(var i=0,j=0;i<fsz;i+=3,j+=4){var p=off+i;a[j]=s.charCodeAt(p);a[j+1]=s.charCodeAt(p+1);a[j+2]=s.charCodeAt(p+2);a[j+3]=255;}\
           out.push(a);}\
         return out",
    );
    let result = func
        .call3(
            &JsValue::NULL,
            &JsValue::from_str(b64),
            &JsValue::from_f64(f64::from(frame_rgb_size)),
            &JsValue::from_f64(f64::from(frame_count)),
        )
        .unwrap_or_else(|_| JsValue::from(js_sys::Array::new()));
    js_sys::Array::from(&result)
}

/// Draw a pre-decoded RGBA `Uint8ClampedArray` frame to a canvas context.
#[cfg(feature = "hydrate")]
fn draw_js_frame_to_canvas(
    ctx: &web_sys::CanvasRenderingContext2d,
    rgba_clamped: &wasm_bindgen::JsValue,
    width: u32,
    height: u32,
) {
    use js_sys::Function;
    use wasm_bindgen::JsValue;

    let func = Function::new_with_args(
        "ctx,a,w,h",
        "ctx.putImageData(new ImageData(a,w,h),0,0)",
    );
    let _ = func.call4(
        &JsValue::NULL,
        ctx,
        rgba_clamped,
        &JsValue::from_f64(f64::from(width)),
        &JsValue::from_f64(f64::from(height)),
    );
}

/// Cancel a `requestAnimationFrame` by ID (if present).
#[cfg(feature = "hydrate")]
fn cancel_raf(id: Option<i32>) {
    if let Some(id) = id {
        let _ = web_sys::window().unwrap().cancel_animation_frame(id);
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

        use wasm_bindgen::prelude::*;

        type RafClosure = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;

        let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
        let playing = RwSignal::new(true);
        let current_frame = RwSignal::new(0u32);
        let raf_id_signal = RwSignal::new(Option::<i32>::None);

        // Decode all frames to RGBA entirely in JS — never copies bulk pixel
        // data into WASM linear memory.
        let frame_rgb_size = width * height * 3;
        let frames: Rc<js_sys::Array> =
            Rc::new(decode_animation_frames(&data, frame_rgb_size, frame_count));

        let frames_effect = frames.clone();
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
            if frames_effect.length() > 0 {
                draw_js_frame_to_canvas(&ctx, &frames_effect.get(0), width, height);
            }

            let frames_loop = frames_effect.clone();
            let frame_interval_ms = if fps > 0 {
                1000.0 / f64::from(fps)
            } else {
                1000.0
            };
            let last_time: Rc<RefCell<f64>> = Rc::new(RefCell::new(0.0));

            let cb: RafClosure = Rc::new(RefCell::new(None));
            let cb_clone = cb.clone();

            let total = frames_loop.length();
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
                if dt >= frame_interval_ms {
                    *last_time.borrow_mut() = timestamp;
                    let idx = current_frame.get_untracked();
                    draw_js_frame_to_canvas(&ctx, &frames_loop.get(idx), width, height);
                    current_frame.set((idx + 1) % total.max(1));
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

        // Cancel rAF on cleanup (T-012).
        on_cleanup(move || {
            cancel_raf(raf_id_signal.get_untracked());
            raf_id_signal.set(None);
        });

        let toggle_play = move |_| {
            playing.update(|p| *p = !*p);
        };

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
                        {move || if playing.get() { "⏸" } else { "▶" }}
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
) -> impl IntoView {
    #[cfg(feature = "hydrate")]
    {
        use std::cell::RefCell;
        use std::rc::Rc;

        use wasm_bindgen::prelude::*;

        type RafClosure = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;

        let canvas_ref = NodeRef::<leptos::html::Canvas>::new();
        let playing = RwSignal::new(true);
        let frame_number = RwSignal::new(0u32);
        let raf_id_signal = RwSignal::new(Option::<i32>::None);
        let tick_in_flight: Rc<RefCell<bool>> = Rc::new(RefCell::new(false));

        let ctx_cell: Rc<RefCell<Option<web_sys::CanvasRenderingContext2d>>> =
            Rc::new(RefCell::new(None));

        let cell_id_loop = cell_id.clone();
        let cell_id_step = cell_id.clone();

        let ctx_cell_effect = ctx_cell.clone();
        let tick_in_flight_effect = tick_in_flight.clone();
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
            draw_b64_rgb_to_canvas(&ctx, &first_frame_data, width, height);

            *ctx_cell_effect.borrow_mut() = Some(ctx);

            let frame_interval_ms = if fps > 0 {
                1000.0 / f64::from(fps)
            } else {
                1000.0
            };
            let last_time: Rc<RefCell<f64>> = Rc::new(RefCell::new(0.0));
            let ctx_loop = ctx_cell_effect.clone();
            let cell_id_inner = cell_id_loop.clone();
            let tick_guard = tick_in_flight_effect.clone();

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

                    let ctx_tick = ctx_loop.clone();
                    let cid = cell_id_inner.clone();
                    let guard = tick_guard.clone();
                    wasm_bindgen_futures::spawn_local(async move {
                        match crate::components::executor::tick_cell(&cid).await {
                            Ok(tick_result) => {
                                if let Some(ref ctx) = *ctx_tick.borrow() {
                                    draw_rgb_to_canvas(
                                        ctx,
                                        &tick_result.rgb_bytes,
                                        tick_result.width,
                                        tick_result.height,
                                    );
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

        // Cancel rAF on cleanup (T-012).
        on_cleanup(move || {
            cancel_raf(raf_id_signal.get_untracked());
            raf_id_signal.set(None);
        });

        let toggle_play = move |_| {
            playing.update(|p| *p = !*p);
        };

        let ctx_step = ctx_cell;
        let tick_in_flight_step = tick_in_flight;
        let step = move |_| {
            if *tick_in_flight_step.borrow() {
                return;
            }
            *tick_in_flight_step.borrow_mut() = true;

            let ctx_s = ctx_step.clone();
            let cid = cell_id_step.clone();
            let guard = tick_in_flight_step.clone();
            wasm_bindgen_futures::spawn_local(async move {
                match crate::components::executor::tick_cell(&cid).await {
                    Ok(tick_result) => {
                        if let Some(ref ctx) = *ctx_s.borrow() {
                            draw_rgb_to_canvas(
                                ctx,
                                &tick_result.rgb_bytes,
                                tick_result.width,
                                tick_result.height,
                            );
                        }
                        frame_number.update(|n| *n += 1);
                    }
                    Err(_e) => {}
                }
                *guard.borrow_mut() = false;
            });
        };

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
        let _ = (width, height, fps, first_frame_data, cell_id);
        view! {
            <div class="animation-canvas-container">
                <div>{format!("Simulation at {fps} fps ({width}×{height})")}</div>
            </div>
        }
        .into_any()
    }
}
