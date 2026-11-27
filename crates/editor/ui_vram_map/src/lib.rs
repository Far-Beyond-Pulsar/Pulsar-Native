//! VRAM Map: where the engine's video memory is, as a pannable, zoomable
//! treemap (`engine_backend::services::vram` is the tracker it draws).
//!
//! Each box is an allocation (or a group of them): its area is the bytes the
//! driver gave it; its darker part, from the bottom up, is the share its owner
//! reports as live, when the owner reports one. "By owner" groups the boxes by
//! who claims them; "By block" lays out each driver memory block in address
//! order, free gaps included.
//!
//! Only video memory: process memory is the Memory Explorer's (Mission
//! Control).

mod components;
mod utils;
pub mod window;

pub use components::VramMapView;
pub use window::VramMapWindow;

// The tool's status-bar button and Tools menu entry (`ui_common::editor_tools`):
// the editor binary links this crate, the shell names none of it.
inventory::submit! {
    ui_common::editor_tools::EditorToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        id: "tool.vram_map",
        label: || "VRAM Map".to_string(),
        icon: ui::IconName::Cpu,
        order: 12,
        open: |_, cx| {
            use gpui::UpdateGlobal as _;
            window_manager::WindowRegistry::update_global(cx, |registry, cx| {
                registry.open("VramMapWindow", cx)
            })
        },
        is_open: None,
        init: None,
    }
}
