//! Flamegraph Tracing UI
//!
//! High-performance flamegraph visualization with instrumentation-based profiling

// Initialize translations
rust_i18n::i18n!("locales", fallback = "en");

mod flamegraph_view;
mod trace_data;
pub mod window;
mod panels;

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
pub use panels::{StatisticsPanel, FlamegraphPanel};

/// Get current locale
pub fn locale() -> String {
    rust_i18n::locale().to_string()
}

/// Set locale
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale);
}

