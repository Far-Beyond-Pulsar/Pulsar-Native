/// Clip Editor Component
/// Detailed clip editing view

use super::state::*;
use super::panel::DawPanel;
use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{v_flex, StyledExt, ActiveTheme};

pub fn render_clip_editor(state: &DawUiState, cx: &mut Context<DawPanel>) -> impl IntoElement {
    v_flex()
        .size_full()
        .items_center()
        .justify_center()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Clip Editor View")
        )
}
