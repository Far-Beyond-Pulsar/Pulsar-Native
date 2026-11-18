pub mod auth;
pub mod image_loader;
pub mod parser;
mod search_index;

pub mod components;
mod handlers;
mod screen;
mod utils;

use gpui::UpdateGlobal as _;
use ui::IconName;
use ui_shell::editor_tools::EditorToolRegistration;
use window_manager::WindowRegistry;

inventory::submit! {
    EditorToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        id: "tool.sketchfab_viewer",
        label: || "Sketchfab Viewer".to_string(),
        icon: IconName::Globe,
        order: 40,
        open: |_, cx| {
            WindowRegistry::update_global(cx, |registry, cx| registry.open("FabSearchWindow", cx))
        },
        is_open: None,
        init: None,
    }
}

pub(crate) use screen::FabSearchWindow;
pub(crate) use utils::actions::DownloadState;
