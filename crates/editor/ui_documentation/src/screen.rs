use std::path::PathBuf;

use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, Root, TitleBar,
    input::{InputEvent, InputState},
    resizable::{ResizableState, h_resizable, resizable_panel},
    scroll::ScrollbarState,
    v_flex,
};
use ui_common::translate;

use crate::components;
use crate::utils::{DocCategory, EngineDocsState, ManualDocsState, ProjectDocsState};

pub struct DocumentationWindow {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) current_category: DocCategory,
    pub(crate) sidebar_resizable_state: Entity<ResizableState>,

    pub(crate) engine_docs: EngineDocsState,
    pub(crate) project_docs: ProjectDocsState,
    pub(crate) manual_docs: ManualDocsState,

    /// The sidebar tree is one virtualized list shared by the three sources.
    pub(crate) tree_scroll: UniformListScrollHandle,
    pub(crate) tree_scrollbar: ScrollbarState,

    pub(crate) new_file_name: String,
    pub(crate) new_file_input_state: Entity<InputState>,
    pub(crate) show_new_file_dialog: bool,
}

impl DocumentationWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_project(window, cx, None)
    }

    pub fn new_with_project(
        window: &mut Window,
        cx: &mut Context<Self>,
        project_path: Option<PathBuf>,
    ) -> Self {
        let sidebar_resizable_state = ResizableState::new(cx);
        let project_root = project_path.or_else(|| std::env::current_dir().ok());

        let engine_docs = EngineDocsState::new(window, cx);
        let project_docs = ProjectDocsState::new(window, cx, project_root.clone());
        let manual_docs = ManualDocsState::new(window, cx, project_root.clone());

        let new_file_input_state = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("getting-started", window, cx);
            state
        });

        cx.subscribe(
            &engine_docs.search_input_state.clone(),
            |this: &mut Self, state, _: &InputEvent, cx| {
                this.engine_docs.search_query = state.read(cx).value().to_string();
                this.engine_docs.rebuild_visible_list();
                cx.notify();
            },
        )
        .detach();

        cx.subscribe(
            &project_docs.search_input_state.clone(),
            |this: &mut Self, state, _: &InputEvent, cx| {
                this.project_docs.search_query = state.read(cx).value().to_string();
                this.project_docs.rebuild_visible_list();
                cx.notify();
            },
        )
        .detach();

        cx.subscribe(
            &manual_docs.editor_input_state.clone(),
            |this: &mut Self, _state, _: &InputEvent, cx| {
                this.manual_docs.update_preview(cx);
                cx.notify();
            },
        )
        .detach();

        cx.subscribe(
            &new_file_input_state.clone(),
            |this: &mut Self, state, _: &InputEvent, cx| {
                this.new_file_name = state.read(cx).value().to_string();
                cx.notify();
            },
        )
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            current_category: DocCategory::Engine,
            sidebar_resizable_state,
            engine_docs,
            project_docs,
            manual_docs,
            tree_scroll: UniformListScrollHandle::new(),
            tree_scrollbar: ScrollbarState::default(),
            new_file_name: String::new(),
            new_file_input_state,
            show_new_file_dialog: false,
        }
    }
}

impl Focusable for DocumentationWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for DocumentationWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let background = cx.theme().background;
        let sidebar = self.render_sidebar(cx);
        let content = self.render_content(window, cx);

        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .relative()
            .bg(background)
            .child(TitleBar::new().child(translate("Window.Title.Documentation")))
            .child(
                div().flex_1().min_h_0().child(
                    h_resizable("docs-split")
                        .state(self.sidebar_resizable_state.clone())
                        .child(resizable_panel().size(px(300.0)).child(sidebar))
                        .child(resizable_panel().child(content)),
                ),
            )
            .when(self.show_new_file_dialog, |this| {
                this.child(components::render_new_file_dialog(self, cx))
            })
    }
}

#[window_manager::register_window]
impl window_manager::PulsarWindow for DocumentationWindow {
    type Params = ();

    fn window_name() -> &'static str {
        "DocumentationWindow"
    }

    fn window_options(_: &()) -> gpui::WindowOptions {
        window_manager::default_window_options(1180.0, 760.0)
    }

    fn build(_: (), window: &mut Window, cx: &mut App) -> gpui::Entity<Self> {
        cx.new(|cx| DocumentationWindow::new_with_project(window, cx, None))
    }
}

pub fn create_documentation_window(window: &mut Window, cx: &mut App) -> Entity<Root> {
    create_documentation_window_with_project(window, cx, None)
}

pub fn create_documentation_window_with_project(
    window: &mut Window,
    cx: &mut App,
    project_path: Option<PathBuf>,
) -> Entity<Root> {
    let docs = cx.new(|cx| DocumentationWindow::new_with_project(window, cx, project_path));
    cx.new(|cx| Root::new(docs.into(), window, cx))
}
