//! The level editor's own menu strip (View / Go / Project).
//!
//! These sit in the level editor's top bar, separate from the app-wide menus in
//! the title bar. Adding a menu here is one entry in [`LevelEditorMenus::render`];
//! nothing in the app shell needs to know about it.

use gpui::*;
use ui::{
    Sizable,
    button::{Button, ButtonVariants as _},
    h_flex,
    popup_menu::PopupMenuExt,
};

use crate::ui::actions::{
    FocusSelected, FrontView, NewScene, OpenScene, OrthographicView, PerspectiveView, SaveScene,
    SaveSceneAs, SideView, ToggleGrid, TogglePerformanceOverlay, ToggleLighting, ToggleViewportOptions,
    ToggleWireframe, TopView,
};

pub struct LevelEditorMenus;

impl LevelEditorMenus {
    pub fn render() -> impl IntoElement {
        h_flex()
            .gap_0p5()
            .items_center()
            .child(
                Button::new("le_menu_view")
                    .label("View")
                    .small()
                    .ghost()
                    .popup_menu(|menu, _, _| {
                        menu.menu("Toggle Grid", Box::new(ToggleGrid))
                            .menu("Toggle Wireframe", Box::new(ToggleWireframe))
                            .menu("Toggle Lighting", Box::new(ToggleLighting))
                            .separator()
                            .menu(
                                "Performance Overlay",
                                Box::new(TogglePerformanceOverlay),
                            )
                            .menu("Viewport Options", Box::new(ToggleViewportOptions))
                            .separator()
                            .menu("Perspective", Box::new(PerspectiveView))
                            .menu("Orthographic", Box::new(OrthographicView))
                            .menu("Top", Box::new(TopView))
                            .menu("Front", Box::new(FrontView))
                            .menu("Side", Box::new(SideView))
                    }),
            )
            .child(
                Button::new("le_menu_go")
                    .label("Go")
                    .small()
                    .ghost()
                    .popup_menu(|menu, _, _| {
                        menu.menu("Focus Selected", Box::new(FocusSelected))
                    }),
            )
            .child(
                Button::new("le_menu_project")
                    .label("Project")
                    .small()
                    .ghost()
                    .popup_menu(|menu, _, _| {
                        menu.menu("New Scene", Box::new(NewScene))
                            .menu("Open Scene", Box::new(OpenScene))
                            .separator()
                            .menu("Save Scene", Box::new(SaveScene))
                            .menu("Save Scene As…", Box::new(SaveSceneAs))
                    }),
            )
    }
}
