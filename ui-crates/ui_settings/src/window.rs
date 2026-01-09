use crate::settings::{SettingsScreen, SettingsScreenProps};
use gpui::*;
use ui::{
    v_flex, ActiveTheme, TitleBar,
};

pub struct SettingsWindow {
    settings_screen: Option<Entity<SettingsScreen>>,
}

impl SettingsWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let proj_dirs = directories::ProjectDirs::from("com", "Pulsar", "Pulsar_Engine")
            .expect("Could not determine app data directory");
        let appdata_dir = proj_dirs.data_dir();
        let config_dir = appdata_dir.join("configs");
        let config_file = config_dir.join("engine.toml");

        let settings_screen = cx.new(|cx| SettingsScreen::new(
            SettingsScreenProps {
                config_path: config_file,
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
