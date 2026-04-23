use crate::settings_modern::ModernSettingsScreen;
use gpui::{prelude::FluentBuilder as _, *};
use ui::{v_flex, ActiveTheme, TitleBar};

pub struct SettingsWindow {
    settings_screen: Option<Entity<ModernSettingsScreen>>,
    window_id: Option<engine_state::WindowId>,
}

impl SettingsWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Initialize default settings in the registry if not already done
        engine_state::register_default_settings();

        // Get the current project path from the engine context
        let project_path = engine_state::EngineContext::global().and_then(|ctx| {
            ctx.project
                .read()
                .as_ref()
                .map(|project| project.path.clone())
        });

        let settings_screen = cx.new(|cx| ModernSettingsScreen::new(project_path, window, cx));

        Self {
            settings_screen: Some(settings_screen),
            window_id: None,
        }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = cx.theme();
        v_flex()
            .size_full()
            .child(
                TitleBar::new().child(
                    div()
                        .flex()
                        .items_center()
                        .px_2()
                        .text_sm()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .child("Settings"),
                ),
            )
            .when_some(
                self.settings_screen.as_ref(),
                |this: gpui::Div, screen: &Entity<ModernSettingsScreen>| this.child(screen.clone()),
            )
    }
}

impl window_manager::PulsarWindow for SettingsWindow {
    type Params = ();

    fn window_name() -> &'static str {
        "SettingsWindow"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(1000.0, 700.0) // Wider for sidebar layout
    }

    fn build(_: (), window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::Entity<Self> {
        cx.new(|cx| SettingsWindow::new(window, cx))
    }
}
