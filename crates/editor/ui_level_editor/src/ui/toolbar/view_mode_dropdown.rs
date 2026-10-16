use gpui::*;
use ui::{
    button::{Button, ButtonVariants as _},
    popup_menu::PopupMenuExt,
    IconName, Sizable,
};

use super::actions::SetViewportDebugMode;
use crate::state::LevelEditorState;

/// Selects the lit viewport image or a debug view registered by a Helio pass.
pub struct ViewModeDropdown;

impl ViewModeDropdown {
    pub fn render(
        state: &LevelEditorState,
        debug_views: &[engine_backend::subsystems::render::ViewportDebugView],
    ) -> impl IntoElement {
        let selected_mode = state.editor.viewport_debug_mode;
        let selected_view = debug_views
            .iter()
            .find(|view| view.debug_mode == selected_mode);
        let label = selected_view.map_or("Lit", |view| view.name);
        let tooltip = selected_view.map_or("Show the scene with materials and lighting", |view| {
            view.description
        });
        let debug_views = debug_views.to_vec();

        Button::new("viewport_view_mode")
            .label(label)
            .icon(IconName::ChevronDown)
            .dropdown_caret(true)
            .small()
            .ghost()
            .tooltip(tooltip)
            .popup_menu(move |menu, _, _| {
                let mut menu = menu.label("View Mode").separator().menu_with_check(
                    "Lit",
                    selected_mode == 0,
                    Box::new(SetViewportDebugMode(0)),
                );
                for view in &debug_views {
                    menu = menu.menu_with_check(
                        view.name,
                        selected_mode == view.debug_mode,
                        Box::new(SetViewportDebugMode(view.debug_mode)),
                    );
                }
                menu
            })
    }
}
