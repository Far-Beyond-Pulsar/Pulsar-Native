//! Flamegraph Tracing UI
//!
//! High-performance flamegraph visualization with instrumentation-based profiling

// Initialize translations
rust_i18n::i18n!("locales", fallback = "en");

mod flamegraph_view;
mod panels;
mod trace_data;
pub mod window;

// Core modules
mod colors;
mod components;
mod constants;
mod coordinates;
mod lod_tree;
mod state;

// Profiling module
mod profiler;

pub use flamegraph_view::FlamegraphView;
pub use panels::{FlamegraphPanel, StatisticsPanel};
pub use profiler::{convert_profile_events_to_trace, InstrumentationCollector};
pub use trace_data::{ThreadInfo, TraceData, TraceFrame, TraceSpan};
pub use window::FlamegraphWindow;

/// Get current locale
pub fn locale() -> String {
    rust_i18n::locale().to_string()
}

/// Set locale
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}
