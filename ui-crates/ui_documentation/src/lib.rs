use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, Root, Sizable, StyledExt, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, IconName, Icon,
    text::TextView,
    resizable::{h_resizable, resizable_panel, ResizableState},
    input::{InputState, TextInput},
};
use ui_common::translate;
use std::path::PathBuf;

mod engine_docs;
mod project_docs;
mod manual_docs;

use engine_docs::{EngineDocsState, TreeNode};
use project_docs::{ProjectDocsState, ProjectTreeNode};
use manual_docs::{ManualDocsState, FileEntry, ViewMode};

// ============================================================================
// Main Documentation Window
// ============================================================================

pub struct DocumentationWindow {
    focus_handle: FocusHandle,
    current_category: DocCategory,
    sidebar_resizable_state: Entity<ResizableState>,
    project_root: Option<PathBuf>,

    // Docs states
    engine_docs: EngineDocsState,
    project_docs: ProjectDocsState,
    manual_docs: ManualDocsState,

    // Manual docs - new file dialog
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

        // Use provided project path or fallback to current directory
        let project_root = project_path.or_else(|| std::env::current_dir().ok());

        let engine_docs = EngineDocsState::new(window, cx);
        let project_docs = ProjectDocsState::new(window, cx, project_root.clone());
        let manual_docs = ManualDocsState::new(window, cx, project_root.clone());

        let new_file_input_state = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("filename.md", window, cx);
            state
        });

        // Subscribe to engine search input
        let engine_search_state = engine_docs.search_input_state.clone();
        cx.subscribe(&engine_search_state, |this: &mut Self, state, _event: &ui::input::InputEvent, cx| {
            this.engine_docs.search_query = state.read(cx).value().to_string();
            this.engine_docs.rebuild_visible_list();
            cx.notify();
        }).detach();

        // Subscribe to manual docs editor for live preview updates
        let manual_editor_state = manual_docs.editor_input_state.clone();
        cx.subscribe(&manual_editor_state, |this: &mut Self, state, _event: &ui::input::InputEvent, cx| {
            // Update preview asynchronously - just update the string, rendering happens on next frame
            this.manual_docs.update_preview(cx);
            cx.notify();
        }).detach();

        // Subscribe to new file input
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

    fn render_breadcrumbs(&self) -> Option<Vec<String>> {
        match self.current_category {
            DocCategory::Engine => {
                let path = self.engine_docs.current_path.as_ref()?;
                let parts: Vec<String> = path.split('/').map(|s| s.to_string()).collect();
                if parts.is_empty() {
                    return None;
                }
                Some(parts)
            }
            _ => None,
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
            .child(
                // Professional header with modern styling
                v_flex()
                    .w_full()
                    .gap_3()
                    .px_6()
                    .py_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .bg(theme.sidebar)
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_3()
                                    .items_center()
                                    .child(
                                        div()
                                            .w(px(48.0))
                                            .h(px(48.0))
                                            .rounded_lg()
                                            .bg(theme.accent.opacity(0.15))
                                            .border_1()
                                            .border_color(theme.accent.opacity(0.3))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .child(
                                                Icon::new(IconName::BookOpen)
                                                    .size(px(24.0))
                                                    .text_color(theme.accent)
                                            )
                                    )
                                    .child(
                                        v_flex()
                                            .gap_1()
                                            .child(
                                                div()
                                                    .text_2xl()
                                                    .font_weight(gpui::FontWeight::BOLD)
                                                    .text_color(theme.foreground)
                                                    .child("Documentation")
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .text_color(theme.muted_foreground)
                                                    .child("Browse and create project documentation")
                                            )
                                    )
                            )
                            .child(
                                Button::new("refresh-docs")
                                    .icon(IconName::Refresh)
                                    .ghost()
                                    .tooltip("Refresh Documentation")
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.refresh_current_category();
                                        cx.notify();
                                    }))
                            )
                    )
                    // Category tabs with modern styling
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .child(
                                Button::new("tab-engine")
                                    .label("Engine Docs")
                                    .icon(IconName::Code)
                                    .when(current_category == DocCategory::Engine, |btn| btn.primary())
                                    .when(current_category != DocCategory::Engine, |btn| btn.ghost())
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.current_category = DocCategory::Engine;
                                        cx.notify();
                                    }))
                            )
                            .child(
                                Button::new("tab-project")
                                    .label("Project Docs")
                                    .icon(IconName::Folder)
                                    .when(current_category == DocCategory::Project, |btn| btn.primary())
                                    .when(current_category != DocCategory::Project, |btn| btn.ghost())
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.current_category = DocCategory::Project;
                                        cx.notify();
                                    }))
                            )
                            .child(
                                Button::new("tab-manual")
                                    .label("Manual Docs")
                                    .icon(IconName::BookOpen)
                                    .when(current_category == DocCategory::Manual, |btn| btn.primary())
                                    .when(current_category != DocCategory::Manual, |btn| btn.ghost())
                                    .on_click(cx.listener(|this, _event, _window, cx| {
                                        this.current_category = DocCategory::Manual;
                                        cx.notify();
                                    }))
                            )
                    )
            )
            .child(
                // Main content area
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child({
                        let content = match current_category {
                            DocCategory::Engine => self.render_engine_docs(window, cx).into_any_element(),
                            DocCategory::Project => self.render_project_docs(window, cx).into_any_element(),
                            DocCategory::Manual => self.render_manual_docs(window, cx).into_any_element(),
                        };
                        content
                    })
            )
            .when(self.show_new_file_dialog, |this| {
                this.child(
                    // Modal overlay
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(gpui::black().opacity(0.5))
                        .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.show_new_file_dialog = false;
                            cx.notify();
                        }))
                        .child(
                            // Dialog content
                            div()
                                .w(px(400.0))
                                .bg(theme.background)
                                .border_1()
                                .border_color(theme.border)
                                .rounded_lg()
                                .shadow_lg()
                                .p_4()
                                .on_mouse_down(gpui::MouseButton::Left, |event, _phase, cx| {
                                    // Stop event propagation to prevent closing dialog when clicking inside
                                    cx.stop_propagation();
                                })
                                .child(
                                    v_flex()
                                        .gap_4()
                                        .child(
                                            div()
                                                .text_lg()
                                                .font_weight(gpui::FontWeight::BOLD)
                                                .text_color(theme.foreground)
                                                .child("Create New Documentation File")
                                        )
                                        .child(
                                            TextInput::new(&self.new_file_input_state)
                                                .w_full()
                                                .appearance(true)
                                                .bordered(true)
                                        )
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .justify_end()
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
                                                        .label("Create")
                                                        .primary()
                                                        .on_click(cx.listener(|this, _, window, cx| {
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
                                        )
                                )
                        )
                )
            })
    }
}

