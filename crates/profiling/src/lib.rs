//! Ultra-fast instrumentation-based profiling inspired by Unreal Insights
//!
//! Unlike sampling profilers that interrupt threads, this uses explicit instrumentation
//! macros that are compiled in and record exact timing with minimal overhead.
//!
//! # Usage
//!
//! ```rust
//! use profiling::profile_scope;
//!
//! fn expensive_function() {
//!     profile_scope!("expensive_function");
//!     // Your code here - timing is automatically captured
//! }
//! ```

pub mod events;
pub mod macros;
pub mod profiler;
pub mod scope;
pub mod utilities;
pub mod database;

pub use events::ProfileEvent;
pub use macros::*;
pub use profiler::Profiler;
pub use scope::{init_profiler, ProfileScope};
pub use utilities::*;

#[inline]
fn get_global_profiler() -> &'static Profiler {
    init_profiler()
}

/// Enable profiling globally
pub fn enable_profiling() {
    get_global_profiler().enable();
}

/// Disable profiling globally
pub fn disable_profiling() {
    get_global_profiler().disable();
}

/// Check if profiling is enabled
pub fn is_profiling_enabled() -> bool {
    get_global_profiler().is_enabled()
}

/// Collect all events captured so far (non-destructive)
pub fn collect_events() -> Vec<ProfileEvent> {
    get_global_profiler().collect_events()
}

/// Get all captured events
pub fn get_all_events() -> Vec<ProfileEvent> {
    get_global_profiler().get_all_events()
}

/// Clear all captured events
pub fn clear_events() {
    get_global_profiler().clear();
}

/// Record a frame time (for FPS tracking in profiler UI)
pub fn record_frame_time(frame_time_ms: f32) {
    utilities::record_frame_time(frame_time_ms);
}
