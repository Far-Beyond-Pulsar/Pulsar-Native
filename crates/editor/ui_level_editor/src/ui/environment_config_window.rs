//! A pop-out environment mixer for the active level.

use std::sync::Arc;

use gpui::*;
use ui::{v_flex, ActiveTheme, TitleBar};

use crate::{LevelEditorState, WorldSettingsPanel};

pub struct EnvironmentConfigWindow {
    panel: Entity<WorldSettingsPanel>,
}

impl EnvironmentConfigWindow {
    pub fn open(state: Arc<parking_lot::RwLock<LevelEditorState>>, cx: &mut App) {
        use ui_common::PulsarWindowExt as _;
        Self::open_additional(state, cx);
    }

    fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let panel = cx.new(|cx| WorldSettingsPanel::new_environment_config(state, window, cx));
        Self { panel }
    }
}

impl Render for EnvironmentConfigWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .min_h_0()
            .bg(cx.theme().background)
            .child(
                TitleBar::new().flex_shrink_0().child(
                    div()
                        .px_3()
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM)
                        .child("Environment Config"),
                ),
            )
            .child(div().flex_1().min_h_0().child(self.panel.clone()))
    }
}

impl window_manager::PulsarWindow for EnvironmentConfigWindow {
    type Params = Arc<parking_lot::RwLock<LevelEditorState>>;

    fn window_name() -> &'static str {
        "EnvironmentConfigWindow"
    }

    fn window_options(_: &Self::Params) -> WindowOptions {
        window_manager::default_window_options(760.0, 900.0)
    }

    fn build(state: Self::Params, window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| Self::new(state, window, cx))
    }
}
