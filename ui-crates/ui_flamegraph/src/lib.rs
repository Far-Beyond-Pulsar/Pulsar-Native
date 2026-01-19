//! Flamegraph Tracing UI
//!
//! High-performance flamegraph visualization with instanced rendering

mod flamegraph_view;
mod trace_data;
pub mod window;

pub use flamegraph_view::FlamegraphView;
pub use trace_data::{TraceData, TraceSpan, TraceFrame, ThreadInfo};
pub use window::FlamegraphWindow;
