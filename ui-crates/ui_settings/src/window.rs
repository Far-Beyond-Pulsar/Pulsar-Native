use crate::settings_v2::{SettingsScreenV2, SettingsScreenV2Props};
use gpui::*;
use ui::{
    v_flex, ActiveTheme, TitleBar,
};

pub struct SettingsWindow {
    settings_screen: Option<Entity<SettingsScreenV2>>,
}

impl SettingsWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Initialize default settings in the registry if not already done
        engine_state::register_default_settings();

        // For now, we don't pass a project path - this could be enhanced to accept one
        let project_path = None;

        let settings_screen = cx.new(|cx| SettingsScreenV2::new(
            SettingsScreenV2Props {
                project_path,
            },
            window,
            cx
        ));

        Self {
            settings_screen: Some(settings_screen),
        }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
