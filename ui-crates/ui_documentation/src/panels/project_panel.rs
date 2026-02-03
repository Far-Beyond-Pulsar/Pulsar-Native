use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, Sizable, StyledExt,
    h_flex, v_flex, IconName, Icon,
    text::TextView,
    resizable::{h_resizable, resizable_panel, ResizableState},
    input::TextInput,
    scroll::ScrollbarAxis,
};
use crate::project_docs::{ProjectDocsState, ProjectTreeNode};

pub struct ProjectDocsPanel;

impl ProjectDocsPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn render(
        &self,
        state: &ProjectDocsState,
        sidebar_resizable: Entity<ResizableState>,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let markdown = state.markdown_content.clone();

        let visible_items: Vec<_> = state.flat_visible_items.iter()
            .map(|&idx| state.tree_items[idx].clone())
            .collect();

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
                    .child(Self::render_content(markdown, window, cx, &theme))
            )
    }

    fn render_sidebar(
        state: &ProjectDocsState,
        tree_nodes: Vec<AnyElement>,
        theme: &ui::ThemeColor,
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.border)
            .child(
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
        markdown: String,
        window: &mut Window,
        cx: &mut App,
        theme: &ui::ThemeColor,
    ) -> impl IntoElement {
        div()
            .size_full()
            .bg(theme.background)
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
                                    "project-docs-markdown",
                                    markdown,
                                    window,
                                    cx,
                                )
                                .selectable()
                            )
                    )
            )
    }

    fn render_tree_node(node: &ProjectTreeNode, state: &ProjectDocsState, cx: &mut App) -> AnyElement {
        match node {
            ProjectTreeNode::Category { name, count, depth } => {
                Self::render_category_node(name, *count, *depth, state, cx)
            }
            ProjectTreeNode::Item { item_name, path, depth, .. } => {
                Self::render_item_node(item_name, path, *depth, state, cx)
            }
        }
    }

    fn render_category_node(
        name: &str,
        count: usize,
        depth: usize,
        state: &ProjectDocsState,
        cx: &mut App,
    ) -> AnyElement {
        let is_expanded = state.expanded_paths.contains(name);
        let indent = px(depth as f32 * 16.0);
        let id = SharedString::from(format!("category-{}", name));
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
                    .child(format!("{} ({})", name, count))
            )
            .into_any_element()
    }

    fn render_item_node(
        item_name: &str,
        path: &str,
        depth: usize,
        state: &ProjectDocsState,
        cx: &mut App,
    ) -> AnyElement {
        let is_selected = state.current_path.as_ref() == Some(&path.to_string());
        let indent = px(depth as f32 * 16.0);
        let id = SharedString::from(format!("project-item-{}", path.replace("::", "-")));
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
