use crate::screen::ModernSettingsScreen;
use gpui::{prelude::FluentBuilder as _, *};
use std::path::PathBuf;
use ui::{v_flex, ActiveTheme, TitleBar};

/// Parameters for opening a [`SettingsWindow`].
///
/// - `project_path: Some(path)` scopes the window to that project's
///   `.pulsar` TOML settings database (the "Project" page edits it).
/// - `None` falls back to the globally-loaded project from `EngineContext`
///   (matching the historical zero-param behaviour); if no project is loaded
///   globally either, only global editor settings are editable.
#[derive(Debug, Clone, Default)]
pub struct SettingsParams {
    pub project_path: Option<PathBuf>,
}

impl SettingsParams {
    pub fn scoped(project_path: impl Into<PathBuf>) -> Self {
        Self {
            project_path: Some(project_path.into()),
        }
    }
}

pub struct SettingsWindow {
    settings_screen: Option<Entity<ModernSettingsScreen>>,
    window_id: Option<engine_state::WindowId>,
}

impl SettingsWindow {
    /// Resolve which project the window is scoped to: explicit params win,
    /// then the global EngineContext project.
    fn resolve_project_path(params: &SettingsParams) -> Option<PathBuf> {
        if params.project_path.is_some() {
            return params.project_path.clone();
        }
        engine_state::EngineContext::global().and_then(|ctx| {
            ctx.store
                .get_or_init::<Option<engine_state::ProjectContext>>()
                .read()
                .as_ref()
                .map(|project| project.path.clone())
        })
    }

    pub fn new(params: SettingsParams, window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Initialize default settings in the registry if not already done
        engine_state::register_default_settings();

        let project_path = Self::resolve_project_path(&params);
        let settings_screen =
            cx.new(|cx| ModernSettingsScreen::new(project_path.clone(), window, cx));

        Self {
            settings_screen: Some(settings_screen),
            window_id: None,
        }
    }

    /// Register the `"SettingsWindow"` opener in the global
    /// [`window_manager::WindowRegistry`] so menu entries using
    /// `reg.open("SettingsWindow", cx)` keep working.
    ///
    /// Call once during app startup (parameterised windows are not
    /// auto-registered by `#[register_window]`).
    pub fn init(cx: &mut App) {
        use ui_common::PulsarWindowExt;
        <Self as PulsarWindowExt>::register(cx);
    }

    /// Open a settings window, optionally pointed at a specific project.
    ///
    /// Focuses the existing settings window if one is already open.
    pub fn open_scoped(project_path: Option<PathBuf>, cx: &mut App) {
        use ui_common::PulsarWindowExt;
        Self::open(SettingsParams { project_path }, cx);
    }

    fn title(&self, cx: &App) -> String {
        let project = self
            .settings_screen
            .as_ref()
            .and_then(|screen| screen.read(cx).project_path().map(std::path::Path::to_path_buf));
        match project.and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string())) {
            Some(name) => format!("Project Settings \u{b7} {}", name),
            None => "Settings".to_string(),
        }
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = self.title(cx);
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
                        .child(title),
                ),
            )
            .when_some(
                self.settings_screen.as_ref(),
                |this: gpui::Div, screen: &Entity<ModernSettingsScreen>| this.child(screen.clone()),
            )
    }
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for SettingsWindow {
    type Params = SettingsParams;

    fn window_name() -> &'static str {
        "SettingsWindow"
    }

    fn window_options(_: &SettingsParams) -> gpui::WindowOptions {
        window_manager::default_window_options(1200.0, 700.0) // Wider for sidebar layout
    }

    fn build(params: SettingsParams, window: &mut gpui::Window, cx: &mut gpui::App) -> gpui::Entity<Self> {
        cx.new(|cx| SettingsWindow::new(params, window, cx))
    }
}
