// ── Animation / Simulation canvas components ────────────────────────────────
//
// Shared canvas-based rendering for `DisplayPanel::Animation` (precomputed
// frame sequences) and `DisplayPanel::Simulation` (live tick-driven frames).
// Both are used from `cell_output.rs` (editor) and `view_only_notebook.rs`.

use leptos::prelude::*;

// ── Base64 → raw bytes (hydrate-only) ───────────────────────────────────────

/// Decode a base64 string to raw bytes using the browser's `atob`.
#[cfg(feature = "hydrate")]
fn decode_base64(b64: &str) -> Vec<u8> {
    let window = web_sys::window().expect("window");
    let decoded = window.atob(b64).unwrap_or_default();
    decoded.as_bytes().to_vec()
}

/// Expand RGB bytes to RGBA (opaque alpha).
#[cfg(feature = "hydrate")]
fn rgb_to_rgba(rgb: &[u8]) -> Vec<u8> {
    let pixel_count = rgb.len() / 3;
    let mut rgba = Vec::with_capacity(pixel_count * 4);
    for chunk in rgb.chunks_exact(3) {
        rgba.push(chunk[0]);
        rgba.push(chunk[1]);
        rgba.push(chunk[2]);
        rgba.push(255);
    }
    rgba
}

/// Draw RGBA pixel data to a canvas 2D context.
#[cfg(feature = "hydrate")]
fn draw_rgba_to_canvas(
    ctx: &web_sys::CanvasRenderingContext2d,
    rgba: &[u8],
    width: u32,
    height: u32,
) {
    use wasm_bindgen::Clamped;

    let owned = rgba.to_vec();
    let image_data =
        web_sys::ImageData::new_with_u8_clamped_array_and_sh(Clamped(&owned), width, height)
            .expect("create ImageData");

    ctx.put_image_data(&image_data, 0.0, 0.0)
        .expect("putImageData");
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
        // Store the rAF ID in a signal so on_cleanup (Send+Sync) can cancel it.
        let raf_id_signal = RwSignal::new(Option::<i32>::None);

        // Decode all frames up front.
        let raw_bytes = decode_base64(&data);
        let frame_size = (width * height * 3) as usize;
        let frames: Rc<Vec<Vec<u8>>> = Rc::new(
            raw_bytes
                .chunks(frame_size)
                .take(frame_count as usize)
                .map(rgb_to_rgba)
                .collect(),
        );

        // Start the animation loop after mount.
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
            if let Some(frame) = frames_effect.first() {
                draw_rgba_to_canvas(&ctx, frame, width, height);
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

            #[allow(clippy::cast_possible_truncation)]
            let total = frames_loop.len() as u32;
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
                    if let Some(frame) = frames_loop.get(idx as usize) {
                        draw_rgba_to_canvas(&ctx, frame, width, height);
                    }
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

            let initial_rgb = decode_base64(&first_frame_data);
            let initial_rgba = rgb_to_rgba(&initial_rgb);
            draw_rgba_to_canvas(&ctx, &initial_rgba, width, height);

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
                                let rgba = rgb_to_rgba(&tick_result.rgb_bytes);
                                if let Some(ref ctx) = *ctx_tick.borrow() {
                                    draw_rgba_to_canvas(
                                        ctx,
                                        &rgba,
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
                        let rgba = rgb_to_rgba(&tick_result.rgb_bytes);
                        if let Some(ref ctx) = *ctx_s.borrow() {
                            draw_rgba_to_canvas(ctx, &rgba, tick_result.width, tick_result.height);
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
