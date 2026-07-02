//! GPU compute shader support via WebGPU.
//!
//! [`GpuCanvas`] lets cell authors write WGSL compute shaders that execute on
//! the GPU and produce [`Canvas`] output, with automatic CPU fallback when
//! WebGPU is unavailable.
//!
//! For live GPU-driven simulations (e.g., particle systems, fluid dynamics),
//! see the [`GpuSimulation`] trait, which dispatches a compute shader per tick
//! and falls back to CPU rendering automatically.
//!
//! # WGSL shader conventions
//!
//! Shaders must use the following binding layout:
//! - `@group(0) @binding(0) var<storage, read> uniforms: array<f32>` — uniform
//!   parameters, with width and height prepended as `uniforms[0]` and
//!   `uniforms[1]`.
//! - `@group(0) @binding(1) var<storage, read_write> output: array<vec4<f32>>`
//!   — output pixel data (RGBA, 0.0–1.0 range).
//!
//! A workgroup size of `@workgroup_size(16, 16)` is recommended for most use
//! cases.

use crate::canvas::Canvas;
use crate::SimSliderMeta;

// ── GPU FFI declarations ────────────────────────────────────────────────────

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "env")]
unsafe extern "C" {
    /// Returns 1 if WebGPU is available, 0 otherwise.
    fn ironpad_gpu_available() -> i32;
    /// Create a GPU buffer. Returns a handle (0 = error).
    /// Usage: 0=`STORAGE`, 1=`STORAGE|COPY_SRC`, 2=`MAP_READ|COPY_DST`, 3=`STORAGE|COPY_DST`
    fn ironpad_gpu_create_buffer(size: u32, usage: u32) -> u32;
    /// Write bytes from WASM linear memory into a GPU buffer.
    fn ironpad_gpu_write_buffer(handle: u32, src_ptr: u32, src_len: u32);
    /// Dispatch a WGSL compute shader. Returns 0 on success, 1 on error.
    fn ironpad_gpu_dispatch_compute(
        shader_ptr: u32,
        shader_len: u32,
        uniform_handle: u32,
        output_handle: u32,
        width: u32,
        height: u32,
    ) -> i32;
}

/// Check if WebGPU is available in the current execution environment.
pub fn gpu_available() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        unsafe { ironpad_gpu_available() == 1 }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
}

// ── GpuCanvas ───────────────────────────────────────────────────────────────

/// A compute-shader-driven canvas that executes on the GPU via WebGPU.
///
/// On conversion to [`CellOutput`](crate::CellOutput), the shader runs on the
/// GPU and results are read back as RGB pixels. When WebGPU is unavailable,
/// the optional CPU fallback closure is used instead.
///
/// # Example
///
/// ```ignore
/// use ironpad_cell::prelude::*;
///
/// let shader = r#"
///     @group(0) @binding(0) var<storage, read> uniforms: array<f32>;
///     @group(0) @binding(1) var<storage, read_write> output: array<vec4<f32>>;
///
///     @compute @workgroup_size(16, 16)
///     fn main(@builtin(global_invocation_id) id: vec3<u32>) {
///         let width = u32(uniforms[0]);
///         let height = u32(uniforms[1]);
///         if id.x >= width || id.y >= height { return; }
///         let idx = id.y * width + id.x;
///         output[idx] = vec4<f32>(f32(id.x) / f32(width), f32(id.y) / f32(height), 0.5, 1.0);
///     }
/// "#;
///
/// GpuCanvas::new(800, 600, shader, &[800.0, 600.0])
///     .with_fallback(|x, y, uniforms| {
///         let r = (x as f64 / uniforms[0] * 255.0) as u8;
///         let g = (y as f64 / uniforms[1] * 255.0) as u8;
///         (r, g, 128)
///     })
/// ```
type FallbackFn = Box<dyn Fn(u32, u32, &[f32]) -> (u8, u8, u8)>;

pub struct GpuCanvas {
    width: u32,
    height: u32,
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    shader: String,
    uniforms: Vec<f32>,
    fallback: Option<FallbackFn>,
}

