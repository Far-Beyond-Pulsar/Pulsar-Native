#![allow(warnings)]

// Engine core types (used by UI components)
pub mod assets;

// Use the new blueprint_compiler instead of the old embedded compiler
pub use blueprint_compiler as compiler;

pub mod diagnostics;
pub mod graph;
pub mod settings;
pub mod themes;

mod event;
mod global_state;
mod icon;
mod index_path;
pub mod utils;
#[cfg(any(feature = "inspector", debug_assertions))]
mod inspector;
mod kbd;
pub mod menu;
mod root;
mod styled;
mod time;
mod title_bar;
pub mod bevy_viewport; // Production-ready zero-copy Bevy viewport using GPUI's gpu_canvas
pub mod gpu_viewport; // Compatibility alias for bevy_viewport
pub mod gpu_mem_tracker; // GPU memory allocation tracker for debugging VRAM leaks
pub mod dx11_shared_opener; // DX11 opener for DX12 shared handles (zero-copy bridge)
mod virtual_list;
mod window_border;
pub mod component; // Component-based UI architecture

pub(crate) mod actions;

pub mod accordion;
pub mod alert;
pub mod animation;
pub mod avatar;
pub mod badge;
pub mod breadcrumb;
pub mod button;
pub mod chart;
pub mod checkbox;
pub mod clipboard;
pub mod code_editor; // Studio-quality virtualized code editor
pub mod color_picker;
pub mod replication; // Multi-user editing and state replication
pub mod description_list;
pub mod divider;
pub mod dock;
pub mod draggable_tabs;
pub mod drawer;
pub mod dropdown;
pub mod form;
pub mod group_box;
pub mod hierarchical_tree;
pub mod highlighter;
pub mod history;
pub mod indicator;
pub mod input;
pub mod json_ui;
pub mod label;
pub mod link;
pub mod list;
pub mod minimap;
pub mod modal;
pub mod notification;
pub mod plot;
pub mod popover;
pub mod progress;
pub mod radio;
pub mod resizable;
pub mod scroll;
pub mod sidebar;
pub mod skeleton;
pub mod slider;
pub mod spinner;
pub mod switch;
pub mod tab;
pub mod table;
pub mod tag;
pub mod text;
pub mod theme;
pub mod tooltip;
#[cfg(feature = "webview")]
pub mod webview;
pub mod workspace;

use gpui::{ App, SharedString };
// re-export
#[cfg(feature = "webview")]
pub use wry;

pub use crate::Disableable;
pub use event::InteractiveElementExt;
pub use index_path::IndexPath;
#[cfg(any(feature = "inspector", debug_assertions))]
pub use inspector::*;
pub use menu::{ context_menu, popup_menu };
pub use root::{ ContextModal, Root };
pub use styled::*;
pub use time::*;
pub use title_bar::*;
pub use virtual_list::{ h_virtual_list, v_virtual_list, VirtualList, VirtualListScrollHandle };
pub use window_border::{ window_border, window_paddings, WindowBorder };

pub use icon::*;
pub use kbd::*;
pub use theme::*;
pub use component::*;
pub use hierarchical_tree::{
    render_tree_folder,
    render_tree_category,
    render_tree_item,
    tree_colors,
};

// Re-export engine types for UI crates
pub use assets::Assets;
pub use graph::*;
pub use compiler::*;
pub use settings::*;
pub use themes::*;

// Engine constants (will be set by engine binary)
pub const ENGINE_NAME: &str = "Pulsar Engine";
pub const ENGINE_VERSION: &str = "0.1.45";
pub const ENGINE_LICENSE: &str = "MIT/Apache-2.0";
pub const ENGINE_AUTHORS: &str = "Pulsar Contributors";

// Common actions
gpui::actions!(ui, [OpenSettings]);

use std::ops::Deref;

rust_i18n::i18n!("locales", fallback = "en");

// Re-export translation function for use by other ui crates
/// Translate a key to the current locale
#[inline]
pub fn translate(key: &str) -> String {
    rust_i18n::t!(key).to_string()
}

/// Initialize the components.
///
/// You must initialize the components at your application's entry point.
pub fn init(cx: &mut App) {
    theme::init(cx);
    global_state::init(cx);
    replication::init(cx);
    #[cfg(any(feature = "inspector", debug_assertions))]
    inspector::init(cx);
    root::init(cx);
    date_picker::init(cx);
    color_picker::init(cx);
    dock::init(cx);
    drawer::init(cx);
    dropdown::init(cx);
    input::init(cx);
    list::init(cx);
    modal::init(cx);
    popover::init(cx);
    menu::init(cx);
    table::init(cx);
    text::init(cx);
}

#[inline]
pub fn locale() -> impl Deref<Target = str> {
    rust_i18n::locale()
}

#[inline]
pub fn set_locale(locale: &str) {
    rust_i18n::set_locale(locale)
}

#[inline]
pub(crate) fn measure_enable() -> bool {
    std::env::var("ZED_MEASUREMENTS").is_ok() || std::env::var("GPUI_MEASUREMENTS").is_ok()
}

/// Measures the execution time of a function and logs it if `if_` is true.
///
/// And need env `GPUI_MEASUREMENTS=1`
#[inline]
#[track_caller]
pub fn measure_if(name: impl Into<SharedString>, if_: bool, f: impl FnOnce()) {
    if if_ && measure_enable() {
        let measure = Measure::new(name);
        f();
        measure.end();
    } else {
        f();
    }
}

/// Measures the execution time.
#[inline]
#[track_caller]
pub fn measure(name: impl Into<SharedString>, f: impl FnOnce()) {
    measure_if(name, true, f);
}

pub struct Measure {
    name: SharedString,
    start: std::time::Instant,
}

impl Measure {
    #[track_caller]
    pub fn new(name: impl Into<SharedString>) -> Self {
        Self {
            name: name.into(),
            start: std::time::Instant::now(),
        }
    }

    #[track_caller]
    pub fn end(self) {
        let duration = self.start.elapsed();
        tracing::trace!("{} in {:?}", self.name, duration);
    }
}
