use crate::settings_v2::{SettingsScreenV2, SettingsScreenV2Props};
use gpui::*;
use ui::{
    v_flex, ActiveTheme, TitleBar,
};

pub struct SettingsWindow {
    settings_screen: Option<Entity<SettingsScreenV2>>,
    window_id: Option<engine_state::WindowId>,
}

impl SettingsWindow {
    pub fn new(cx: &mut Context<Self>) -> Self {
        // Initialize default settings in the registry if not already done
        engine_state::register_default_settings();

        // Get the current project path from the engine context
        let project_path = engine_state::EngineContext::global()
            .and_then(|ctx| {
                ctx.project.read()
                    .as_ref()
                    .map(|project| project.path.clone())
            });

        let settings_screen = cx.new(|cx| SettingsScreenV2::new(
            SettingsScreenV2Props {
                project_path,
            },
            // window_manager will provide the window
            cx
        ));

        Self {
            settings_screen: Some(settings_screen),
            window_id: None,
        }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(TitleBar::new())
            .child(
                if let Some(screen) = &self.settings_screen {
                    screen.clone().into_any_element()
                } else {
                    div().into_any_element()
                }
            )
    }
}