// ============================================================================
// Rendering Methods
// ============================================================================

impl DocumentationWindow {
    fn render_engine_docs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let breadcrumb_parts = self.render_breadcrumbs();
        
        let visible_items: Vec<_> = self.engine_docs.flat_visible_items.iter()
            .map(|&idx| self.engine_docs.tree_items[idx].clone())
            .collect();
        let markdown = self.engine_docs.markdown_content.clone();
        
        // Collect tree nodes first
        let tree_nodes: Vec<_> = visible_items.iter().map(|node| {
            self.render_tree_node(node, cx)
        }).collect();
        
        let theme = cx.theme();

        h_resizable("docs-horizontal", self.sidebar_resizable_state.clone())
            .child(
                resizable_panel()
                    .size(px(300.0))
                    .child(
                        v_flex()
                            .size_full()
                            .bg(theme.sidebar.opacity(0.5))
                            .border_r_1()
                            .border_color(theme.border.opacity(0.5))
                            .child(
                                // Professional sidebar header
                                div()
                                    .w_full()
                                    .px_4()
                                    .py_3()
                                    .bg(theme.background.opacity(0.3))
                                    .border_b_1()
                                    .border_color(theme.border.opacity(0.5))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                Icon::new(IconName::Code)
                                                    .size_4()
                                                    .text_color(theme.accent)
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(theme.foreground)
                                                    .child("API Reference")
                                            )
                                    )
                            )
                            .child(
                                // Search bar with modern styling
                                div()
                                    .w_full()
                                    .p_3()
                                    .border_b_1()
                                    .border_color(theme.border.opacity(0.5))
                                    .child(
                                        TextInput::new(&self.engine_docs.search_input_state)
                                            .w_full()
                                            .prefix(
                                                Icon::new(IconName::Search)
                                                    .size_4()
                                                    .text_color(theme.muted_foreground)
                                            )
                                            .appearance(true)
                                            .bordered(true)
                                    )
                            )
                            .child(
                                // Tree items with proper scroll
                                div()
                                    .id("engine-tree-scroll")
                                    .flex_1()
                                    .overflow_y_scroll()
                                    .child(
                                        v_flex()
                                            .w_full()
                                            .p_2()
                                            .gap_0p5()
                                            .font_family("monospace")
                                            .children(tree_nodes)
                                    )
                            )
                    )
            )
            .child(
                resizable_panel()
                    .child(
                        div()
                            .size_full()
                            .bg(theme.background)
                            .child(
                                div()
                                    .id("engine-content-scroll")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .child(
                                        v_flex()
                                            .w_full()
                                            .child(
                                                // Breadcrumbs with modern styling
                                                {
                                                    if let Some(parts) = breadcrumb_parts {
                                                        div()
                                                            .w_full()
                                                            .px_8()
                                                            .py_4()
                                                            .border_b_1()
                                                            .border_color(theme.border)
                                                            .bg(theme.sidebar.opacity(0.3))
                                                            .child({
                                                                let mut crumbs = h_flex().gap_2().items_center();
                                                                crumbs = crumbs
                                                                    .child(
                                                                        Icon::new(IconName::BookOpen)
                                                                            .size_4()
                                                                            .text_color(theme.accent)
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .text_sm()
                                                                            .text_color(theme.muted_foreground)
                                                                            .child("Documentation")
                                                                    );
                                                                for part in parts.iter() {
                                                                    crumbs = crumbs
                                                                        .child(
                                                                            Icon::new(IconName::ChevronRight)
                                                                                .size_3()
                                                                                .text_color(theme.muted_foreground)
                                                                        )
                                                                        .child(
                                                                            div()
                                                                                .text_sm()
                                                                                .font_weight(gpui::FontWeight::MEDIUM)
                                                                                .text_color(theme.foreground)
                                                                                .child(part.clone())
                                                                        );
                                                                }
                                                                crumbs
                                                            })
                                                            .into_any_element()
                                                    } else {
                                                        div().into_any_element()
                                                    }
                                                }
                                            )
                                            .child(
                                                div()
                                                    .w_full()
                                                    .max_w(px(1200.0))
                                                    .mx_auto()
                                                    .px_8()
                                                    .py_8()
                                                    .child(
                                                        TextView::markdown(
                                                            "docs-markdown",
                                                            markdown,
                                                            window,
                                                            cx,
                                                        )
                                                        .selectable()
                                                    )
                                            )
                                    )
                            )
                    )
            )
    }

    fn render_project_docs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let markdown = self.project_docs.markdown_content.clone();

        let visible_items: Vec<_> = self.project_docs.flat_visible_items.iter()
            .map(|&idx| self.project_docs.tree_items[idx].clone())
            .collect();

        // Collect tree nodes first
        let tree_nodes: Vec<_> = visible_items.iter().map(|node| {
            self.render_project_tree_node(node, cx)
        }).collect();

        let theme = cx.theme();

        h_resizable("docs-horizontal", self.sidebar_resizable_state.clone())
            .child(
                resizable_panel()
                    .size(px(300.0))
                    .child(
                        v_flex()
                            .size_full()
                            .bg(theme.sidebar.opacity(0.5))
                            .border_r_1()
                            .border_color(theme.border.opacity(0.5))
                            .child(
                                // Professional sidebar header
                                div()
                                    .w_full()
                                    .px_4()
                                    .py_3()
                                    .bg(theme.background.opacity(0.3))
                                    .border_b_1()
                                    .border_color(theme.border.opacity(0.5))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .items_center()
                                            .child(
                                                Icon::new(IconName::Folder)
                                                    .size_4()
                                                    .text_color(theme.accent)
                                            )
                                            .child(
                                                div()
                                                    .text_sm()
                                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                                    .text_color(theme.foreground)
                                                    .child("Project API")
                                            )
                                    )
                            )
                            .child(
                                // Search bar with modern styling
                                div()
                                    .w_full()
                                    .p_3()
                                    .border_b_1()
                                    .border_color(theme.border.opacity(0.5))
                                    .child(
                                        TextInput::new(&self.project_docs.search_input_state)
                                            .w_full()
                                            .prefix(
                                                Icon::new(IconName::Search)
                                                    .size_4()
                                                    .text_color(theme.muted_foreground)
                                            )
                                            .appearance(true)
                                            .bordered(true)
                                    )
                            )
                            .child(
                                // Tree items with proper scroll
                                div()
                                    .id("project-tree-scroll")
                                    .flex_1()
                                    .overflow_y_scroll()
                                    .child(
                                        v_flex()
                                            .w_full()
                                            .p_2()
                                            .gap_0p5()
                                            .font_family("monospace")
                                            .children(tree_nodes)
                                    )
                            )
                    )
            )
            .child(
                resizable_panel()
                    .child(
                        div()
                            .size_full()
                            .bg(theme.background)
                            .child(
                                div()
                                    .id("project-content-scroll")
                                    .size_full()
                                    .overflow_y_scroll()
                                    .child(
                                        div()
                                            .w_full()
                                            .max_w(px(1200.0))
                                            .mx_auto()
                                            .px_8()
                                            .pt_6()
                                            .pb_8()
                                            .child(
                                                TextView::markdown(
                                                    "project-docs-markdown",
                                                    markdown,
                                                    window,
                                                    cx,
                                                )
                                                .selectable()
                                            )
                                    )
                            )
                    )
            )
    }

    fn render_manual_docs(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let visible_files: Vec<_> = self.manual_docs.visible_entries.iter()
            .map(|&idx| self.manual_docs.file_tree[idx].clone())
            .collect();
        
        // Render file entries before getting theme
        let file_entries: Vec<AnyElement> = visible_files.into_iter().map(|entry| {
            self.render_file_entry_inline(&entry, cx)
        }).collect();
        
        let theme = cx.theme();

        h_resizable("docs-horizontal", self.sidebar_resizable_state.clone())
            .child(
                // File tree sidebar
                resizable_panel()
                    .size(px(250.0))
                    .child(
                        v_flex()
                            .size_full()
                            .bg(theme.sidebar)
                            .border_r_1()
                            .border_color(theme.border)
                            .child(
                                // Toolbar
                                h_flex()
                                    .w_full()
                                    .h(px(40.0))
                                    .items_center()
                                    .px_2()
                                    .gap_2()
                                    .border_b_1()
                                    .border_color(theme.border)
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(theme.foreground)
                                            .child("Files")
                                    )
                                    .child(div().flex_1())
                                    .child(
                                        Button::new("new-file")
                                            .icon(IconName::Plus)
                                            .ghost()
                                            .small()
                                            .tooltip("New File")
                                            .on_click(cx.listener(|this, _event, _window, cx| {
                                                this.show_new_file_dialog = true;
                                                cx.notify();
                                            }))
                                    )
                            )
                            .child(
                                // File tree with scroll
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .id("manual-tree-scroll")
                                            .size_full()
                                            .overflow_y_scroll()
                                            .child(
                                                v_flex()
                                                    .w_full()
                                                    .py_2()
                                                    .children(file_entries)
                                            )
                                    )
                            )
                    )
            )
            .child(
                // Editor/Preview area
                resizable_panel()
                    .child(
                        v_flex()
                            .size_full()
                            .bg(theme.background)
                            .child(
                                // Toolbar with view mode toggle
                                h_flex()
                                    .w_full()
                                    .h(px(40.0))
                                    .items_center()
                                    .px_4()
                                    .gap_2()
                                    .border_b_1()
                                    .border_color(theme.border)
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(theme.foreground)
                                            .child({
                                                let file_name = self.manual_docs.selected_file
                                                    .as_ref()
                                                    .and_then(|p| p.file_name())
                                                    .and_then(|n| n.to_str())
                                                    .unwrap_or("No file selected")
                                                    .to_string();
                                                file_name
                                            })
                                    )
                                    .child(div().flex_1())
                                    .child(
                                        Button::new("mode-editor")
                                            .label("Editor")
                                            .small()
                                            .when(self.manual_docs.view_mode == ViewMode::Editor, |btn| btn.primary())
                                            .when(self.manual_docs.view_mode != ViewMode::Editor, |btn| btn.ghost())
                                            .on_click(cx.listener(|this, _event, _window, cx| {
                                                this.manual_docs.set_view_mode(ViewMode::Editor);
                                                cx.notify();
                                            }))
                                    )
                                    .child(
                                        Button::new("mode-preview")
                                            .label("Preview")
                                            .small()
                                            .when(self.manual_docs.view_mode == ViewMode::Preview, |btn| btn.primary())
                                            .when(self.manual_docs.view_mode != ViewMode::Preview, |btn| btn.ghost())
                                            .on_click(cx.listener(|this, _event, _window, cx| {
                                                this.manual_docs.set_view_mode(ViewMode::Preview);
                                                cx.notify();
                                            }))
                                    )
                                    .child(
                                        Button::new("mode-split")
                                            .label("Split")
                                            .small()
                                            .when(self.manual_docs.view_mode == ViewMode::Split, |btn| btn.primary())
                                            .when(self.manual_docs.view_mode != ViewMode::Split, |btn| btn.ghost())
                                            .on_click(cx.listener(|this, _event, _window, cx| {
                                                this.manual_docs.set_view_mode(ViewMode::Split);
                                                cx.notify();
                                            }))
                                    )
                                    .child(
                                        Button::new("save-file")
                                            .icon(IconName::Check)
                                            .ghost()
                                            .small()
                                            .tooltip("Save File (updates preview)")
                                            .on_click(cx.listener(|this, _event, window, cx| {
                                                let _ = this.manual_docs.save_current_file(window, cx);
                                                cx.notify();
                                            }))
                                    )
                            )
                            .child({
                                // Content area based on view mode
                                // Clone view mode before match to avoid borrow conflicts
                                let view_mode = self.manual_docs.view_mode;
                                div()
                                    .flex_1()
                                    .overflow_hidden()
                                    .child({
                                        let content = match view_mode {
                                            ViewMode::Editor => self.render_editor_only(window, cx, gpui::transparent_black()).into_any_element(),
                                            ViewMode::Preview => self.render_preview_only(window, cx, gpui::transparent_black()).into_any_element(),
                                            ViewMode::Split => self.render_split_view(window, cx, gpui::transparent_black(), gpui::transparent_black()).into_any_element(),
                                        };
                                        content
                                    })
                            })
                    )
            )
    }

    fn render_editor_only(&mut self, _window: &mut Window, _cx: &mut Context<Self>, _bg: gpui::Hsla) -> impl IntoElement {
        let theme = _cx.theme();

        div()
            .size_full()
            .bg(theme.background)
            .p_4()
            .child(
                TextInput::new(&self.manual_docs.editor_input_state)
                    .w_full()
                    .h_full()
                    .appearance(true)
                    .bordered(true)
            )
    }

    fn render_preview_only(&mut self, window: &mut Window, cx: &mut Context<Self>, _bg: gpui::Hsla) -> impl IntoElement {
        let theme = cx.theme();
        let markdown = self.manual_docs.markdown_preview.clone();

        div()
            .size_full()
            .bg(theme.background)
            .overflow_hidden()
            .child(
                div()
                    .id("manual-preview-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(1200.0))
                            .mx_auto()
                            .px_8()
                            .pt_6()
                            .pb_8()
                            .child(
                                TextView::markdown(
                                    "manual-docs-preview",
                                    markdown,
                                    window,
                                    cx,
                                )
                                .debounce_ms(30)
                                .selectable()
                            )
                    )
            )
    }

    fn render_split_view(&mut self, window: &mut Window, cx: &mut Context<Self>, _bg: gpui::Hsla, _border: gpui::Hsla) -> impl IntoElement {
        let theme = cx.theme();
        let markdown = self.manual_docs.markdown_preview.clone();

        h_flex()
            .size_full()
            .child(
                // Editor (left)
                div()
                    .flex_1()
                    .size_full()
                    .bg(theme.background)
                    .border_r_1()
                    .border_color(theme.border)
                    .child(
                        TextInput::new(&self.manual_docs.editor_input_state)
                            .w_full()
                            .h_full()
                            .appearance(true)
                            .bordered(true)
                    )
            )
            .child(
                // Preview (right)
                div()
                    .id("manual-preview-split-scroll")
                    .flex_1()
                    .size_full()
                    .bg(theme.background)
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .max_w(px(1200.0))
                            .mx_auto()
                            .px_8()
                            .pt_6()
                            .pb_8()
                            .child(
                                TextView::markdown(
                                    "manual-docs-preview-split",
                                    markdown,
                                    window,
                                    cx,
                                )
                                .debounce_ms(30)
                                .selectable()
                            )
                    )
            )
    }

    fn render_file_entry(&self, entry: &FileEntry, cx: &mut Context<Self>, _theme: &ui::ThemeColor) -> AnyElement {
        self.render_file_entry_inline(entry, cx)
    }
    
    fn render_file_entry_inline(&self, entry: &FileEntry, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let is_selected = self.manual_docs.selected_file.as_ref() == Some(&entry.path);
        let is_expanded = self.manual_docs.expanded_folders.contains(&entry.path);
        let indent = px(entry.depth as f32 * 16.0);
        let path = entry.path.clone();
        let is_directory = entry.is_directory;

        div()
            .flex()
            .items_center()
            .gap_2()
            .h(px(28.0))
            .pl(indent + px(8.0))
            .pr_3()
            .rounded_sm()
            .when(is_selected, |style| style.bg(theme.accent).shadow_sm())
            .when(!is_selected, |style| style.hover(|s| s.bg(theme.accent.opacity(0.08))))
            .cursor_pointer()
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, window, cx| {
                if is_directory {
                    this.manual_docs.toggle_folder(path.clone());
                } else {
                    this.manual_docs.select_file(path.clone(), window, cx);
                }
                cx.notify();
            }))
            .child(
                Icon::new(if is_directory {
                    if is_expanded { IconName::FolderOpen } else { IconName::Folder }
                } else {
                    IconName::BookOpen
                })
                .size_4()
                .text_color(if is_selected {
                    theme.accent_foreground
                } else {
                    theme.accent.opacity(0.7)
                })
            )
            .child(
                div()
                    .text_sm()
                    .text_color(if is_selected {
                        theme.accent_foreground
                    } else {
                        theme.foreground.opacity(0.85)
                    })
                    .child(entry.name.clone())
            )
            .into_any_element()
    }

    fn render_tree_node(&self, node: &TreeNode, cx: &mut Context<Self>) -> AnyElement {
        match node {
            TreeNode::Crate { name, depth, .. } => {
                let is_expanded = self.engine_docs.expanded_paths.contains(name);
                let crate_name = name.clone();
                let indent = px(*depth as f32 * 16.0);
                let id = SharedString::from(format!("crate-{}", name));
                let theme = cx.theme();

                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(28.0))
                    .pl(indent + px(8.0))
                    .pr_3()
                    .rounded_sm()
                    .hover(|style| style.bg(theme.accent.opacity(0.08)))
                    .cursor_pointer()
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.engine_docs.toggle_expansion(crate_name.clone());
                        cx.notify();
                    }))
                    .child(
                        Icon::new(if is_expanded { IconName::FolderOpen } else { IconName::Folder })
                            .size_4()
                            .text_color(theme.accent)
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.foreground)
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(name.clone())
                    )
                    .into_any_element()
            }
            TreeNode::Section { crate_name, section_name, count, depth } => {
                let section_path = format!("{}/{}", crate_name, section_name);
                let is_expanded = self.engine_docs.expanded_paths.contains(&section_path);
                let section_path_clone = section_path.clone();
                let indent = px(*depth as f32 * 16.0);
                let id = SharedString::from(format!("section-{}-{}", crate_name, section_name));
                let theme = cx.theme();

                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(28.0))
                    .pl(indent + px(8.0))
                    .pr_3()
                    .rounded_sm()
                    .hover(|style| style.bg(theme.accent.opacity(0.08)))
                    .cursor_pointer()
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.engine_docs.toggle_expansion(section_path_clone.clone());
                        cx.notify();
                    }))
                    .child(
                        Icon::new(if is_expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                            .size_3p5()
                            .text_color(theme.foreground.opacity(0.7))
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.foreground.opacity(0.9))
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("{} ({})", section_name, count))
                    )
                    .into_any_element()
            }
            TreeNode::Item { item_name, path, depth, .. } => {
                let is_selected = self.engine_docs.current_path.as_ref() == Some(path);
                let path_clone = path.clone();
                let indent = px(*depth as f32 * 16.0);
                let id = SharedString::from(format!("item-{}", path.replace('/', "-")));
                let theme = cx.theme();

                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(28.0))
                    .pl(indent + px(16.0))
                    .pr_3()
                    .rounded_sm()
                    .when(is_selected, |style| {
                        style.bg(theme.accent).shadow_sm()
                    })
                    .when(!is_selected, |style| {
                        style.hover(|style| style.bg(theme.accent.opacity(0.08)))
                    })
                    .cursor_pointer()
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.engine_docs.load_content(&path_clone);
                        cx.notify();
                    }))
                    .child(
                        Icon::new(IconName::Code)
                            .size_3p5()
                            .text_color(if is_selected {
                                theme.accent_foreground
                            } else {
                                theme.accent.opacity(0.7)
                            })
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(if is_selected {
                                theme.accent_foreground
                            } else {
                                theme.foreground.opacity(0.85)
                            })
                            .child(item_name.clone())
                    )
                    .into_any_element()
            }
        }
    }

    fn render_project_tree_node(&self, node: &ProjectTreeNode, cx: &mut Context<Self>) -> AnyElement {
        match node {
            ProjectTreeNode::Category { name, count, depth } => {
                let is_expanded = self.project_docs.expanded_paths.contains(name);
                let category_name = name.clone();
                let indent = px(*depth as f32 * 16.0);
                let id = SharedString::from(format!("category-{}", name));
                let theme = cx.theme();

                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(28.0))
                    .pl(indent + px(8.0))
                    .pr_3()
                    .rounded_sm()
                    .hover(|style| style.bg(theme.accent.opacity(0.08)))
                    .cursor_pointer()
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.project_docs.toggle_expansion(category_name.clone());
                        cx.notify();
                    }))
                    .child(
                        Icon::new(if is_expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                            .size_3p5()
                            .text_color(theme.foreground.opacity(0.7))
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(theme.foreground.opacity(0.9))
                            .font_weight(FontWeight::MEDIUM)
                            .child(format!("{} ({})", name, count))
                    )
                    .into_any_element()
            }
            ProjectTreeNode::Item { item_name, path, depth, .. } => {
                let is_selected = self.project_docs.current_path.as_ref() == Some(path);
                let path_clone = path.clone();
                let indent = px(*depth as f32 * 16.0);
                let id = SharedString::from(format!("project-item-{}", path.replace("::", "-")));
                let theme = cx.theme();

                div()
                    .id(id)
                    .flex()
                    .items_center()
                    .gap_2()
                    .h(px(28.0))
                    .pl(indent + px(16.0))
                    .pr_3()
                    .rounded_sm()
                    .when(is_selected, |style| {
                        style.bg(theme.accent).shadow_sm()
                    })
                    .when(!is_selected, |style| {
                        style.hover(|style| style.bg(theme.accent.opacity(0.08)))
                    })
                    .cursor_pointer()
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |this, _, _, cx| {
                        this.project_docs.load_content(&path_clone);
                        cx.notify();
                    }))
                    .child(
                        Icon::new(IconName::Code)
                            .size_3p5()
                            .text_color(if is_selected {
                                theme.accent_foreground
                            } else {
                                theme.accent.opacity(0.7)
                            })
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(if is_selected {
                                theme.accent_foreground
                            } else {
                                theme.foreground.opacity(0.85)
                            })
                            .child(item_name.clone())
                    )
                    .into_any_element()
            }
        }
    }
}

pub fn create_documentation_window(window: &mut Window, cx: &mut App) -> Entity<Root> {
    create_documentation_window_with_project(window, cx, None)
}

pub fn create_documentation_window_with_project(window: &mut Window, cx: &mut App, project_path: Option<PathBuf>) -> Entity<Root> {
    let docs = cx.new(|cx| DocumentationWindow::new_with_project(window, cx, project_path));
    cx.new(|cx| Root::new(docs.into(), window, cx))
}
