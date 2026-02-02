//! Panel Window - Displays a single panel in a popup window

use gpui::*;
use std::sync::Arc;
use ui::{
    v_flex, ActiveTheme as _, TitleBar,
};

pub struct PanelWindow {
    panel: Arc<dyn ui::dock::PanelView>,
}

impl PanelWindow {
    pub fn new(
        panel: Arc<dyn ui::dock::PanelView>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self { panel }
    }
}

impl Render for PanelWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(TitleBar::new().child(self.panel.title(window, cx)))
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(self.panel.view())
            )
    }
}
