//! Flamegraph Tracing UI
//!
//! High-performance flamegraph visualization with instrumentation-based profiling

mod flamegraph_view;
mod trace_data;
pub mod window;

// Core modules
mod constants;
mod colors;
mod state;
mod coordinates;
mod components;
mod lod_tree;

// Profiling module
mod profiler;

pub use flamegraph_view::FlamegraphView;
pub use trace_data::{TraceData, TraceSpan, TraceFrame, ThreadInfo};
pub use window::FlamegraphWindow;
pub use profiler::{InstrumentationCollector, convert_profile_events_to_trace};

