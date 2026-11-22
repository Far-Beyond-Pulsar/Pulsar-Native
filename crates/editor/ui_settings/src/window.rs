//! The two settings windows: Project Settings and Editor Settings. Each
//! wraps a [`SettingsScreen`] of its kind under a title bar.

use std::path::PathBuf;

use gpui::{
    div, App, AppContext as _, Context, Entity, Global, IntoElement, ParentElement as _, Render,
    Styled, WeakEntity, Window,
};
use ui::{v_flex, TitleBar};

use crate::components::SettingsScreen;
use crate::utils::model::WindowKind;

/// Parameters for opening a [`ProjectSettingsWindow`].
///
/// - `project_path: Some(path)` edits that project's settings.
/// - `None` falls back to the project loaded in `EngineContext`.
#[derive(Debug, Clone, Default)]
pub struct SettingsParams {
    pub project_path: Option<PathBuf>,
    /// The page to open on, by title (a plugin's name); the first page when
    /// `None` or not found.
    pub page: Option<String>,
}

impl SettingsParams {
    pub fn scoped(project_path: impl Into<PathBuf>) -> Self {
        Self {
            project_path: Some(project_path.into()),
            page: None,
        }
    }
}

/// The open Project Settings screen, so a request for a page reaches a
/// window that is already open.
#[derive(Default)]
struct OpenProjectScreen(Option<WeakEntity<SettingsScreen>>);

impl Global for OpenProjectScreen {}

fn title_bar(title: String) -> impl IntoElement {
    TitleBar::new().child(
        div()
            .flex()
            .items_center()
            .px_2()
            .text_sm()
            .font_weight(gpui::FontWeight::MEDIUM)
            .child(title),
    )
}

// ── Project Settings ─────────────────────────────────────────────────────────

pub struct ProjectSettingsWindow {
    screen: Entity<SettingsScreen>,
}

impl ProjectSettingsWindow {
    /// The project the window edits: the params' or the loaded one.
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
        engine_state::register_default_settings();
        let project_path = Self::resolve_project_path(&params);
        let screen = cx.new(|cx| {
            SettingsScreen::new(WindowKind::Project, project_path, params.page.clone(), window, cx)
        });
        cx.set_global(OpenProjectScreen(Some(screen.downgrade())));
        Self { screen }
    }

    /// Register the `"ProjectSettingsWindow"` opener in the
    /// [`window_manager::WindowRegistry`] (parameterised windows are not
    /// registered by `#[register_window]`). Call once at startup.
    pub fn init(cx: &mut App) {
        use ui_shell::PulsarWindowExt;
        <Self as PulsarWindowExt>::register(cx);
        EditorSettingsWindow::init(cx);
    }

    /// Open the window for `project_path` (focusing it when it is open).
    pub fn open_scoped(project_path: Option<PathBuf>, cx: &mut App) {
        use ui_shell::PulsarWindowExt;
        Self::open(SettingsParams { project_path, page: None }, cx);
    }

    /// Open the window on the page titled `page` (a plugin's name). An open
    /// window is focused and turns to that page.
    pub fn open_page(project_path: Option<PathBuf>, page: String, cx: &mut App) {
        use ui_shell::PulsarWindowExt;
        let open = cx
            .try_global::<OpenProjectScreen>()
            .and_then(|open| open.0.as_ref())
            .and_then(WeakEntity::upgrade);
        if let Some(screen) = open {
            screen.update(cx, |screen, cx| screen.show_page(&page, cx));
        }
        Self::open(SettingsParams { project_path, page: Some(page) }, cx);
    }
}

impl Render for ProjectSettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let title = match self
            .screen
            .read(cx)
            .project_path()
            .and_then(|p| p.file_name())
            .map(|name| name.to_string_lossy().to_string())
        {
            Some(name) => format!("Project Settings \u{b7} {name}"),
            None => "Project Settings".to_string(),
        };
        v_flex()
            .size_full()
            .child(title_bar(title))
            .child(div().flex_1().min_h_0().child(self.screen.clone()))
    }
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for ProjectSettingsWindow {
    type Params = SettingsParams;

    fn window_name() -> &'static str {
        "ProjectSettingsWindow"
    }

    fn window_options(_: &SettingsParams) -> gpui::WindowOptions {
        window_manager::default_window_options(1200.0, 760.0)
    }

    fn build(params: SettingsParams, window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| ProjectSettingsWindow::new(params, window, cx))
    }
}

// ── Editor Settings ──────────────────────────────────────────────────────────

pub struct EditorSettingsWindow {
    screen: Entity<SettingsScreen>,
}

impl EditorSettingsWindow {
    /// Register the `"EditorSettingsWindow"` opener in the
    /// [`window_manager::WindowRegistry`]. Call once at startup.
    pub fn init(cx: &mut App) {
        use ui_shell::PulsarWindowExt;
        <Self as PulsarWindowExt>::register(cx);
    }

    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        engine_state::register_default_settings();
        let screen = cx.new(|cx| SettingsScreen::new(WindowKind::Editor, None, None, window, cx));
        Self { screen }
    }
}

impl Render for EditorSettingsWindow {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .child(title_bar("Editor Settings".to_string()))
            .child(div().flex_1().min_h_0().child(self.screen.clone()))
    }
}

impl window_manager::PulsarWindow for EditorSettingsWindow {
    type Params = ();

    fn window_name() -> &'static str {
        "EditorSettingsWindow"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(1100.0, 720.0)
    }

    fn build(_: (), window: &mut Window, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| EditorSettingsWindow::new(window, cx))
    }
}
