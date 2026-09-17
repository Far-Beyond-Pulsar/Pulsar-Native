use gpui::*;
use rust_i18n::t;
use std::sync::Arc;
use ui::{
    button::{Button, ButtonVariants as _},
    popup_menu::PopupMenuExt,
    ActiveTheme, Sizable,
};

use super::actions::SetToolMode;
use crate::level_editor::state::LevelEditorState;
use crate::level_editor::tool_modes::ToolModeId;

/// Tool mode dropdown — select between Level Edit, Terrain, etc.
pub struct ToolModeDropdown;

impl ToolModeDropdown {
    pub fn render<V>(
        state: &LevelEditorState,
        _state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        _cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let current_id = state.editor.tool_mode_registry.selected_id();
        let current_mode = state.editor.tool_mode_registry.selected();
        let current_icon = current_mode.icon();
        let current_label = t!(current_mode.label_key());
        let current_tooltip = t!(current_mode.description_key());

        let modes: Vec<(ToolModeId, String)> = state
            .editor
            .tool_mode_registry
            .modes()
            .iter()
            .map(|m| (m.id(), t!(m.label_key()).to_string()))
            .collect();

        Button::new("tool_mode_dropdown")
            .label(current_label)
            .icon(current_icon)
            .dropdown_caret(true)
            .small()
            .ghost()
            .tooltip(current_tooltip)
            .popup_menu(move |menu, _, _| {
                let mut menu = menu.label("Tool Mode").separator();
                for (id, label) in &modes {
                    let is_selected = *id == current_id;
                    menu = menu.menu_with_check(
                        label.clone(),
                        is_selected,
                        Box::new(SetToolMode(*id)),
                    );
                }
                menu
            })
    }
}
