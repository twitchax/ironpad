/// Cross-platform stopwatch that works on both native and `wasm32` targets.
///
/// On WASM, `std::time::Instant` panics ("time not implemented on this
/// platform"). This type uses `js_sys::Date::now()` on WASM and
/// `std::time::Instant` on native, providing a consistent API for
/// measuring elapsed wall-clock time in notebook cells.
pub struct Stopwatch {
    #[cfg(target_arch = "wasm32")]
    start_ms: f64,
    #[cfg(not(target_arch = "wasm32"))]
    start: std::time::Instant,
}

impl Stopwatch {
    /// Start a new stopwatch.
    #[must_use]
    pub fn new() -> Self {
        Self {
            #[cfg(target_arch = "wasm32")]
            start_ms: js_sys::Date::now(),
            #[cfg(not(target_arch = "wasm32"))]
            start: std::time::Instant::now(),
        }
    }

    /// Milliseconds elapsed since the stopwatch was created.
    #[must_use]
    pub fn elapsed_ms(&self) -> f64 {
        #[cfg(target_arch = "wasm32")]
        {
            js_sys::Date::now() - self.start_ms
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.start.elapsed().as_secs_f64() * 1000.0
        }
    }

    /// Seconds elapsed since the stopwatch was created.
    #[must_use]
    pub fn elapsed_secs(&self) -> f64 {
        self.elapsed_ms() / 1000.0
    }
}

impl Default for Stopwatch {
    fn default() -> Self {
        Self::new()
    }
}
