use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, Root, Sizable, StyledExt, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, IconName, Icon,
    resizable::ResizableState,
    input::{InputState, TextInput},
};
use ui_common::translate;
use std::path::PathBuf;

mod engine_docs;
mod project_docs;
mod manual_docs;
mod panels;

use engine_docs::EngineDocsState;
use project_docs::ProjectDocsState;
use manual_docs::{ManualDocsState, ViewMode};
use panels::{EngineDocsPanel, ProjectDocsPanel, ManualDocsPanel};

// ============================================================================
// Main Documentation Window
// ============================================================================

pub struct DocumentationWindow {
    focus_handle: FocusHandle,
    current_category: DocCategory,
    sidebar_resizable_state: Entity<ResizableState>,
    project_root: Option<PathBuf>,

    // State
    engine_docs: EngineDocsState,
    project_docs: ProjectDocsState,
    manual_docs: ManualDocsState,

    // Panels
    engine_panel: EngineDocsPanel,
    project_panel: ProjectDocsPanel,
    manual_panel: ManualDocsPanel,

    // New file dialog state
    new_file_name: String,
    new_file_input_state: Entity<InputState>,
    show_new_file_dialog: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum DocCategory {
    Engine,
    Project,
    Manual,
}

impl DocumentationWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_project(window, cx, None)
    }

    pub fn new_with_project(window: &mut Window, cx: &mut Context<Self>, project_path: Option<PathBuf>) -> Self {
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

        // Subscribe to search inputs
        let engine_search_state = engine_docs.search_input_state.clone();
        cx.subscribe(&engine_search_state, |this: &mut Self, state, _event: &ui::input::InputEvent, cx| {
            this.engine_docs.search_query = state.read(cx).value().to_string();
            this.engine_docs.rebuild_visible_list();
            cx.notify();
        }).detach();

        let manual_editor_state = manual_docs.editor_input_state.clone();
        cx.subscribe(&manual_editor_state, |this: &mut Self, _state, _event: &ui::input::InputEvent, cx| {
            this.manual_docs.update_preview(cx);
            cx.notify();
        }).detach();

        let new_file_state = new_file_input_state.clone();
        cx.subscribe(&new_file_state, |this: &mut Self, state, _event: &ui::input::InputEvent, cx| {
            this.new_file_name = state.read(cx).value().to_string();
            cx.notify();
        }).detach();

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

    fn refresh_current_category(&mut self) {
        match self.current_category {
            DocCategory::Engine => {
                self.engine_docs.tree_items.clear();
                self.engine_docs.expanded_paths.clear();
                self.engine_docs.load_documentation();
            }
            DocCategory::Manual => {
                self.manual_docs.load_file_tree();
            }
            _ => {}
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
                this.child(self.render_new_file_dialog(&theme, window, cx))
            })
    }
}

