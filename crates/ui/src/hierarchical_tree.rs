use gpui::{prelude::*, *};
use crate::{ActiveTheme as _, StyledExt, IconName, Icon};

/// Renders a tree folder/crate node with expand/collapse functionality
/// 
/// # Examples
/// ```ignore
/// render_tree_folder(
///     "crate-gpui",
///     "gpui",
///     IconName::Folder,
///     Hsla { h: 45.0, s: 0.8, l: 0.5, a: 1.0 }, // Yellow folder
///     0, // depth
///     true, // is_expanded
///     |view, _event, window, cx| {
///         // Handle click - toggle expansion
///     },
///     cx,
/// )
/// ```
pub fn render_tree_folder<V: 'static>(
    id: &str,
    name: &str,
    icon: IconName,
    icon_color: Hsla,
    depth: usize,
    is_expanded: bool,
    on_click: impl Fn(&mut V, &gpui::ClickEvent, &mut Window, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> AnyElement
where
    V: Render,
{
    let indent = px(depth as f32 * 16.0);
    let item_id = SharedString::from(format!("{}", id));
    let theme = cx.theme();

    div()
        .id(item_id)
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
        .on_click(cx.listener(on_click))
        .child(
            Icon::new(icon)
                .size_4()
                .text_color(icon_color)
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.foreground)
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .child(name.to_string())
        )
        .into_any_element()
}

/// Renders a tree category/section node with item count
/// 
/// # Examples
/// ```ignore
/// render_tree_category(
///     "section-structs",
///     "Structs",
///     42, // count
///     1, // depth
///     false, // is_expanded
///     |view, _event, window, cx| {
///         // Handle click - toggle expansion
///     },
///     cx,
/// )
/// ```
pub fn render_tree_category<V: 'static>(
    id: &str,
    name: &str,
    count: usize,
    depth: usize,
    is_expanded: bool,
    on_click: impl Fn(&mut V, &gpui::ClickEvent, &mut Window, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> AnyElement
where
    V: Render,
{
    let indent = px(depth as f32 * 16.0);
    let item_id = SharedString::from(format!("{}", id));
    let theme = cx.theme();

    div()
        .id(item_id)
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
        .on_click(cx.listener(on_click))
        .child(
            Icon::new(if is_expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                .size_3p5()
                .text_color(theme.secondary_foreground)
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.foreground)
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(format!("{} ({})", name, count))
        )
        .into_any_element()
}

/// Renders a tree leaf item (selectable, no children)
/// 
/// # Examples
/// ```ignore
/// render_tree_item(
///     "item-Window",
///     "Window",
///     Hsla { h: 200.0, s: 0.7, l: 0.55, a: 1.0 }, // Blue for code
///     2, // depth
///     true, // is_selected
///     |view, _event, window, cx| {
///         // Handle click - load content
///     },
///     cx,
/// )
/// ```
pub fn render_tree_item<V: 'static>(
    id: &str,
    name: &str,
    icon_color: Hsla,
    depth: usize,
    is_selected: bool,
    on_click: impl Fn(&mut V, &gpui::ClickEvent, &mut Window, &mut Context<V>) + 'static,
    cx: &mut Context<V>,
) -> AnyElement
where
    V: Render,
{
    let indent = px(depth as f32 * 16.0);
    let item_id = SharedString::from(format!("{}", id));
    let theme = cx.theme();

    div()
        .id(item_id)
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
        .on_click(cx.listener(on_click))
        .child(
            Icon::new(IconName::Code)
                .size_3p5()
                .text_color(if is_selected {
                    theme.accent_foreground
                } else {
                    icon_color
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
                .child(name.to_string())
        )
        .into_any_element()
}

/// Standard icon colors matching level editor hierarchy
pub mod tree_colors {
    use gpui::Hsla;
    
    /// Yellow/Orange - For folders and crates
    pub const FOLDER: Hsla = Hsla { h: 45.0, s: 0.8, l: 0.5, a: 1.0 };
    
    /// Blue - For code items and engine docs
    pub const CODE_BLUE: Hsla = Hsla { h: 200.0, s: 0.7, l: 0.55, a: 1.0 };
    
    /// Purple - For project items and structs
    pub const CODE_PURPLE: Hsla = Hsla { h: 280.0, s: 0.6, l: 0.6, a: 1.0 };
    
    /// Teal/Green - For documentation files
    pub const DOC_TEAL: Hsla = Hsla { h: 160.0, s: 0.7, l: 0.45, a: 1.0 };
    
    /// Yellow - For lights and special items
    pub const SPECIAL_YELLOW: Hsla = Hsla { h: 50.0, s: 0.9, l: 0.55, a: 1.0 };
    
    /// Orange - For particle systems and effects
    pub const EFFECT_ORANGE: Hsla = Hsla { h: 30.0, s: 0.9, l: 0.55, a: 1.0 };
}
