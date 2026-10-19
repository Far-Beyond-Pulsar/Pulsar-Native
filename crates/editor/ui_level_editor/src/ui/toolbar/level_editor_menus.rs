//! The level editor's own menu strip (Edit / Create / Tools / View / Go /
//! Project).
//!
//! These sit in the level editor's top bar, separate from the app-wide menus in
//! the title bar (Pulsar-Native#1012). Adding a menu here is one more
//! [`EditorMenu`] in [`LevelEditorMenus::render`]; nothing in the app shell
//! needs to know about it.

use gpui::*;
use plugin_editor_api::{EditorMenu, EditorMenuStrip};
use rust_i18n::t;

use crate::ui::actions::{
    AddObjectOfType, DeleteObject, DuplicateObject, FocusSelected, NewScene, OpenEnvironmentConfig,
    OpenScene, SaveScene, SaveSceneAs, ToggleCameraModeSelector, ToggleGrid, ToggleViewportOptions,
};
use crate::LevelEditorState;

pub struct LevelEditorMenus;

impl LevelEditorMenus {
    pub fn render(state: &LevelEditorState) -> impl IntoElement {
        let selected = state.scene.selected_object().is_some();
        EditorMenuStrip::new("level-editor-menus")
            .menu(
                EditorMenu::new(t!("LevelEditor.Menu.Edit"))
                    .undo_redo(
                        t!("LevelEditor.Menu.Undo"),
                        t!("LevelEditor.Menu.Redo"),
                        Some(state.scene.history().clone() as _),
                    )
                    .separator()
                    .action_if(t!("LevelEditor.Menu.Duplicate"), DuplicateObject, selected)
                    .action_if(t!("LevelEditor.Menu.Delete"), DeleteObject, selected),
            )
            .menu(Self::create_menu())
            .menu(EditorMenu::new(t!("LevelEditor.Menu.Tools")).action(
                t!("LevelEditor.Menu.EnvironmentConfig"),
                OpenEnvironmentConfig,
            ))
            .menu(
                EditorMenu::new(t!("LevelEditor.Menu.View"))
                    .action(t!("LevelEditor.Menu.ToggleGrid"), ToggleGrid)
                    .separator()
                    .action("Viewport controls", ToggleViewportOptions)
                    .action("Camera controls", ToggleCameraModeSelector),
            )
            .menu(EditorMenu::new(t!("LevelEditor.Menu.Go")).action_if(
                t!("LevelEditor.Menu.FocusSelected"),
                FocusSelected,
                selected,
            ))
            .menu(
                EditorMenu::new(t!("LevelEditor.Menu.Project"))
                    .action(t!("LevelEditor.Menu.NewScene"), NewScene)
                    .action(t!("LevelEditor.Menu.OpenScene"), OpenScene)
                    .separator()
                    .action(t!("LevelEditor.Menu.SaveScene"), SaveScene)
                    .action(t!("LevelEditor.Menu.SaveSceneAs"), SaveSceneAs),
            )
    }

    /// Create: what used to be the global Game Engine menu's objects and
    /// lights, each added at the origin as one undo step and selected.
    fn create_menu() -> EditorMenu {
        let mut menu = EditorMenu::new(t!("LevelEditor.Menu.Create"));
        let mut last_group = None;
        for &(kind, label) in crate::scene_edit::create::KINDS {
            // Separators between objects, lights, meshes and environment.
            let this_group = match kind.split('_').next() {
                Some("light") => "light",
                Some("mesh") => "mesh",
                Some("sky" | "wind") => "environment",
                _ => "object",
            };
            if last_group.is_some_and(|last| last != this_group) {
                menu = menu.separator();
            }
            last_group = Some(this_group);
            menu = menu.action(
                t!(label),
                AddObjectOfType {
                    object_type: kind.to_string(),
                },
            );
        }
        menu
    }
}
