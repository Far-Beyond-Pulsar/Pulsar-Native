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
pub use profiler::{init_profiler, Profiler};
pub use scope::ProfileScope;
pub use utilities::*;

fn get_global_profiler() -> &'static Profiler {
    static PROFILER: once_cell::sync::Lazy<Profiler> = once_cell::sync::Lazy::new(Profiler::new);
    &PROFILER
}
