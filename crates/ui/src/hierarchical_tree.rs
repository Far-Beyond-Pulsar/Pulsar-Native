use gpui::{prelude::*, *};
use crate::{ActiveTheme as _, StyledExt, IconName, Icon, v_flex};

/// Entry in a hierarchical tree
#[derive(Clone, Debug)]
pub struct TreeEntry {
    pub id: String,
    pub name: String,
    pub is_parent: bool,
    pub is_expanded: bool,
    pub depth: usize,
    pub icon: IconName,
}

/// Configuration for the hierarchical tree
#[derive(Clone)]
pub struct HierarchicalTreeConfig {
    pub title: String,
    pub footer: Option<String>,
    pub item_height: Pixels,
    pub show_header: bool,
    pub show_footer: bool,
}

impl Default for HierarchicalTreeConfig {
    fn default() -> Self {
        Self {
            title: "Explorer".to_string(),
            footer: None,
            item_height: px(28.0),
            show_header: true,
            show_footer: true,
        }
    }
}

/// Helper to render a single tree item
pub fn render_tree_item(
    entry: &TreeEntry,
    idx: usize,
    is_selected: bool,
    item_height: Pixels,
    cx: &App,
) -> impl IntoElement {
    let indent = px(entry.depth as f32 * 16.0);
    let item_id = SharedString::from(format!("tree-item-{}", idx));
    
    div()
        .id(item_id)
        .flex()
        .items_center()
        .gap_2()
        .h(item_height)
        .pl(indent + px(12.0))
        .pr_3()
        .rounded_md()
        .when(is_selected, |style| style.bg(cx.theme().accent))
        .when(!is_selected, |style| {
            style.hover(|style| style.bg(cx.theme().accent.opacity(0.1)))
        })
        .cursor_pointer()
        .child(
            Icon::new(entry.icon.clone())
                .size_4()
                .when(is_selected, |icon| icon.text_color(cx.theme().accent_foreground))
                .when(!is_selected, |icon| icon.text_color(cx.theme().foreground))
        )
        .child(
            div()
                .text_sm()
                .when(is_selected, |style| style.text_color(cx.theme().accent_foreground))
                .when(!is_selected, |style| style.text_color(cx.theme().foreground))
                .child(entry.name.clone())
        )
}

/// Helper to render a hierarchical tree container
pub fn hierarchical_tree_container(
    config: HierarchicalTreeConfig,
    cx: &App,
) -> Div {
    v_flex()
        .size_full()
        .flex()
        .flex_col()
        .font_family("monospace")
        .font(gpui::Font {
            family: "Jetbrains Mono".to_string().into(),
            weight: gpui::FontWeight::NORMAL,
            style: gpui::FontStyle::Normal,
            features: gpui::FontFeatures::default(),
            fallbacks: Some(gpui::FontFallbacks::from_fonts(vec!["monospace".to_string()])),
        })
        .when(config.show_header, |el| {
            el.child(
                // Header
                div()
                    .w_full()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child(config.title.clone())
                    )
            )
        })
}
