//! Shared tree item rendering logic for hierarchical views
//!
//! This module provides reusable components for rendering tree items with:
//! - Expand/collapse arrows
//! - Drag-and-drop support
//! - Recursive children rendering
//! - Modifier key operations (nest, reorder, un-nest)

use gpui::{prelude::*, *};
use std::sync::Arc;
use ui::{
    draggable::{DragHandlePosition, Draggable},
    drop_area::DropArea,
    h_flex, v_flex, ActiveTheme, Icon, IconName, Sizable, StyledExt,
};

/// Generic tree item configuration
pub struct TreeItemConfig<T: Clone + Render + 'static> {
    /// Unique ID for this item
    pub id: String,

    /// Item display name
    pub name: String,

    /// Icon to display
    pub icon: IconName,

    /// Icon color
    pub icon_color: Hsla,

    /// Whether this item is selected
    pub is_selected: bool,

    /// Whether this item has children
    pub has_children: bool,

    /// Whether this item is expanded
    pub is_expanded: bool,

    /// Tree depth for indentation
    pub depth: usize,

    /// Drag payload
    pub drag_payload: T,

    /// Child elements to render (if expanded)
    pub children: Vec<AnyElement>,

    /// Callback when expand/collapse is toggled
    pub on_toggle_expand: Arc<dyn Fn()>,

    /// Callback when item is clicked
    pub on_click: Arc<dyn Fn()>,

    /// Callback when dropped onto
    pub on_drop: Arc<dyn Fn(T, &Modifiers)>,

    /// Whether this item can accept drops
    pub can_accept_drop: Arc<dyn Fn(&T) -> bool>,

    /// Optional extra content to render at the end of the row
    pub extra_content: Option<AnyElement>,
}

/// Renders a tree item with expand/collapse, drag-drop, and children
pub fn render_tree_item<T, V>(config: TreeItemConfig<T>, cx: &mut Context<V>) -> impl IntoElement
where
    T: Clone + Render + 'static,
    V: 'static + Render,
{
    let indent = px(config.depth as f32 * 20.0 + 4.0);

    // Text colors based on selection state
    let text_color = if config.is_selected {
        cx.theme().accent_foreground
    } else {
        cx.theme().foreground
    };

    let muted_color = if config.is_selected {
        cx.theme().accent_foreground.opacity(0.7)
    } else {
        cx.theme().muted_foreground
    };

    // Expand/collapse arrow
    let expand_arrow: AnyElement = if config.has_children {
        let on_toggle = config.on_toggle_expand.clone();
        div()
            .w_4()
            .h_4()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(2.0))
            .cursor_pointer()
            .hover(|s| s.bg(cx.theme().muted.opacity(0.5)))
            .child(
                Icon::new(if config.is_expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .size(px(12.0))
                .text_color(muted_color),
            )
            .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                cx.stop_propagation();
                (on_toggle)();
            })
            .into_any_element()
    } else {
        div().w_4().into_any_element()
    };

    // Row content
    let on_click = config.on_click.clone();
    let row_content =
        h_flex()
            .id(SharedString::from(format!("tree-item-{}", config.id)))
            .w_full()
            .items_center()
            .gap_1()
            .h_7()
            .pl(indent)
            .pr_2()
            .rounded(px(4.0))
            .cursor_pointer()
            .when(config.is_selected, |s| s.bg(cx.theme().accent).shadow_sm())
            .when(!config.is_selected, |s| {
                s.hover(|style| style.bg(cx.theme().muted.opacity(0.3)))
            })
            .on_click(cx.listener(move |_view, _event, _window, cx| {
                (on_click)();
                cx.notify();
            }))
            .child(expand_arrow)
            // Type icon
            .child(
                div()
                    .w_5()
                    .h_5()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.0))
                    .bg(config.icon_color.opacity(0.15))
                    .child(Icon::new(config.icon).size(px(14.0)).text_color(
                        if config.is_selected {
                            text_color
                        } else {
                            config.icon_color
                        },
                    )),
            )
            // Name
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(text_color)
                    .overflow_hidden()
                    .text_ellipsis()
                    .child(config.name.clone()),
            )
            .children(config.extra_content);

    // Drag source wrapper
    let draggable_row = Draggable::new(
        format!("tree-drag-{}", config.id),
        config.drag_payload.clone(),
    )
    .drag_handle(DragHandlePosition::Left)
    .w_full()
    .child(row_content);

    // Drop target wrapper
    let can_accept = config.can_accept_drop.clone();
    let on_drop_callback = config.on_drop.clone();
    let drop_row = DropArea::<T>::new(format!("tree-drop-{}", config.id))
        .can_accept(move |payload| (can_accept)(payload))
        .on_drop(move |payload, window, _cx| {
            let modifiers = window.modifiers();
            (on_drop_callback)(payload.clone(), &modifiers);
        })
        .w_full()
        .child(draggable_row);

    // Compose: drop zone + children
    v_flex().w_full().child(drop_row).children(config.children)
}
