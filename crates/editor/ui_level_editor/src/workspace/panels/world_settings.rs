//! World Settings dock panel.

use crate::state::LevelEditorState;
use crate::ui::WorldSettingsPanelImpl;
use gpui::*;
use std::sync::Arc;
use ui::{
    dock::{Panel, PanelEvent},
    v_flex, ActiveTheme,
};

/// World Settings Panel (replaced Scene Browser)
pub struct WorldSettingsPanel {
    pub(crate) world_settings: WorldSettingsPanelImpl,
    focus_handle: FocusHandle,
}

impl WorldSettingsPanel {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            world_settings: WorldSettingsPanelImpl::new(state.clone(), window, cx),
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<PanelEvent> for WorldSettingsPanel {}

ui_common::panel_boilerplate!(WorldSettingsPanel);

impl Render for WorldSettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::render_stats::count("world settings panel: render");
        let _t = gpui::render_stats::scope("world settings panel: render");

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(self.world_settings.render(window, cx))
    }
}

impl Panel for WorldSettingsPanel {
    fn panel_name(&self) -> &'static str {
        "world_settings"
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        "World".into_any_element()
    }
}
