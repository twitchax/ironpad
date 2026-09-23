//! The one `requestAnimationFrame` loop behind every ticking output panel:
//! `AnimationCanvas`, `SimulationCanvas` and `LiveViewPanel` (hydrate-only).
//!
//! Each panel supplies only its per-frame body. The loop owns the invariants
//! that used to be restated in three copies:
//!
//! - **No self-referential cycle.** The frame closure reschedules itself
//!   through a *weak* handle; the only strong reference lives in a
//!   component-scoped [`StoredValue`], so disposing the panel frees the
//!   closure and everything it captured (frame data, a canvas context).
//! - **A paused loop stops.** A frame that finds `playing` false clears the
//!   pending id and does not reschedule, rather than spinning ~60 times a
//!   second doing nothing. [`RafLoop::start`] is what brings it back.
//! - **One loop, however fast the toggling.** `start` is a no-op while a
//!   frame is already scheduled, so a pause/play faster than a frame fires
//!   cannot add a second loop, which would reschedule itself forever.
//! - **Cleanup cancels.** The pending frame is cancelled on unmount, and the
//!   loop's own reads are disposal-safe.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};

use leptos::prelude::*;
use wasm_bindgen::prelude::*;

type FrameClosure = Closure<dyn FnMut(f64)>;
type FrameSlot = Rc<RefCell<Option<FrameClosure>>>;

/// A throttled `requestAnimationFrame` loop. `Copy`, so event handlers and
/// effects capture it by value.
#[derive(Clone, Copy)]
pub(crate) struct RafLoop {
    /// The pending frame's id, `None` while the loop is stopped.
    raf_id: RwSignal<Option<i32>>,
    /// The strong owner of the frame closure, for the component's lifetime.
    holder: StoredValue<Option<FrameSlot>, LocalStorage>,
}

/// Milliseconds between frames at `fps`; 0 fps runs at one frame a second.
fn frame_interval_ms(fps: u32) -> f64 {
    if fps > 0 {
        1000.0 / f64::from(fps)
    } else {
        1000.0
    }
}

/// Ask for the next frame and record its id.
fn request_frame(closure: &FrameClosure, raf_id: RwSignal<Option<i32>>) {
    let Some(window) = web_sys::window() else {
        return;
    };
    if let Ok(id) = window.request_animation_frame(closure.as_ref().unchecked_ref()) {
        raf_id.set(Some(id));
    }
}

impl RafLoop {
    /// Build a loop throttled to `fps` that runs while `playing` holds.
    ///
    /// `on_frame` runs at most once per frame interval and returns whether it
    /// took the frame: `false` (a tick still in flight, say) leaves the
    /// interval clock where it was, so the next frame tries again instead of
    /// waiting out a whole interval. The loop is idle until [`Self::start`];
    /// its pending frame is cancelled when the calling component unmounts.
    pub(crate) fn new(
        fps: u32,
        playing: RwSignal<bool>,
        mut on_frame: impl FnMut() -> bool + 'static,
    ) -> Self {
        let raf_id = RwSignal::new(None);
        let interval_ms = frame_interval_ms(fps);
        let last_time = Cell::new(0.0_f64);

        let slot: FrameSlot = Rc::new(RefCell::new(None));
        let weak: Weak<RefCell<Option<FrameClosure>>> = Rc::downgrade(&slot);
        *slot.borrow_mut() = Some(Closure::new(move |timestamp: f64| {
            // Disposal-safe: a disposed `playing` reads as paused.
            if !playing.try_get_untracked().unwrap_or(false) {
                raf_id.set(None);
                return;
            }
            if timestamp - last_time.get() >= interval_ms && on_frame() {
                last_time.set(timestamp);
            }
            if let Some(slot) = weak.upgrade() {
                if let Some(ref closure) = *slot.borrow() {
                    request_frame(closure, raf_id);
                }
            }
        }));

        let holder = StoredValue::new_local(Some(slot));
        on_cleanup(move || {
            if let (Some(id), Some(window)) =
                (raf_id.try_get_untracked().flatten(), web_sys::window())
            {
                let _ = window.cancel_animation_frame(id);
            }
            raf_id.set(None);
        });

        Self { raf_id, holder }
    }

    /// Start the loop, or restart one that paused itself. A no-op while a
    /// frame is already scheduled, which is what keeps it to one loop.
    pub(crate) fn start(self) {
        if self.raf_id.try_get_untracked().flatten().is_some() {
            return;
        }
        let raf_id = self.raf_id;
        self.holder.try_with_value(|slot| {
            if let Some(slot) = slot {
                if let Some(ref closure) = *slot.borrow() {
                    request_frame(closure, raf_id);
                }
            }
        });
    }

    /// The play/pause button: flip `playing`, and restart the loop when it
    /// resumes.
    pub(crate) fn toggle(self, playing: RwSignal<bool>) {
        playing.update(|p| *p = !*p);
        if playing.get_untracked() {
            self.start();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::frame_interval_ms;

    #[test]
    fn frame_interval_follows_fps_and_never_divides_by_zero() {
        assert!((frame_interval_ms(50) - 20.0).abs() < f64::EPSILON);
        assert!((frame_interval_ms(1) - 1000.0).abs() < f64::EPSILON);
        assert!((frame_interval_ms(0) - 1000.0).abs() < f64::EPSILON);
    }
}
