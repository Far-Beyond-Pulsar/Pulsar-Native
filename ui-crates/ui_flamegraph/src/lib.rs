//! Flamegraph Tracing UI
//!
//! High-performance flamegraph visualization with instanced rendering

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

pub use flamegraph_view::FlamegraphView;
pub use trace_data::{TraceData, TraceSpan, TraceFrame, ThreadInfo};
pub use window::FlamegraphWindow;

// Re-export dtrace_profiler
pub use dtrace_profiler::DTraceProfiler as BackgroundProfiler;
