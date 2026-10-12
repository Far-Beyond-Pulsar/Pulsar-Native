//! The level editor's own menu strip (Edit / View / Go / Project).
//!
//! These sit in the level editor's top bar, separate from the app-wide menus in
//! the title bar (Pulsar-Native#1012). Adding a menu here is one more
//! [`EditorMenu`] in [`LevelEditorMenus::render`]; nothing in the app shell
//! needs to know about it.

use gpui::*;
use plugin_editor_api::{EditorMenu, EditorMenuStrip};
use rust_i18n::t;

use crate::ui::actions::{
    DeleteObject, DuplicateObject, FocusSelected, FrontView, NewScene, OpenScene, OrthographicView,
    PerspectiveView, Redo, SaveScene, SaveSceneAs, SideView, ToggleGrid, ToggleLighting,
    TogglePerformanceOverlay, ToggleViewportOptions, ToggleWireframe, TopView, Undo,
};
use crate::LevelEditorState;

pub struct LevelEditorMenus;

impl LevelEditorMenus {
    pub fn render(state: &LevelEditorState) -> impl IntoElement {
        let selected = state.scene.selected_object().is_some();
        EditorMenuStrip::new("level-editor-menus")
            .menu(
                EditorMenu::new(t!("LevelEditor.Menu.Edit"))
                    .action_if(t!("LevelEditor.Menu.Undo"), Undo, state.scene.can_undo())
                    .action_if(t!("LevelEditor.Menu.Redo"), Redo, state.scene.can_redo())
                    .separator()
                    .action_if(t!("LevelEditor.Menu.Duplicate"), DuplicateObject, selected)
                    .action_if(t!("LevelEditor.Menu.Delete"), DeleteObject, selected),
            )
            .menu(
                EditorMenu::new(t!("LevelEditor.Menu.View"))
                    .action(t!("LevelEditor.Menu.ToggleGrid"), ToggleGrid)
                    .action(t!("LevelEditor.Menu.ToggleWireframe"), ToggleWireframe)
                    .action(t!("LevelEditor.Menu.ToggleLighting"), ToggleLighting)
                    .separator()
                    .action(
                        t!("LevelEditor.Menu.PerformanceOverlay"),
                        TogglePerformanceOverlay,
                    )
                    .action(
                        t!("LevelEditor.Menu.ViewportOptions"),
                        ToggleViewportOptions,
                    )
                    .separator()
                    .action(t!("LevelEditor.Menu.Perspective"), PerspectiveView)
                    .action(t!("LevelEditor.Menu.Orthographic"), OrthographicView)
                    .action(t!("LevelEditor.Menu.Top"), TopView)
                    .action(t!("LevelEditor.Menu.Front"), FrontView)
                    .action(t!("LevelEditor.Menu.Side"), SideView),
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
}
