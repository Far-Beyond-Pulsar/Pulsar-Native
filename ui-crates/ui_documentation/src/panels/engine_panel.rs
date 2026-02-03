use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, Sizable, StyledExt,
    h_flex, v_flex, IconName, Icon,
    text::TextView,
    resizable::{h_resizable, resizable_panel, ResizableState},
    input::TextInput,
    scroll::ScrollbarAxis,
};
use crate::engine_docs::{EngineDocsState, TreeNode};

pub struct EngineDocsPanel;

impl EngineDocsPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn render(
        &self,
        state: &EngineDocsState,
        sidebar_resizable: Entity<ResizableState>,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let breadcrumb_parts = Self::render_breadcrumbs(state);
        
        let visible_items: Vec<_> = state.flat_visible_items.iter()
            .map(|&idx| state.tree_items[idx].clone())
            .collect();
        let markdown = state.markdown_content.clone();
        
        let tree_nodes: Vec<_> = visible_items.iter().map(|node| {
            Self::render_tree_node(node, state, cx)
        }).collect();
        
        let theme = cx.theme().clone();

        h_resizable("docs-horizontal", sidebar_resizable)
            .child(
                resizable_panel()
                    .size(px(280.0))
                    .child(Self::render_sidebar(state, tree_nodes, &theme))
            )
            .child(
                resizable_panel()
                    .child(Self::render_content(breadcrumb_parts, markdown, window, cx, &theme))
            )
    }

    fn render_sidebar(
        state: &EngineDocsState,
        tree_nodes: Vec<AnyElement>,
        theme: &ui::ThemeColor,
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
                // Compact sidebar header
                h_flex()
                    .w_full()
                    .h(px(44.0))
                    .px_4()
                    .items_center()
                    .justify_between()
                    .bg(theme.sidebar.opacity(0.8))
                    .border_b_1()
                    .border_color(theme.border)
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
                // Search bar
                div()
                    .w_full()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        TextInput::new(&state.search_input_state)
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
                // Tree items with scroll
                div()
                    .flex_1()
                    .overflow_hidden()
                    .child(
                        v_flex()
                            .size_full()
                            .p_2()
                            .gap_px()
                            .font_family("monospace")
                            .scrollable(ScrollbarAxis::Vertical)
                            .children(tree_nodes)
                    )
            )
    }

    fn render_content(
        breadcrumb_parts: Option<Vec<String>>,
        markdown: String,
        window: &mut Window,
        cx: &mut App,
        theme: &ui::ThemeColor,
    ) -> impl IntoElement {
        div()
            .size_full()
            .bg(theme.background)
            .child(
                v_flex()
                    .size_full()
                    .when(breadcrumb_parts.is_some(), |this| {
                        this.child(Self::render_breadcrumb_bar(breadcrumb_parts.unwrap(), theme))
                    })
                    .child(
                        div()
                            .flex_1()
                            .overflow_hidden()
                            .child(
                                div()
                                    .size_full()
                                    .scrollable(ScrollbarAxis::Vertical)
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
    }

    fn render_breadcrumb_bar(parts: Vec<String>, theme: &ui::ThemeColor) -> impl IntoElement {
        h_flex()
            .w_full()
            .h(px(44.0))
            .px_6()
            .items_center()
            .gap_2()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.sidebar.opacity(0.3))
            .child({
                let mut crumbs = h_flex().gap_2().items_center();
                crumbs = crumbs.child(
                    Icon::new(IconName::BookOpen)
                        .size_4()
                        .text_color(theme.accent)
                );
                for (idx, part) in parts.iter().enumerate() {
                    if idx > 0 {
                        crumbs = crumbs.child(
                            Icon::new(IconName::ChevronRight)
                                .size_3()
                                .text_color(theme.muted_foreground)
                        );
                    }
                    crumbs = crumbs.child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(if idx == parts.len() - 1 {
                                theme.foreground
                            } else {
                                theme.muted_foreground
                            })
                            .child(part.clone())
                    );
                }
                crumbs
            })
    }

    fn render_breadcrumbs(state: &EngineDocsState) -> Option<Vec<String>> {
        let path = state.current_path.as_ref()?;
        let parts: Vec<String> = path.split('/').map(|s| s.to_string()).collect();
        if parts.is_empty() {
            return None;
        }
        Some(parts)
    }

    fn render_tree_node(node: &TreeNode, state: &EngineDocsState, cx: &mut App) -> AnyElement {
        match node {
            TreeNode::Crate { name, depth, .. } => {
                Self::render_crate_node(name, *depth, state, cx)
            }
            TreeNode::Section { crate_name, section_name, count, depth } => {
                Self::render_section_node(crate_name, section_name, *count, *depth, state, cx)
            }
            TreeNode::Item { item_name, path, depth, .. } => {
                Self::render_item_node(item_name, path, *depth, state, cx)
            }
        }
    }

    fn render_crate_node(name: &str, depth: usize, state: &EngineDocsState, cx: &mut App) -> AnyElement {
        let is_expanded = state.expanded_paths.contains(name);
        let indent = px(depth as f32 * 16.0);
        let id = SharedString::from(format!("crate-{}", name));
        let theme = cx.theme();

        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .h(px(32.0))
            .pl(indent + px(12.0))
            .pr_3()
            .mx_2()
            .rounded(px(6.0))
            .hover(|style| style.bg(theme.accent.opacity(0.1)))
            .cursor_pointer()
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
                    .child(name.to_string())
            )
            .into_any_element()
    }

    fn render_section_node(
        crate_name: &str,
        section_name: &str,
        count: usize,
        depth: usize,
        state: &EngineDocsState,
        cx: &mut App,
    ) -> AnyElement {
        let section_path = format!("{}/{}", crate_name, section_name);
        let is_expanded = state.expanded_paths.contains(&section_path);
        let indent = px(depth as f32 * 16.0);
        let id = SharedString::from(format!("section-{}-{}", crate_name, section_name));
        let theme = cx.theme();

        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .h(px(32.0))
            .pl(indent + px(12.0))
            .pr_3()
            .mx_2()
            .rounded(px(6.0))
            .hover(|style| style.bg(theme.accent.opacity(0.1)))
            .cursor_pointer()
            .child(
                Icon::new(if is_expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                    .size_3p5()
                    .text_color(theme.muted_foreground)
            )
            .child(
                div()
                    .text_sm()
                    .text_color(theme.foreground)
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("{} ({})", section_name, count))
            )
            .into_any_element()
    }

    fn render_item_node(
        item_name: &str,
        path: &str,
        depth: usize,
        state: &EngineDocsState,
        cx: &mut App,
    ) -> AnyElement {
        let is_selected = state.current_path.as_ref() == Some(&path.to_string());
        let indent = px(depth as f32 * 16.0);
        let id = SharedString::from(format!("item-{}", path.replace('/', "-")));
        let theme = cx.theme();

        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .h(px(32.0))
            .pl(indent + px(20.0))
            .pr_3()
            .mx_2()
            .rounded(px(6.0))
            .when(is_selected, |style| {
                style
                    .bg(theme.accent)
                    .shadow_sm()
            })
            .when(!is_selected, |style| {
                style.hover(|style| style.bg(theme.accent.opacity(0.1)))
            })
            .cursor_pointer()
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
                        theme.foreground
                    })
                    .child(item_name.to_string())
            )
            .into_any_element()
    }
}
