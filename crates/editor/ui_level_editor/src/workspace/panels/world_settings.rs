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
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    /// World revision last painted: the Sky section shows a component of the
    /// world, which edits anywhere (properties panel, undo) can change.
    last_revision: u64,
    pump_started: bool,
}

impl WorldSettingsPanel {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let last_revision = state.read().scene.world_revision();
        Self {
            world_settings: WorldSettingsPanelImpl::new(state.clone(), window, cx),
            focus_handle: cx.focus_handle(),
            state,
            last_revision,
            pump_started: false,
        }
    }

    fn start_pump(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pump_started {
            return;
        }
        self.pump_started = true;
        crate::ui::frame_pump::spawn_frame_pump(&cx.entity(), window, |this, _window, cx| {
            let revision = this.state.read().scene.world_revision();
            if revision != this.last_revision {
                this.last_revision = revision;
                cx.notify();
            }
        });
    }
}

impl EventEmitter<PanelEvent> for WorldSettingsPanel {}

ui_common::panel_boilerplate!(WorldSettingsPanel);

impl Render for WorldSettingsPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::render_stats::count("world settings panel: render");
        let _t = gpui::render_stats::scope("world settings panel: render");

        self.start_pump(window, cx);
        self.last_revision = self.state.read().scene.world_revision();

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
