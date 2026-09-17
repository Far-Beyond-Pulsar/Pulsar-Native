use gpui::*;
use rust_i18n::t;
use ui::{ActiveTheme, Sizable};

use crate::level_editor::state::LevelEditorState;

/// Mode indicator - Beautiful badge showing Playing/Editing state and current Tool Mode
pub struct ModeIndicator;

impl ModeIndicator {
    pub fn render<V>(state: &LevelEditorState, cx: &mut Context<V>) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let theme = cx.theme();
        let current_mode = state.editor.tool_mode_registry.selected();
        let tool_icon = current_mode.icon();
        let tool_label = t!(current_mode.label_key());

        div()
            .flex()
            .items_center()
            .gap_1p5()
            .px_3()
            .py_1p5()
            .rounded(px(6.0))
            .bg(if state.scene.is_play_mode() {
                theme.accent.opacity(0.12)
            } else {
                theme.muted.opacity(0.08)
            })
            .border_1()
            .border_color(if state.scene.is_play_mode() {
                theme.accent.opacity(0.25)
            } else {
                theme.border.opacity(0.5)
            })
            .child(
                div()
                    .size(px(7.0))
                    .rounded(px(3.5))
                    .bg(if state.scene.is_play_mode() {
                        gpui::green()
                    } else {
                        theme.muted_foreground.opacity(0.6)
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if state.scene.is_play_mode() {
                        theme.accent
                    } else {
                        theme.foreground.opacity(0.8)
                    })
                    .child(if state.scene.is_play_mode() {
                        "Playing"
                    } else {
                        "Editing"
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground.opacity(0.4))
                    .child("·"),
            )
            .child(
                ui::Icon::new(tool_icon)
                    .size_3p5()
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.foreground.opacity(0.8))
                    .child(tool_label.into_owned()),
            )
    }
}
