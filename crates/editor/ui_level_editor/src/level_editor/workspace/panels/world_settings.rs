//! World Settings dock panel.

use crate::level_editor::state::LevelEditorState;
use crate::level_editor::ui::WorldSettingsPanelImpl;
use gpui::*;
use std::collections::HashSet;
use std::sync::Arc;
use ui::{
    dock::{Panel, PanelEvent},
    v_flex, ActiveTheme,
};

/// World Settings Panel (replaced Scene Browser)
pub struct WorldSettingsPanel {
    pub(crate) world_settings: WorldSettingsPanelImpl,
    state: Arc<parking_lot::RwLock<LevelEditorState>>,
    focus_handle: FocusHandle,
    /// Tracks which sections are collapsed (by section name)
    collapsed_sections: HashSet<String>,
}

impl WorldSettingsPanel {
    pub fn new(
        state: Arc<parking_lot::RwLock<LevelEditorState>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Default all sections to collapsed
        let mut collapsed_sections = HashSet::new();
        collapsed_sections.insert("Environment".to_string());
        collapsed_sections.insert("Global Illumination".to_string());
        collapsed_sections.insert("Fog & Atmosphere".to_string());
        collapsed_sections.insert("Physics".to_string());
        collapsed_sections.insert("Audio".to_string());

        Self {
            world_settings: WorldSettingsPanelImpl::new(window, cx),
            state,
            focus_handle: cx.focus_handle(),
            collapsed_sections,
        }
    }

    pub fn toggle_section(&mut self, section: String, cx: &mut Context<Self>) {
        if self.collapsed_sections.contains(&section) {
            self.collapsed_sections.remove(&section);
        } else {
            self.collapsed_sections.insert(section);
        }
        cx.notify();
    }

    pub fn is_section_collapsed(&self, section: &str) -> bool {
        self.collapsed_sections.contains(section)
    }
}

impl EventEmitter<PanelEvent> for WorldSettingsPanel {}

ui_common::panel_boilerplate!(WorldSettingsPanel);

impl Render for WorldSettingsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::render_stats::count("world settings panel: render");
        let _t = gpui::render_stats::scope("world settings panel: render");

        let self_entity_id = cx.entity().entity_id();
        let state = self.state.read();
        let collapsed_sections = self.collapsed_sections.clone();
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                self.world_settings
                    .render(&state, self.state.clone(), &collapsed_sections, cx),
            )
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
