//! Process-wide options a profiling recording can switch on.

use std::sync::atomic::{AtomicBool, Ordering};

static UNCAP_FRAME_RATE: AtomicBool = AtomicBool::new(false);

/// Ask the engine to drop its target frame rate while a recording runs, so the
/// renderer goes as fast as it can instead of pacing to the display refresh.
///
/// This only records the request; the render thread and the presentation layer
/// read it. The recording UI sets it when a capture starts and clears it when the
/// capture stops.
pub fn set_uncap_frame_rate(uncapped: bool) {
    UNCAP_FRAME_RATE.store(uncapped, Ordering::Relaxed);
}

/// Whether the frame-rate cap is currently lifted for a recording.
#[inline]
pub fn uncap_frame_rate() -> bool {
    UNCAP_FRAME_RATE.load(Ordering::Relaxed)
}
