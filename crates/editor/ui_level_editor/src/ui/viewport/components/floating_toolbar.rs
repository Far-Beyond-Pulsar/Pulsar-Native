//! Floating toolbar with a drag handle.
//!
//! A toolbar card for the viewport's Panel Canvas: a grip strip on one side
//! (the canvas's shared [`DragHandle`]) and the toolbar's content.

use gpui::{prelude::FluentBuilder as _, *};
use ui::panel_canvas::{DragHandle, PanelRef};
use ui::{h_flex, ActiveTheme};

#[derive(Clone, Copy)]
pub enum HandleSide {
    Left,
    Right,
}

/// The grip strip that moves the toolbar's card on its canvas.
pub fn create_drag_handle<V>(panel: PanelRef, side: HandleSide, cx: &Context<V>) -> impl IntoElement
where
    V: Render + 'static,
{
    let background = cx.theme().background;
    DragHandle::new(panel)
        .w(px(12.0))
        .h_full()
        .bg(background.opacity(0.9))
        .when(matches!(side, HandleSide::Left), |el| {
            el.rounded_l(cx.theme().radius)
        })
        .when(matches!(side, HandleSide::Right), |el| {
            el.rounded_r(cx.theme().radius)
        })
        .border_1()
        .border_color(cx.theme().border)
        .hover(move |style| style.bg(background))
}

/// Create a toolbar with drag handle and content.
pub fn toolbar_with_drag_handle<V>(
    drag_handle_content: impl IntoElement,
    toolbar_content: impl IntoElement,
    side: HandleSide,
    cx: &Context<V>,
) -> impl IntoElement
where
    V: 'static + Render,
{
    let content = h_flex()
        .h_full()
        .gap_2()
        .p_1()
        .bg(cx.theme().background.opacity(0.9))
        .when(matches!(side, HandleSide::Left), |el| {
            el.rounded_r(cx.theme().radius).border_r_1()
        })
        .when(matches!(side, HandleSide::Right), |el| {
            el.rounded_l(cx.theme().radius).border_l_1()
        })
        .border_y_1()
        .border_color(cx.theme().border)
        .items_center()
        .child(toolbar_content);

    // Match the actual child order to the anchored edge; do not depend on
    // reversed flex layout to move the interactive handle.
    // Set the painted bar's height here, not only on an outer wrapper whose
    // centered children can still shrink to their individual control heights.
    let toolbar = h_flex().gap_0().h(px(42.0)).items_stretch();
    match side {
        HandleSide::Left => toolbar.child(drag_handle_content).child(content),
        HandleSide::Right => toolbar.child(content).child(drag_handle_content),
    }
}

/// A small grip in a card's top-right corner, for overlays whose content is
/// a view of its own (no room for a handle in it).
pub fn corner_grip<V>(panel: PanelRef, cx: &Context<V>) -> impl IntoElement
where
    V: 'static,
{
    DragHandle::new(panel)
        .absolute()
        .top(px(4.0))
        .right(px(4.0))
        .size(px(14.0))
        .rounded(px(3.0))
        .bg(cx.theme().background.opacity(0.6))
        .hover(|style| style.bg(cx.theme().secondary))
}
