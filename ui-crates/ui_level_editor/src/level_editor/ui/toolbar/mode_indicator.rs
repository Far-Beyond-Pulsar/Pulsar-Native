use gpui::*;
use ui::ActiveTheme;

use super::super::state::LevelEditorState;

/// Mode indicator - Beautiful badge showing Playing/Editing state
pub struct ModeIndicator;

impl ModeIndicator {
    pub fn render<V: 'static>(
        state: &LevelEditorState,
        cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let theme = cx.theme();
        
        div()
            .flex()
            .items_center()
            .gap_1p5()
            .px_3()
            .py_1p5()
            .rounded(px(6.0))
            .bg(if state.is_play_mode() {
                theme.accent.opacity(0.12)
            } else {
                theme.muted.opacity(0.08)
            })
            .border_1()
            .border_color(if state.is_play_mode() {
                theme.accent.opacity(0.25)
            } else {
                theme.border.opacity(0.5)
            })
            .child(
                div()
                    .size(px(7.0))
                    .rounded(px(3.5))
                    .bg(if state.is_play_mode() {
                        gpui::green()
                    } else {
                        theme.muted_foreground.opacity(0.6)
                    })
            )
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(if state.is_play_mode() {
                        theme.accent
                    } else {
                        theme.foreground.opacity(0.8)
                    })
                    .child(if state.is_play_mode() {
                        "Playing"
                    } else {
                        "Editing"
                    })
            )
    }
}
