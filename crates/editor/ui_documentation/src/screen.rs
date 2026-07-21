use std::path::PathBuf;

use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, Icon, IconName, Root, Sizable, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{InputState, TextInput},
    resizable::ResizableState,
    v_flex,
};
use ui_common::translate;

use crate::components;
use crate::handlers;
use crate::components::{EngineDocsPanel, ManualDocsPanel, ProjectDocsPanel};
use crate::utils::{DocCategory, EngineDocsState, ManualDocsState, ProjectDocsState};

pub struct DocumentationWindow {
    pub(crate) focus_handle: FocusHandle,
    pub(crate) current_category: DocCategory,
    pub(crate) sidebar_resizable_state: Entity<ResizableState>,
    pub(crate) project_root: Option<PathBuf>,

    pub(crate) engine_docs: EngineDocsState,
    pub(crate) project_docs: ProjectDocsState,
    pub(crate) manual_docs: ManualDocsState,

    pub(crate) engine_panel: EngineDocsPanel,
    pub(crate) project_panel: ProjectDocsPanel,
    pub(crate) manual_panel: ManualDocsPanel,

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
            state.set_placeholder("filename.md", window, cx);
            state
        });

        let engine_search_state = engine_docs.search_input_state.clone();
        cx.subscribe(
            &engine_search_state,
            |this: &mut Self, state, _event: &ui::input::InputEvent, cx| {
                this.engine_docs.search_query = state.read(cx).value().to_string();
                this.engine_docs.rebuild_visible_list();
                cx.notify();
            },
        )
        .detach();

        let manual_editor_state = manual_docs.editor_input_state.clone();
        cx.subscribe(
            &manual_editor_state,
            |this: &mut Self, _state, _event: &ui::input::InputEvent, cx| {
                this.manual_docs.update_preview(cx);
                cx.notify();
            },
        )
        .detach();

        let new_file_state = new_file_input_state.clone();
        cx.subscribe(
            &new_file_state,
            |this: &mut Self, state, _event: &ui::input::InputEvent, cx| {
                this.new_file_name = state.read(cx).value().to_string();
                cx.notify();
            },
        )
        .detach();

        Self {
            focus_handle: cx.focus_handle(),
            current_category: DocCategory::Engine,
            sidebar_resizable_state,
            project_root,
            engine_docs,
            project_docs,
            manual_docs,
            engine_panel: EngineDocsPanel::new(),
            project_panel: ProjectDocsPanel::new(),
            manual_panel: ManualDocsPanel::new(),
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
        let theme = cx.theme().clone();
        let current_category = self.current_category;

        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .child(TitleBar::new().child(translate("Window.Title.Documentation")))
            .child(self.render_header(&theme, current_category, cx))
            .child(self.render_tabs(&theme, current_category, cx))
            .child(self.render_content(current_category, window, cx))
            .when(self.show_new_file_dialog, |this| {
                this.child(components::render_new_file_dialog(
                    self,
                    &theme,
                    window,
                    cx,
                ))
            })
    }
}

impl DocumentationWindow {
    fn render_header(
        &self,
        theme: &ui::ThemeColor,
        _current_category: DocCategory,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(px(56.0))
            .items_center()
            .px_6()
            .justify_between()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.sidebar)
            .child(
                h_flex()
                    .gap_4()
                    .items_center()
                    .child(
                        div()
                            .w(px(36.0))
                            .h(px(36.0))
                            .rounded_lg()
                            .bg(theme.accent.opacity(0.12))
                            .border_1()
                            .border_color(theme.accent.opacity(0.25))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(
                                Icon::new(IconName::BookOpen)
                                    .size(px(18.0)),
                            ),
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child("Documentation"),
                    ),
            )
            .child(
                Button::new("refresh-docs")
                    .icon(IconName::Refresh)
                    .ghost()
                    .xsmall()
                    .tooltip("Refresh Documentation")
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        handlers::refresh_current_category(this);
                        cx.notify();
                    })),
            )
    }

    fn render_tabs(
        &self,
        theme: &ui::ThemeColor,
        current_category: DocCategory,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(px(48.0))
            .items_center()
            .px_6()
            .gap_2()
            .bg(theme.sidebar.opacity(0.6))
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_px()
                    .p_px()
                    .rounded_lg()
                    .bg(theme.muted.opacity(0.3))
                    .child(
                        Button::new("tab-engine")
                            .label("Engine API")
                            .icon(IconName::Code)
                            .small()
                            .when(current_category == DocCategory::Engine, |btn| {
                                btn.bg(theme.accent).text_color(theme.accent_foreground)
                            })
                            .when(current_category != DocCategory::Engine, |btn| btn.ghost())
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                handlers::set_category(this, DocCategory::Engine);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("tab-project")
                            .label("Project API")
                            .icon(IconName::Folder)
                            .small()
                            .when(current_category == DocCategory::Project, |btn| {
                                btn.bg(theme.accent).text_color(theme.accent_foreground)
                            })
                            .when(current_category != DocCategory::Project, |btn| btn.ghost())
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                handlers::set_category(this, DocCategory::Project);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("tab-manual")
                            .label("Documentation")
                            .icon(IconName::BookOpen)
                            .small()
                            .when(current_category == DocCategory::Manual, |btn| {
                                btn.bg(theme.accent).text_color(theme.accent_foreground)
                            })
                            .when(current_category != DocCategory::Manual, |btn| btn.ghost())
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                handlers::set_category(this, DocCategory::Manual);
                                cx.notify();
                            })),
                    ),
            )
    }

    fn render_content(
        &mut self,
        current_category: DocCategory,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .flex_1()
            .overflow_hidden()
            .child(match current_category {
                DocCategory::Engine => self
                    .engine_panel
                    .render(
                        &self.engine_docs,
                        self.sidebar_resizable_state.clone(),
                        |this: &mut Self, path, _window, cx| {
                            this.engine_docs.toggle_expansion(path);
                            cx.notify();
                        },
                        |this: &mut Self, path, _window, cx| {
                            this.engine_docs.load_content(&path);
                            cx.notify();
                        },
                        window,
                        cx,
                    )
                    .into_any_element(),
                DocCategory::Project => self
                    .project_panel
                    .render(
                        &self.project_docs,
                        self.sidebar_resizable_state.clone(),
                        |this: &mut Self, path, _window, cx| {
                            this.project_docs.toggle_expansion(path);
                            cx.notify();
                        },
                        |this: &mut Self, path, _window, cx| {
                            this.project_docs.load_content(&path);
                            cx.notify();
                        },
                        window,
                        cx,
                    )
                    .into_any_element(),
                DocCategory::Manual => self
                    .manual_panel
                    .render(
                        &self.manual_docs,
                        self.sidebar_resizable_state.clone(),
                        |this: &mut Self, _event, _window, cx| {
                            handlers::open_new_file_dialog(this);
                            cx.notify();
                        },
                        |this: &mut Self, _event, window, cx| {
                            handlers::save_current_file(this, window, cx);
                            cx.notify();
                        },
                        |this: &mut Self, mode, _window, cx| {
                            handlers::set_view_mode(this, mode);
                            cx.notify();
                        },
                        window,
                        cx,
                    )
                    .into_any_element(),
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
        window_manager::default_window_options(1100.0, 750.0)
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