impl GpuCanvas {
    /// Create a new GPU canvas with a WGSL compute shader.
    ///
    /// The shader must write to `output: array<vec4<f32>>` at binding 1,
    /// reading uniforms from `uniforms: array<f32>` at binding 0.
    /// Width and height are automatically prepended to the uniforms array
    /// as `uniforms[0]` and `uniforms[1]`.
    #[allow(clippy::cast_precision_loss)]
    pub fn new(width: u32, height: u32, shader: &str, uniforms: &[f32]) -> Self {
        let mut full_uniforms = vec![width as f32, height as f32];
        full_uniforms.extend_from_slice(uniforms);
        Self {
            width,
            height,
            shader: shader.to_string(),
            uniforms: full_uniforms,
            fallback: None,
        }
    }

    /// Attach a CPU fallback closure for when WebGPU is unavailable.
    ///
    /// The closure receives `(x, y, &uniforms)` and returns `(r, g, b)`.
    /// The uniforms slice includes the prepended width and height.
    pub fn with_fallback(mut self, f: impl Fn(u32, u32, &[f32]) -> (u8, u8, u8) + 'static) -> Self {
        self.fallback = Some(Box::new(f));
        self
    }

    /// Render the GPU canvas, returning a standard [`Canvas`].
    ///
    /// If WebGPU is available, dispatches the compute shader and requests
    /// async readback via host message. If unavailable (or no GPU), falls
    /// back to the CPU closure.
    #[allow(clippy::cast_possible_truncation)]
    pub fn render(self) -> Canvas {
        if gpu_available() {
            self.render_gpu()
        } else {
            self.render_cpu_inner()
        }
    }

    /// GPU rendering path.
    #[allow(clippy::cast_possible_truncation)]
    fn render_gpu(self) -> Canvas {
        #[cfg(target_arch = "wasm32")]
        {
            let uniform_bytes: Vec<u8> =
                self.uniforms.iter().flat_map(|f| f.to_le_bytes()).collect();
            let uniform_size = uniform_bytes.len() as u32;

            // Output buffer: vec4<f32> per pixel = 16 bytes per pixel.
            let output_size = self.width * self.height * 16;

            // Create GPU buffers.
            // Usage 3 = STORAGE|COPY_DST (for uniforms — written from CPU).
            let uniform_handle = unsafe { ironpad_gpu_create_buffer(uniform_size, 3) };
            // Usage 1 = STORAGE|COPY_SRC (for output — read by staging).
            let output_handle = unsafe { ironpad_gpu_create_buffer(output_size, 1) };
            // Usage 2 = MAP_READ|COPY_DST (for staging — read by CPU).
            let staging_handle = unsafe { ironpad_gpu_create_buffer(output_size, 2) };

            if uniform_handle == 0 || output_handle == 0 || staging_handle == 0 {
                return self.render_cpu_inner();
            }

            // Write uniforms to GPU.
            unsafe {
                ironpad_gpu_write_buffer(
                    uniform_handle,
                    uniform_bytes.as_ptr() as u32,
                    uniform_size,
                );
            }

            // Dispatch compute shader.
            let result = unsafe {
                ironpad_gpu_dispatch_compute(
                    self.shader.as_ptr() as u32,
                    self.shader.len() as u32,
                    uniform_handle,
                    output_handle,
                    self.width,
                    self.height,
                )
            };

            if result != 0 {
                return self.render_cpu_inner();
            }

            // Request async readback via host message.
            // The JS executor processes this AFTER cell_main returns,
            // reads the GPU output buffer, converts to RGB, and replaces
            // the placeholder BlobImage panel in the CellOutput.
            let readback_msg = serde_json::json!({
                "type": "gpu_read_pixels",
                "output_handle": output_handle,
                "staging_handle": staging_handle,
                "width": self.width,
                "height": self.height
            });
            crate::host_message(&readback_msg.to_string());

            // Return a placeholder canvas (gray).
            // The JS executor will replace this with the actual GPU output.
            Canvas::from_fn(self.width, self.height, |_x, _y| (128, 128, 128))
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            self.render_cpu_inner()
        }
    }

    /// Inner CPU rendering (shared by fallback and GPU-failure paths).
    fn render_cpu_inner(&self) -> Canvas {
        if let Some(f) = &self.fallback {
            let uniforms = &self.uniforms;
            Canvas::from_fn(self.width, self.height, |x, y| f(x, y, uniforms))
        } else {
            crate::host_message(
                &serde_json::json!({
                    "type": "warning",
                    "message": "WebGPU unavailable and no CPU fallback provided"
                })
                .to_string(),
            );
            Canvas::new(self.width, self.height)
        }
    }
}