// ============================================================================
// UI Components
// ============================================================================

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
                                    .size(px(18.0))
                                    .text_color(theme.accent)
                            )
                    )
                    .child(
                        div()
                            .text_lg()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child("Documentation")
                    )
            )
            .child(
                Button::new("refresh-docs")
                    .icon(IconName::Refresh)
                    .ghost()
                    .xsmall()
                    .tooltip("Refresh Documentation")
                    .on_click(cx.listener(|this, _event, _window, cx| {
                        this.refresh_current_category();
                        cx.notify();
                    }))
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
                // Grouped tab selector with pill background
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
                                btn.bg(theme.accent)
                                    .text_color(theme.accent_foreground)
                            })
                            .when(current_category != DocCategory::Engine, |btn| {
                                btn.ghost()
                            })
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.current_category = DocCategory::Engine;
                                cx.notify();
                            }))
                    )
                    .child(
                        Button::new("tab-project")
                            .label("Project API")
                            .icon(IconName::Folder)
                            .small()
                            .when(current_category == DocCategory::Project, |btn| {
                                btn.bg(theme.accent)
                                    .text_color(theme.accent_foreground)
                            })
                            .when(current_category != DocCategory::Project, |btn| {
                                btn.ghost()
                            })
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.current_category = DocCategory::Project;
                                cx.notify();
                            }))
                    )
                    .child(
                        Button::new("tab-manual")
                            .label("Documentation")
                            .icon(IconName::BookOpen)
                            .small()
                            .when(current_category == DocCategory::Manual, |btn| {
                                btn.bg(theme.accent)
                                    .text_color(theme.accent_foreground)
                            })
                            .when(current_category != DocCategory::Manual, |btn| {
                                btn.ghost()
                            })
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.current_category = DocCategory::Manual;
                                cx.notify();
                            }))
                    )
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
                DocCategory::Engine => self.engine_panel.render(
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
                ).into_any_element(),
                DocCategory::Project => self.project_panel.render(
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
                ).into_any_element(),
                DocCategory::Manual => self.manual_panel.render(
                    &self.manual_docs,
                    self.sidebar_resizable_state.clone(),
                    |this: &mut Self, _event, _window, cx| {
                        this.show_new_file_dialog = true;
                        cx.notify();
                    },
                    |this: &mut Self, _event, window, cx| {
                        let _ = this.manual_docs.save_current_file(window, cx);
                        cx.notify();
                    },
                    |this: &mut Self, mode, _window, cx| {
                        this.manual_docs.set_view_mode(mode);
                        cx.notify();
                    },
                    window,
                    cx,
                ).into_any_element(),
            })
    }

    fn render_new_file_dialog(
        &self,
        theme: &ui::ThemeColor,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::black().opacity(0.6))
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                this.show_new_file_dialog = false;
                cx.notify();
            }))
            .child(
                div()
                    .w(px(480.0))
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_xl()
                    .shadow_2xl()
                    .overflow_hidden()
                    .on_mouse_down(gpui::MouseButton::Left, |_event, _phase, cx| {
                        cx.stop_propagation();
                    })
                    .child(
                        v_flex()
                            .child(self.render_dialog_header(theme, cx))
                            .child(self.render_dialog_body(theme))
                            .child(self.render_dialog_footer(theme, window, cx))
                    )
            )
    }

    fn render_dialog_header(
        &self,
        theme: &ui::ThemeColor,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(px(56.0))
            .px_6()
            .items_center()
            .justify_between()
            .bg(theme.sidebar)
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .gap_3()
                    .items_center()
                    .child(
                        Icon::new(IconName::Plus)
                            .size_4()
                            .text_color(theme.accent)
                    )
                    .child(
                        div()
                            .text_base()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(theme.foreground)
                            .child("Create Documentation File")
                    )
            )
            .child(
                Button::new("close-dialog")
                    .icon(IconName::Close)
                    .ghost()
                    .xsmall()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_new_file_dialog = false;
                        this.new_file_name.clear();
                        cx.notify();
                    }))
            )
    }

    fn render_dialog_body(&self, theme: &ui::ThemeColor) -> impl IntoElement {
        v_flex()
            .w_full()
            .p_6()
            .gap_4()
            .child(
                v_flex()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.foreground)
                            .child("File Name")
                    )
                    .child(
                        TextInput::new(&self.new_file_input_state)
                            .w_full()
                            .appearance(true)
                            .bordered(true)
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("File will be saved with .md extension")
                    )
            )
    }

    fn render_dialog_footer(
        &self,
        theme: &ui::ThemeColor,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(px(64.0))
            .px_6()
            .items_center()
            .gap_3()
            .justify_end()
            .bg(theme.sidebar.opacity(0.5))
            .border_t_1()
            .border_color(theme.border)
            .child(
                Button::new("cancel-new-file")
                    .label("Cancel")
                    .ghost()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.show_new_file_dialog = false;
                        this.new_file_name.clear();
                        cx.notify();
                    }))
            )
            .child(
                Button::new("create-new-file")
                    .label("Create File")
                    .icon(IconName::Plus)
                    .primary()
                    .on_click(cx.listener(move |this, _, window, cx| {
                        if !this.new_file_name.is_empty() {
                            let file_name = this.new_file_name.clone();
                            if let Err(e) = this.manual_docs.create_new_file(file_name, window, cx) {
                                eprintln!("Failed to create file: {}", e);
                            }
                            this.show_new_file_dialog = false;
                            this.new_file_name.clear();
                            cx.notify();
                        }
                    }))
            )
    }
}

// ============================================================================
// Public API
// ============================================================================

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
