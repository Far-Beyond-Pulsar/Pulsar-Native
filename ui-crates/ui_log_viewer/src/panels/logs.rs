//! Logs panel — streams engine log output in the center dock.

use crate::log_drawer_v2::LogDrawer;
use gpui::*;
use ui::{
    dock::{Panel, PanelEvent},
    v_flex, ActiveTheme,
};

pub struct LogsPanel {
    pub(crate) log_drawer: Entity<LogDrawer>,
    focus_handle: FocusHandle,
}

impl LogsPanel {
    pub fn new(log_drawer: Entity<LogDrawer>, cx: &mut Context<Self>) -> Self {
        Self {
            log_drawer,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for LogsPanel {}

ui_common::panel_boilerplate!(LogsPanel);

impl Render for LogsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(self.log_drawer.clone())
    }
}

impl Panel for LogsPanel {
    fn panel_name(&self) -> &'static str {
        "logs"
    }
    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "Logs".into_any_element()
    }
}