// ── GpuSimulation ───────────────────────────────────────────────────────────

/// Trait for GPU-driven live simulations.
///
/// Analogous to [`Simulation`](crate::Simulation) but each tick dispatches a
/// WGSL compute shader on the GPU.  When WebGPU is unavailable the default
/// [`tick`](GpuSimulation::tick) implementation automatically falls back to
/// CPU rendering via [`tick_cpu`](GpuSimulation::tick_cpu).
///
/// Because `tick()` returns a [`Canvas`] (the same type `Simulation::tick`
/// returns), a `GpuSimulation` can be adapted to `Simulation` today without
/// scaffold changes:
///
/// ```ignore
/// struct MySim { /* … */ }
/// impl GpuSimulation for MySim { /* … */ }
/// impl Simulation for MySim {
///     fn init() -> Self { GpuSimulation::init() }
///     fn tick(&mut self) -> Canvas { GpuSimulation::tick(self) }
/// }
/// ```
///
/// # Example
///
/// ```ignore
/// use ironpad_cell::prelude::*;
///
/// struct Particles { frame: u32 }
///
/// impl GpuSimulation for Particles {
///     fn init() -> Self { Particles { frame: 0 } }
///     fn dimensions(&self) -> (u32, u32) { (400, 400) }
///     fn shader(&self) -> &str { "/* WGSL compute shader */" }
///     fn uniforms(&self) -> Vec<f32> { vec![self.frame as f32] }
///
///     fn tick_cpu(&mut self) -> Canvas {
///         self.frame += 1;
///         Canvas::from_fn(400, 400, |x, y| {
///             let v = ((self.frame + x + y) % 256) as u8;
///             (v, v, v)
///         })
///     }
/// }
/// ```
pub trait GpuSimulation: Sized + 'static {
    /// Create the initial simulation state.
    fn init() -> Self;

    /// Canvas dimensions `(width, height)`.
    fn dimensions(&self) -> (u32, u32);

    /// The WGSL compute shader source.
    ///
    /// The shader should read uniforms from `@group(0) @binding(0)` and write
    /// `vec4<f32>` pixel data to `@group(0) @binding(1)`.
    fn shader(&self) -> &str;

    /// Per-tick uniform values.
    ///
    /// Width and height are **not** included here — they are prepended
    /// automatically by [`GpuCanvas::new`].
    fn uniforms(&self) -> Vec<f32>;

    /// Advance simulation state and render one frame.
    ///
    /// The default implementation dispatches the compute shader when WebGPU is
    /// available and falls back to [`tick_cpu`](GpuSimulation::tick_cpu)
    /// otherwise.
    fn tick(&mut self) -> Canvas {
        if gpu_available() {
            let (w, h) = self.dimensions();
            GpuCanvas::new(w, h, self.shader(), &self.uniforms()).render()
        } else {
            self.tick_cpu()
        }
    }

    /// CPU fallback for when WebGPU is unavailable.
    ///
    /// The default returns a black canvas. Override to provide a meaningful
    /// software rasteriser.
    fn tick_cpu(&mut self) -> Canvas {
        let (w, h) = self.dimensions();
        Canvas::new(w, h)
    }

    /// Target frames per second (default: 30).
    fn fps() -> u32 {
        30
    }

    /// Slider declarations for this simulation (default: none).
    fn sliders() -> Vec<SimSliderMeta> {
        vec![]
    }
}

#[cfg(test)]
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_precision_loss,
    clippy::unnecessary_literal_bound
)]
mod tests {
    use super::*;

    #[test]
    fn gpu_available_returns_false_on_non_wasm() {
        assert!(!gpu_available());
    }

    #[test]
    fn gpu_canvas_without_fallback_renders_black() {
        let canvas = GpuCanvas::new(4, 4, "// unused", &[]).render();
        assert_eq!(canvas.width(), 4);
        assert_eq!(canvas.height(), 4);
        // On non-wasm, GPU is unavailable and no fallback → black canvas.
        assert_eq!(canvas.get_pixel(0, 0), (0, 0, 0));
    }

