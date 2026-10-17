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
    environment_only: bool,
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
            environment_only: false,
        }
    }

    pub(crate) fn new_environment_config(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut panel = Self::new(state, window, cx);
        panel.environment_only = true;
        panel
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
            .min_h_0()
            .bg(cx.theme().sidebar)
            .child(if self.environment_only {
                self.world_settings.render_environment_config(window, cx)
            } else {
                self.world_settings.render(window, cx).into_any_element()
            })
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
