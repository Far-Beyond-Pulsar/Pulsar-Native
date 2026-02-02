use gpui::*;
use ui::{button::{Button, ButtonVariants as _}, ActiveTheme, IconName, Sizable, popup_menu::PopupMenuExt};
use std::sync::Arc;

use super::super::state::LevelEditorState;
use super::actions::SetTimeScale;

/// Time scale dropdown - Polished, professional simulation speed control
pub struct TimeScaleDropdown;

impl TimeScaleDropdown {
    pub fn render<V: 'static>(
        state: &LevelEditorState,
        _state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let theme = cx.theme();
        let time_scale = state.game_time_scale;
        
        ui::h_flex()
            .gap_2()
            .items_center()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        ui::Icon::new(IconName::Clock)
                            .size_3p5()
                            .text_color(theme.muted_foreground)
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("Speed")
                    )
            )
            .child(
                Button::new("time_scale_dropdown")
                    .label(format!("{:.2}x", time_scale))
                    .icon(IconName::ChevronDown)
                    .small()
                    .ghost()
                    .tooltip("Select simulation speed")
                    .popup_menu(move |menu, _, _| {
                        menu
                            .label("Simulation Speed")
                            .separator()
                            .menu_with_check("0.125x", time_scale == 0.125, Box::new(SetTimeScale(0.125)))
                            .menu_with_check("0.25x", time_scale == 0.25, Box::new(SetTimeScale(0.25)))
                            .menu_with_check("0.5x", time_scale == 0.5, Box::new(SetTimeScale(0.5)))
                            .menu_with_check("1.0x (Normal)", time_scale == 1.0, Box::new(SetTimeScale(1.0)))
                            .menu_with_check("2.0x", time_scale == 2.0, Box::new(SetTimeScale(2.0)))
                            .menu_with_check("4.0x", time_scale == 4.0, Box::new(SetTimeScale(4.0)))
                            .menu_with_check("8.0x", time_scale == 8.0, Box::new(SetTimeScale(8.0)))
                    })
            )
    }
}
