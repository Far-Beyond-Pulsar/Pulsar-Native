use crate::FlamegraphView;
use gpui::*;
use ui::{
    dock::{Panel, PanelEvent},
    v_flex, ActiveTheme,
};

pub struct FlamegraphPanel {
    view: Entity<FlamegraphView>,
    focus_handle: FocusHandle,
}

impl FlamegraphPanel {
    pub fn new(view: Entity<FlamegraphView>, cx: &mut Context<Self>) -> Self {
        Self {
            view,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for FlamegraphPanel {}

ui_common::panel_boilerplate!(FlamegraphPanel);

impl Panel for FlamegraphPanel {
    fn panel_name(&self) -> &'static str {
        "flamegraph_main"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        div().child("Flamegraph").into_any_element()
    }
}

impl Render for FlamegraphPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .id("flamegraph-panel")
            .size_full()
            .bg(theme.background)
            .child(self.view.clone())
    }
}
