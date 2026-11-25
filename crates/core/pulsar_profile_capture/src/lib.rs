//! Profiling captures as data.
//!
//! The profiler ([`profiling`]) records scope events in every instrumented
//! process and streams them to a viewer. This crate is what a viewer that is
//! an AI agent needs on top of that: a bounded [`CaptureBuffer`] that keeps the
//! newest events of a long recording, a [`Capture`] that is the finished
//! recording, and analyses over it that answer with JSON instead of pixels:
//!
//! - [`analysis::top_scopes`]: self and total time per scope, call counts,
//!   percentiles, by scope or by thread and scope;
//! - [`analysis::frame_breakdown`]: the scope tree of one frame, what the other
//!   threads did during it, and the critical path;
//! - [`analysis::frame_times`]: the series, histogram and hitches of the frame
//!   period, with what ran in each hitch;
//! - [`analysis::threads`]: busy and idle time per thread and the time spent
//!   waiting (locks, fences, present, sleeps);
//! - [`analysis::gpu`]: GPU pass timings the renderer published;
//! - [`analysis::latency`]: the input-to-present chain, stage by stage;
//! - [`analysis::gaps`]: scopes whose own time is large, that is, where a
//!   missing inner scope hides the cost;
//! - [`export`]: the raw capture and a Chrome trace.
//!
//! Nothing here reads a process or draws anything; the crate that owns the
//! viewer connection (`ui_flamegraph`) feeds it events and registers the MCP
//! tools.

pub mod analysis;
pub mod buffer;
pub mod export;
pub mod frames;
pub mod index;
pub mod latency;
pub mod stats;

pub use buffer::{Capture, CaptureBuffer, Target};
pub use frames::{Frame, FrameSpec};
pub use index::Index;

#[cfg(test)]
mod test_support;