    #[test]
    fn gpu_canvas_with_fallback_uses_fallback_on_non_wasm() {
        let canvas = GpuCanvas::new(2, 2, "// unused", &[1.0, 2.0])
            .with_fallback(|x, y, uniforms| {
                // uniforms[0]=width(2), uniforms[1]=height(2), uniforms[2]=1.0, uniforms[3]=2.0
                let r = (x * 100) as u8;
                let g = (y * 100) as u8;
                let b = (uniforms[2] * 50.0) as u8;
                (r, g, b)
            })
            .render();

        assert_eq!(canvas.width(), 2);
        assert_eq!(canvas.height(), 2);
        assert_eq!(canvas.get_pixel(0, 0), (0, 0, 50));
        assert_eq!(canvas.get_pixel(1, 0), (100, 0, 50));
        assert_eq!(canvas.get_pixel(0, 1), (0, 100, 50));
        assert_eq!(canvas.get_pixel(1, 1), (100, 100, 50));
    }

    #[test]
    fn uniforms_prepend_width_and_height() {
        let gc = GpuCanvas::new(800, 600, "// shader", &[3.125, 2.72]);
        // uniforms should be [800.0, 600.0, 3.125, 2.72]
        let canvas = gc
            .with_fallback(|_x, _y, u| {
                assert_eq!(u.len(), 4);
                assert!((u[0] - 800.0).abs() < f32::EPSILON);
                assert!((u[1] - 600.0).abs() < f32::EPSILON);
                assert!((u[2] - 3.125).abs() < 0.01);
                assert!((u[3] - 2.72).abs() < 0.01);
                (0, 0, 0)
            })
            .render();
        assert_eq!(canvas.width(), 800);
    }

    #[test]
    fn gpu_canvas_converts_to_cell_output() {
        use crate::{CellOutput, IntoPanels};
        let gc = GpuCanvas::new(2, 2, "// unused", &[]).with_fallback(|_, _, _| (255, 0, 0));
        let output: CellOutput = gc.into();
        assert!(!output.into_panels().is_empty());
    }

    // ── GpuSimulation tests ─────────────────────────────────────────────────

    #[test]
    fn gpu_simulation_tick_uses_cpu_fallback() {
        struct TestSim {
            frame: u32,
        }
        impl GpuSimulation for TestSim {
            fn init() -> Self {
                TestSim { frame: 0 }
            }
            fn dimensions(&self) -> (u32, u32) {
                (4, 4)
            }
            fn shader(&self) -> &str {
                "// unused on CPU"
            }
            fn uniforms(&self) -> Vec<f32> {
                vec![self.frame as f32]
            }
            fn tick_cpu(&mut self) -> Canvas {
                self.frame += 1;
                Canvas::from_fn(4, 4, |x, y| {
                    let v = (self.frame * 10) as u8;
                    (v, x as u8, y as u8)
                })
            }
        }

        let mut sim = TestSim::init();
        // GPU unavailable on native → falls back to tick_cpu.
        let frame1 = sim.tick();
        assert_eq!(frame1.width(), 4);
        assert_eq!(frame1.height(), 4);
        assert_eq!(frame1.get_pixel(0, 0), (10, 0, 0));

        let frame2 = sim.tick();
        assert_eq!(frame2.get_pixel(0, 0), (20, 0, 0));
    }

    #[test]
    fn gpu_simulation_default_tick_cpu_returns_black() {
        struct MinSim;
        impl GpuSimulation for MinSim {
            fn init() -> Self {
                MinSim
            }
            fn dimensions(&self) -> (u32, u32) {
                (2, 2)
            }
            fn shader(&self) -> &str {
                ""
            }
            fn uniforms(&self) -> Vec<f32> {
                vec![]
            }
        }

        let mut sim = MinSim::init();
        let canvas = sim.tick();
        assert_eq!(canvas.width(), 2);
        assert_eq!(canvas.height(), 2);
        assert_eq!(canvas.get_pixel(0, 0), (0, 0, 0));
    }

    #[test]
    fn gpu_simulation_default_fps() {
        struct Sim;
        impl GpuSimulation for Sim {
            fn init() -> Self {
                Sim
            }
            fn dimensions(&self) -> (u32, u32) {
                (1, 1)
            }
            fn shader(&self) -> &str {
                ""
            }
            fn uniforms(&self) -> Vec<f32> {
                vec![]
            }
        }
        assert_eq!(Sim::fps(), 30);
    }

