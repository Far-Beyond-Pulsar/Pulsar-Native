//! Flamegraph Tracing UI
//!
//! High-performance flamegraph visualization with instrumentation-based profiling

// Initialize translations
rust_i18n::i18n!("locales", fallback = "en");

mod analysis;
mod export;
mod flamegraph_view;
mod format;
mod panels;
mod sample_data;
mod trace_data;
pub mod window;

// Core modules
mod colors;
mod components;
mod constants;
mod coordinates;
mod lod_tree;
mod rendering;
mod state;

// Profiling module
mod profiler;

pub use flamegraph_view::FlamegraphView;
pub use panels::{CallstackPanel, FlamegraphPanel, ScriptsPanel, StatisticsPanel};
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

// The flamegraph's status-bar button and Tools menu entry
// (`ui_common::editor_tools`): the editor binary links this crate, the shell
// names none of it.
inventory::submit! {
    ui_common::editor_tools::EditorToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        id: "tool.flamegraph",
        label: || rust_i18n::t!("Flamegraph.Title").to_string(),
        icon: ui::IconName::Activity,
        order: 10,
        open: |_, cx| {
            use gpui::UpdateGlobal as _;
            window_manager::WindowRegistry::update_global(cx, |registry, cx| {
                registry.open("FlamegraphWindow", cx)
            })
        },
        is_open: None,
        init: None,
    }
}
