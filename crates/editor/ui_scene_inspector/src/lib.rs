//! SceneDB Inspector: what is in a live scene's `pulsar_scenedb::World`.
//!
//! Entities as the object tree, a flat list, by component type or by
//! archetype; each entity's components and those of its attached component
//! instances; and each component's value as typed fields (reflection) or as
//! bytes in a hex and binary viewer, for one entity or for a whole column.
//!
//! It reads only. Scenes come from
//! `engine_backend::services::scene_directory`, and every read runs on a
//! background thread (`handlers::refresh`). See `.agents/SCENE_INSPECTOR.md`.

mod components;
mod handlers;
mod screen;
#[cfg(test)]
mod tests;
mod utils;
pub mod window;

pub use screen::SceneInspectorView;
pub use window::SceneInspectorWindow;

// The tool's status-bar button and Tools menu entry (`ui_common::editor_tools`):
// the editor binary links this crate, the shell names none of it.
inventory::submit! {
    ui_common::editor_tools::EditorToolRegistration {
        origin: pulsar_plugins::engine_origin!(),
        id: "tool.scene_inspector",
        label: || "SceneDB Inspector".to_string(),
        icon: ui::IconName::Database,
        order: 13,
        open: |_, cx| {
            use gpui::UpdateGlobal as _;
            window_manager::WindowRegistry::update_global(cx, |registry, cx| {
                registry.open("SceneInspectorWindow", cx)
            })
        },
        is_open: None,
        init: None,
    }
}