    #[test]
    fn gpu_simulation_default_sliders_empty() {
        struct Sim;
        impl GpuSimulation for Sim {
            fn init() -> Self {
                Sim
            }
            fn dimensions(&self) -> (u32, u32) {
                (1, 1)
            }
            fn shader(&self) -> &str {
                ""
            }
            fn uniforms(&self) -> Vec<f32> {
                vec![]
            }
        }
        assert!(Sim::sliders().is_empty());
    }

    #[test]
    fn gpu_simulation_custom_fps_and_sliders() {
        struct Sim;
        impl GpuSimulation for Sim {
            fn init() -> Self {
                Sim
            }
            fn dimensions(&self) -> (u32, u32) {
                (1, 1)
            }
            fn shader(&self) -> &str {
                ""
            }
            fn uniforms(&self) -> Vec<f32> {
                vec![]
            }
            fn fps() -> u32 {
                60
            }
            fn sliders() -> Vec<crate::SimSliderMeta> {
                vec![crate::SimSliderMeta {
                    key: "speed".into(),
                    min: 0.0,
                    max: 10.0,
                    step: 0.1,
                    label: "Speed".into(),
                    default: 5.0,
                }]
            }
        }
        assert_eq!(Sim::fps(), 60);
        assert_eq!(Sim::sliders().len(), 1);
        assert_eq!(Sim::sliders()[0].key, "speed");
    }

    // ── Edge-case & coverage tests ──────────────────────────────────────────

    #[test]
    fn gpu_canvas_zero_width() {
        let canvas = GpuCanvas::new(0, 4, "// unused", &[]).render();
        assert_eq!(canvas.width(), 0);
        assert_eq!(canvas.height(), 4);
    }

    #[test]
    fn gpu_canvas_zero_height() {
        let canvas = GpuCanvas::new(4, 0, "// unused", &[]).render();
        assert_eq!(canvas.width(), 4);
        assert_eq!(canvas.height(), 0);
    }

    #[test]
    fn gpu_canvas_zero_dimensions() {
        let canvas = GpuCanvas::new(0, 0, "// unused", &[]).render();
        assert_eq!(canvas.width(), 0);
        assert_eq!(canvas.height(), 0);
    }

    #[test]
    fn gpu_canvas_large_dimensions() {
        let canvas = GpuCanvas::new(4096, 2160, "// unused", &[])
            .with_fallback(|x, y, _| {
                let r = (x % 256) as u8;
                let g = (y % 256) as u8;
                (r, g, 0)
            })
            .render();
        assert_eq!(canvas.width(), 4096);
        assert_eq!(canvas.height(), 2160);
        assert_eq!(canvas.get_pixel(255, 255), (255, 255, 0));
        assert_eq!(canvas.get_pixel(256, 256), (0, 0, 0));
    }

    #[test]
    fn gpu_canvas_1x1() {
        let canvas = GpuCanvas::new(1, 1, "// unused", &[42.0])
            .with_fallback(|_, _, u| {
                let v = u[2] as u8;
                (v, v, v)
            })
            .render();
        assert_eq!(canvas.width(), 1);
        assert_eq!(canvas.height(), 1);
        assert_eq!(canvas.get_pixel(0, 0), (42, 42, 42));
    }

    #[test]
    fn gpu_canvas_empty_user_uniforms() {
        // Only width and height should be prepended.
        let canvas = GpuCanvas::new(3, 5, "// unused", &[])
            .with_fallback(|_x, _y, u| {
                assert_eq!(u.len(), 2);
                assert!((u[0] - 3.0).abs() < f32::EPSILON);
                assert!((u[1] - 5.0).abs() < f32::EPSILON);
                (1, 2, 3)
            })
            .render();
        assert_eq!(canvas.get_pixel(0, 0), (1, 2, 3));
    }

    #[test]
    fn gpu_canvas_render_gpu_falls_back_to_cpu_on_non_wasm() {
        // render() internally calls render_gpu() on the GPU path, or
        // render_cpu_inner() directly. On native, gpu_available() is false
        // so render() always goes through render_cpu_inner().
        let canvas = GpuCanvas::new(2, 2, "// shader ignored", &[])
            .with_fallback(|x, y, _| ((x * 50) as u8, (y * 50) as u8, 99))
            .render();
        assert_eq!(canvas.get_pixel(0, 0), (0, 0, 99));
        assert_eq!(canvas.get_pixel(1, 1), (50, 50, 99));
    }

