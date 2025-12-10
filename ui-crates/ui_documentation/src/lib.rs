use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, Root, Sizable, StyledExt, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, IconName, Icon,
    text::TextView,
    resizable::{h_resizable, resizable_panel, ResizableState},
    input::{InputState, InputEvent, TextInput},
};
use pulsar_docs::{get_doc_content, get_crate_index, list_crates, docs_available, CrateIndex};
use std::collections::HashMap;

pub struct DocumentationWindow {
    focus_handle: FocusHandle,
    current_path: Option<String>,
    tree_items: Vec<TreeNode>,
    flat_visible_items: Vec<usize>,
    expanded_paths: HashMap<String, bool>,
    markdown_content: String,
    search_query: String,
    sidebar_resizable_state: Entity<ResizableState>,
    search_input_state: Entity<InputState>,
}

#[derive(Clone, Debug)]
enum TreeNode {
    Crate {
        name: String,
        index: CrateIndex,
        depth: usize,
    },
    Section {
        crate_name: String,
        section_name: String,
        count: usize,
        depth: usize,
    },
    Item {
        crate_name: String,
        section_name: String,
        item_name: String,
        path: String,
        doc_summary: Option<String>,
        depth: usize,
    },
}

impl DocumentationWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sidebar_resizable_state = ResizableState::new(cx);
        let search_input_state = cx.new(|cx| {
            let mut state = InputState::new(window, cx);
            state.set_placeholder("Search documentation...", window, cx);
            state
        });

        let mut doc_window = Self {
            focus_handle: cx.focus_handle(),
            current_path: None,
            tree_items: Vec::new(),
            flat_visible_items: Vec::new(),
            expanded_paths: HashMap::new(),
            markdown_content: "# Pulsar Engine Documentation\n\nSelect an item from the sidebar to view its documentation.".to_string(),
            search_query: String::new(),
            sidebar_resizable_state,
            search_input_state: search_input_state.clone(),
        };

        // Subscribe to search input changes
        cx.subscribe(&search_input_state, |this: &mut Self, state, _event: &InputEvent, cx| {
            this.search_query = state.read(cx).text().to_string();
            this.rebuild_visible_list();
            cx.notify();
        }).detach();

        doc_window.load_documentation();
        doc_window
    }

    fn load_documentation(&mut self) {
        if !docs_available() {
            self.markdown_content = "# No Documentation Available\n\nDocumentation has not been generated yet. Build in release mode to generate docs.".to_string();
            return;
        }

        // Build flat tree structure
        let mut crates = list_crates();
        crates.sort();

        for crate_name in crates {
            if let Some(index) = get_crate_index(&crate_name) {
                // Add crate node
                self.tree_items.push(TreeNode::Crate {
                    name: crate_name.clone(),
                    index: index.clone(),
                    depth: 0,
                });

                // Add section nodes (sorted)
                let mut sections = index.sections.clone();
                sections.sort_by(|a, b| a.name.cmp(&b.name));

                for section in &sections {
                    self.tree_items.push(TreeNode::Section {
                        crate_name: crate_name.clone(),
                        section_name: section.name.clone(),
                        count: section.count,
                        depth: 1,
                    });

                    // Add item nodes (sorted)
                    let mut items = section.items.clone();
                    items.sort_by(|a, b| a.name.cmp(&b.name));

                    for item in &items {
                        self.tree_items.push(TreeNode::Item {
                            crate_name: crate_name.clone(),
                            section_name: section.name.clone(),
                            item_name: item.name.clone(),
                            path: format!("{}/{}", crate_name, item.path),
                            doc_summary: item.doc_summary.clone(),
                            depth: 2,
                        });
                    }
                }
            }
        }

        self.rebuild_visible_list();
    }

    fn rebuild_visible_list(&mut self) {
        self.flat_visible_items.clear();
        let query = self.search_query.to_lowercase();
        let is_searching = !query.is_empty();

        for (idx, node) in self.tree_items.iter().enumerate() {
            match node {
                TreeNode::Crate { name, .. } => {
                    let matches = name.to_lowercase().contains(&query);
                    if !is_searching || matches {
                        self.flat_visible_items.push(idx);
                    }
                }
                TreeNode::Section { crate_name, section_name, .. } => {
                    let matches = section_name.to_lowercase().contains(&query);
                    let parent_expanded = self.is_expanded(crate_name);

                    if is_searching && matches {
                        // Auto-expand parent when match found
                        self.expanded_paths.insert(crate_name.clone(), true);
                    }

                    if (parent_expanded || (is_searching && matches)) && (!is_searching || matches || self.is_expanded(crate_name)) {
                        self.flat_visible_items.push(idx);
                    }
                }
                TreeNode::Item { crate_name, section_name, item_name, .. } => {
                    let section_path = format!("{}/{}", crate_name, section_name);
                    let matches = item_name.to_lowercase().contains(&query);
                    let section_expanded = self.is_expanded(&section_path);

                    if is_searching && matches {
                        // Auto-expand parents when match found
                        self.expanded_paths.insert(crate_name.clone(), true);
                        self.expanded_paths.insert(section_path.clone(), true);
                    }

                    if (section_expanded || (is_searching && matches)) && (!is_searching || matches) {
                        self.flat_visible_items.push(idx);
                    }
                }
            }
        }

        // Show "no results" message when searching with no matches
        if is_searching && self.flat_visible_items.is_empty() {
            self.markdown_content = format!(
                "# No Results\n\nNo documentation found matching \"{}\".\n\nTry a different search term.",
                self.search_query
            );
        }
    }

    fn is_expanded(&self, path: &str) -> bool {
        self.expanded_paths.get(path).copied().unwrap_or(false)
    }

    fn toggle_expansion(&mut self, path: String, cx: &mut Context<Self>) {
        let is_expanded = self.is_expanded(&path);
        self.expanded_paths.insert(path, !is_expanded);
        self.rebuild_visible_list();
        cx.notify();
    }

    fn load_content(&mut self, path: &str, cx: &mut Context<Self>) {
        self.current_path = Some(path.to_string());

        if let Some(markdown) = get_doc_content(path) {
            self.markdown_content = markdown;
        } else {
            self.markdown_content = format!("# Error\n\nFailed to load documentation: {}", path);
        }

        cx.notify();
    }

    fn render_breadcrumbs(&self) -> Option<Vec<String>> {
        let path = self.current_path.as_ref()?;
        let parts: Vec<String> = path.split('/').map(|s| s.to_string()).collect();

        if parts.is_empty() {
            return None;
        }

        Some(parts)
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
        let bg = theme.background;
        let sidebar_bg = theme.sidebar;
        let border = theme.border;
        let fg = theme.foreground;
        let muted_fg = theme.muted_foreground;

        // Clone data needed for rendering to avoid borrow issues
        let visible_items: Vec<_> = self.flat_visible_items.iter()
            .map(|&idx| self.tree_items[idx].clone())
            .collect();

        let markdown = self.markdown_content.clone();
        let breadcrumb_parts = self.render_breadcrumbs();

        v_flex()
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(bg)
            .child(TitleBar::new().child("Documentation"))
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(
                // Compact Header (no search)
                div()
                    .w_full()
                    .h(px(48.0))
                    .flex()
                    .items_center()
                    .px_4()
                    .gap_4()
                    .bg(sidebar_bg)
                    .border_b_1()
                    .border_color(border)
                    .child(
                        // Icon + Title
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(IconName::BookOpen).size_5().text_color(theme.accent))
                            .child(
                                div()
                                    .text_base()
                                    .font_semibold()
                                    .text_color(fg)
                                    .child("Documentation")
                            )
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("refresh-docs")
                            .icon(IconName::Refresh)
                            .ghost()
                            .small()
                            .tooltip("Refresh Documentation")
                            .on_click(cx.listener(|this, _event, _window, cx| {
                                this.tree_items.clear();
                                this.expanded_paths.clear();
                                this.load_documentation();
                                cx.notify();
                            }))
                    )
            )
            .child(
                // Main content area with resizable sidebar
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        h_resizable("docs-horizontal", self.sidebar_resizable_state.clone())
                    .child(
                        resizable_panel()
                            .size(px(300.0))
                            .child(
                                // Sidebar with search and tree
                                v_flex()
                                    .id("docs-sidebar")
                                    .h_full()
                                    .bg(sidebar_bg)
                                    .border_r_1()
                                    .border_color(border)
                                    .font_family("monospace")
                                    .font(gpui::Font {
                                        family: "JetBrains Mono".to_string().into(),
                                        weight: gpui::FontWeight::NORMAL,
                                        style: gpui::FontStyle::Normal,
                                        features: gpui::FontFeatures::default(),
                                        fallbacks: Some(gpui::FontFallbacks::from_fonts(vec!["monospace".to_string()])),
                                    })
                                    .child(
                                        // Search bar in sidebar
                                        div()
                                            .w_full()
                                            .p_2()
                                            .border_b_1()
                                            .border_color(border)
                                            .child(
                                                TextInput::new(&self.search_input_state)
                                                    .prefix(Icon::new(IconName::Search).size_4())
                                                    .appearance(true)
                                                    .bordered(true)
                                                    .small()
                                            )
                                    )
                                    .child(
                                        // Tree items (scrollable)
                                        div()
                                            .flex_1()
                                            .overflow_y(gpui::Overflow::Scroll)
                                            .child(
                                                v_flex()
                                                    .w_full()
                                                    .py_2()
                                                    .px_2()
                                                    .children(
                                                        visible_items.iter().map(|node| {
                                                            self.render_tree_node(node, cx)
                                                        })
                                                    )
                                            )
                                    )
                            )
                    )
                    .child(
                        resizable_panel()
                            .child(
                                // Content area with markdown
                                div()
                                    .id("docs-content")
                                    .flex_1()
                                    .h_full()
                                    .overflow_y(gpui::Overflow::Scroll)
                                    .bg(bg)
                                    .child(
                                        div()
                                            .w_full()
                                            .max_w(px(1200.0))
                                            .mx_auto()
                                            .child(
                                                v_flex()
                                                    .w_full()
                                                    .gap_4()
                                                    // Breadcrumb navigation
                                                    .when_some(breadcrumb_parts, |this, parts| {
                                                        this.child(
                                                            div()
                                                                .pt_6()
                                                                .px_8()
                                                                .border_b_1()
                                                                .border_color(border)
                                                                .pb_3()
                                                                .child({
                                                                    let mut crumbs = h_flex().gap_2().items_center();
                                                                    crumbs = crumbs.child(
                                                                        div()
                                                                            .text_sm()
                                                                            .text_color(muted_fg)
                                                                            .child("Documentation")
                                                                    );
                                                                    for part in parts.iter() {
                                                                        crumbs = crumbs
                                                                            .child(
                                                                                div()
                                                                                    .text_sm()
                                                                                    .text_color(muted_fg)
                                                                                    .child("/")
                                                                            )
                                                                            .child(
                                                                                div()
                                                                                    .text_sm()
                                                                                    .text_color(fg)
                                                                                    .child(part.clone())
                                                                            );
                                                                    }
                                                                    crumbs
                                                                })
                                                        )
                                                    })
                                                    // Markdown content
                                                    .child(
                                                        div()
                                                            .w_full()
                                                            .px_8()
                                                            .pt_6()
                                                            .pb_8()
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
                    )
            )
        )
    }
}

impl DocumentationWindow {
    fn render_tree_node(&self, node: &TreeNode, cx: &mut Context<Self>) -> AnyElement {
        match node {
            TreeNode::Crate { name, depth, .. } => {
                let is_expanded = self.is_expanded(name);
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
                        this.toggle_expansion(crate_name.clone(), cx);
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
                let is_expanded = self.is_expanded(&section_path);
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
                        this.toggle_expansion(section_path_clone.clone(), cx);
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
                let is_selected = self.current_path.as_ref() == Some(path);
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
                        this.load_content(&path_clone, cx);
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

/// Helper to create documentation window with Root wrapper
pub fn create_documentation_window(window: &mut Window, cx: &mut App) -> Entity<Root> {
    let docs = cx.new(|cx| DocumentationWindow::new(window, cx));
    cx.new(|cx| Root::new(docs.into(), window, cx))
}
