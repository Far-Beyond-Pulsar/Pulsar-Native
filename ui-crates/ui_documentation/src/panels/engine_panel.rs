use gpui::{prelude::*, *};
use ui::{
    ActiveTheme, Sizable, StyledExt,
    h_flex, v_flex, IconName, Icon,
    text::TextView,
    resizable::{h_resizable, resizable_panel, ResizableState},
    input::TextInput,
    scroll::ScrollbarAxis,
    hierarchical_tree::{render_tree_folder, render_tree_category, render_tree_item, tree_colors},
};
use crate::engine_docs::{EngineDocsState, TreeNode};

pub struct EngineDocsPanel;

impl EngineDocsPanel {
    pub fn new() -> Self {
        Self
    }

    pub fn render<V: 'static>(
        &self,
        state: &EngineDocsState,
        sidebar_resizable: Entity<ResizableState>,
        on_toggle_expansion: impl Fn(&mut V, String, &mut Window, &mut Context<V>) + 'static + Clone,
        on_load_content: impl Fn(&mut V, String, &mut Window, &mut Context<V>) + 'static + Clone,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> impl IntoElement 
    where
        V: Render,
    {
        let breadcrumb_parts = Self::render_breadcrumbs(state);
        let markdown = state.markdown_content.clone();
        
        let theme = cx.theme().clone();

        let visible_items: Vec<_> = state.flat_visible_items.iter()
            .map(|&idx| state.tree_items[idx].clone())
            .collect();
        
        let tree_nodes: Vec<AnyElement> = visible_items.iter().map(|node| {
            Self::render_tree_node(node, state, on_toggle_expansion.clone(), on_load_content.clone(), cx)
        }).collect();

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
            .bg(theme.sidebar.opacity(0.95))
            .border_r_1()
            .border_color(theme.border)
            .child(
                // Professional header with badge
                h_flex()
                    .w_full()
                    .h(px(48.0))
                    .px_4()
                    .items_center()
                    .justify_between()
                    .bg(theme.sidebar)
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Icon::new(IconName::Code)
                                    .size_4()
                                    .text_color(tree_colors::CODE_BLUE)
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_color(theme.foreground)
                                    .child("API Reference")
                            )
                    )
                    .child(
                        div()
                            .px_2()
                            .py(px(3.0))
                            .rounded(px(6.0))
                            .bg(theme.accent.opacity(0.12))
                            .text_xs()
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .text_color(theme.accent)
                            .child(format!("{}", tree_nodes.len()))
                    )
            )
            .child(
                // Search bar
                div()
                    .w_full()
                    .px_3()
                    .py_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        TextInput::new(&state.search_input_state)
                            .w_full()
                            .prefix(
                                Icon::new(IconName::Search)
                                    .size_4()
                                    .text_color(theme.secondary_foreground)
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
                                .text_color(theme.secondary_foreground)
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

    fn render_tree_node<V: 'static>(
        node: &TreeNode,
        state: &EngineDocsState,
        on_toggle_expansion: impl Fn(&mut V, String, &mut Window, &mut Context<V>) + 'static + Clone,
        on_load_content: impl Fn(&mut V, String, &mut Window, &mut Context<V>) + 'static + Clone,
        cx: &mut Context<V>,
    ) -> AnyElement 
    where
        V: Render,
    {
        match node {
            TreeNode::Crate { name, depth, .. } => {
                let is_expanded = state.expanded_paths.contains(name);
                let crate_name = name.clone();
                
                render_tree_folder(
                    &format!("crate-{}", name),
                    name,
                    if is_expanded { IconName::FolderOpen } else { IconName::Folder },
                    tree_colors::FOLDER,
                    *depth,
                    is_expanded,
                    move |view, _event, window, cx| {
                        on_toggle_expansion(view, crate_name.clone(), window, cx);
                    },
                    cx,
                )
            }
            TreeNode::Section { crate_name, section_name, count, depth } => {
                let section_path = format!("{}/{}", crate_name, section_name);
                let is_expanded = state.expanded_paths.contains(&section_path);
                let path_for_click = section_path.clone();

                render_tree_category(
                    &format!("section-{}-{}", crate_name, section_name),
                    section_name,
                    *count,
                    *depth,
                    is_expanded,
                    move |view, _event, window, cx| {
                        on_toggle_expansion(view, path_for_click.clone(), window, cx);
                    },
                    cx,
                )
            }
            TreeNode::Item { item_name, path, depth, .. } => {
                let is_selected = state.current_path.as_ref() == Some(&path.to_string());
                let path_for_click = path.to_string();

                render_tree_item(
                    &format!("item-{}", path.replace('/', "-")),
                    item_name,
                    tree_colors::CODE_BLUE,
                    *depth,
                    is_selected,
                    move |view, _event, window, cx| {
                        on_load_content(view, path_for_click.clone(), window, cx);
                    },
                    cx,
                )
            }
        }
    }
}
