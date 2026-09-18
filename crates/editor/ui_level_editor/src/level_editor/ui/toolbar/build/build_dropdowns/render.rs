use super::*;

impl BuildDropdowns {
    pub fn render<V>(
        state: &LevelEditorState,
        _state_arc: Arc<parking_lot::RwLock<LevelEditorState>>,
        _cx: &mut Context<V>,
    ) -> impl IntoElement
    where
        V: 'static + EventEmitter<ui::dock::PanelEvent> + Render,
    {
        let config_label = match state.build.config {
            BuildConfig::Debug => "Debug",
            BuildConfig::Release => "Release",
            BuildConfig::Shipping => "Ship",
        };

        let platform_label = Self::get_platform_label(state.build.target_platform);
        let platform_icon = Self::get_platform_icon(state.build.target_platform);

        let current_platform = state.build.target_platform;

        ui::h_flex()
            .gap_1p5()
            .items_center()
            .child(Self::config_button(config_label))
            .child(Self::platform_button(
                platform_label,
                platform_icon,
                current_platform,
            ))
    }
}
