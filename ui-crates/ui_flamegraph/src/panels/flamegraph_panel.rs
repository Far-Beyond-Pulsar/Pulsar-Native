use gpui::*;
use ui::{
    v_flex,
    ActiveTheme,
    dock::{Panel, PanelEvent},
};
use crate::FlamegraphView;

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

impl Focusable for FlamegraphPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Panel for FlamegraphPanel {
    fn panel_name(&self) -> &'static str {
        "flamegraph_main"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        div()
            .child("Flamegraph")
            .into_any_element()
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