    #[test]
    fn gpu_canvas_into_panels_returns_placeholder() {
        use crate::IntoPanels;
        let gc = GpuCanvas::new(2, 2, "// unused", &[]);
        let panels = gc.into_panels();
        assert_eq!(panels.len(), 1);
        match &panels[0] {
            crate::DisplayPanel::Text(t) => {
                assert!(
                    t.contains("GpuCanvas"),
                    "expected placeholder text, got: {t}"
                );
            }
            other => panic!("expected Text panel, got: {other:?}"),
        }
    }

    #[test]
    fn gpu_canvas_type_tag() {
        use crate::TypeTag;
        assert_eq!(GpuCanvas::type_tag(), "GpuCanvas");
    }

    #[test]
    fn gpu_canvas_from_cell_output_has_panels_and_type_tag() {
        use crate::{CellOutput, IntoPanels};
        let gc = GpuCanvas::new(3, 3, "// unused", &[]).with_fallback(|_, _, _| (10, 20, 30));
        let output: CellOutput = gc.into();
        let panels = output.into_panels();
        assert!(!panels.is_empty());
        // The From impl renders the canvas, producing a BlobImage panel.
        match &panels[0] {
            crate::DisplayPanel::BlobImage { mime_type, .. } => {
                assert_eq!(mime_type, "image/bmp");
            }
            other => panic!("expected BlobImage panel from rendered canvas, got: {other:?}"),
        }
    }

    #[test]
    fn gpu_simulation_multiple_ticks_advance_state() {
        struct Counter {
            count: u32,
        }
        impl GpuSimulation for Counter {
            fn init() -> Self {
                Counter { count: 0 }
            }
            fn dimensions(&self) -> (u32, u32) {
                (2, 2)
            }
            fn shader(&self) -> &str {
                ""
            }
            fn uniforms(&self) -> Vec<f32> {
                vec![self.count as f32]
            }
            fn tick_cpu(&mut self) -> Canvas {
                self.count += 1;
                let v = self.count as u8;
                Canvas::from_fn(2, 2, |_, _| (v, 0, 0))
            }
        }

        let mut sim = Counter::init();
        assert_eq!(sim.count, 0);

        let c1 = sim.tick();
        assert_eq!(sim.count, 1);
        assert_eq!(c1.get_pixel(0, 0), (1, 0, 0));

        let c2 = sim.tick();
        assert_eq!(sim.count, 2);
        assert_eq!(c2.get_pixel(0, 0), (2, 0, 0));

        let c3 = sim.tick();
        assert_eq!(sim.count, 3);
        assert_eq!(c3.get_pixel(0, 0), (3, 0, 0));
    }

    #[test]
    fn gpu_simulation_init_returns_valid_state() {
        struct InitSim {
            ready: bool,
        }
        impl GpuSimulation for InitSim {
            fn init() -> Self {
                InitSim { ready: true }
            }
            fn dimensions(&self) -> (u32, u32) {
                (8, 8)
            }
            fn shader(&self) -> &str {
                "// init shader"
            }
            fn uniforms(&self) -> Vec<f32> {
                vec![1.0, 2.0, 3.0]
            }
        }

        let sim = InitSim::init();
        assert!(sim.ready);
        assert_eq!(sim.dimensions(), (8, 8));
        assert_eq!(sim.shader(), "// init shader");
        assert_eq!(sim.uniforms(), vec![1.0, 2.0, 3.0]);
    }

    #[test]
    fn gpu_canvas_fallback_receives_all_uniforms() {
        let user_uniforms: Vec<f32> = vec![10.0, 20.0, 30.0, 40.0, 50.0];
        let canvas = GpuCanvas::new(1, 1, "// unused", &user_uniforms)
            .with_fallback(move |_x, _y, u| {
                // Width=1, Height=1, then user uniforms.
                assert_eq!(u.len(), 7);
                assert!((u[0] - 1.0).abs() < f32::EPSILON);
                assert!((u[1] - 1.0).abs() < f32::EPSILON);
                for (i, &expected) in user_uniforms.iter().enumerate() {
                    assert!((u[i + 2] - expected).abs() < f32::EPSILON);
                }
                (u.len() as u8, 0, 0)
            })
            .render();
        assert_eq!(canvas.get_pixel(0, 0), (7, 0, 0));
    }
}
