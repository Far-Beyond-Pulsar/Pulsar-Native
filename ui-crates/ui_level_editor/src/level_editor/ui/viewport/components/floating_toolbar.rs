//! Floating toolbar component with drag handle.
//!
//! This component provides a reusable floating toolbar that can be dragged
//! and positioned anywhere in the viewport. It includes a drag handle with
//! grip dots and supports custom content through children.

use std::sync::Arc;

use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{h_flex, v_flex, ActiveTheme, StyledExt};

/// Builder for creating a floating toolbar with drag functionality.
///
/// # Example
/// ```rust
/// FloatingToolbar::new()
///     .position(100.0, 50.0)
///     .on_drag(|dx, dy| {
///         // Handle drag
///     })
///     .child(Button::new("my_button").icon(IconName::Grid))
///     .build(cx)
/// ```
pub struct FloatingToolbar<E: IntoElement> {
    position: (f32, f32),
    children: Vec<E>,
    on_drag_start: Option<Box<dyn Fn(f32, f32)>>,
    on_drag: Option<Box<dyn Fn(f32, f32)>>,
    on_drag_end: Option<Box<dyn Fn()>>,
}

impl<E: IntoElement> FloatingToolbar<E> {
    /// Create a new floating toolbar.
    pub fn new() -> Self {
        Self {
            position: (0.0, 0.0),
            children: Vec::new(),
            on_drag_start: None,
            on_drag: None,
            on_drag_end: None,
        }
    }

    /// Set the toolbar position (x, y).
    pub fn position(mut self, x: f32, y: f32) -> Self {
        self.position = (x, y);
        self
    }

    /// Add a child element to the toolbar.
    pub fn child(mut self, child: E) -> Self {
        self.children.push(child);
        self
    }

    /// Set the drag start callback.
    pub fn on_drag_start<F>(mut self, f: F) -> Self
    where
        F: Fn(f32, f32) + 'static,
    {
        self.on_drag_start = Some(Box::new(f));
        self
    }

    /// Set the drag callback (called continuously during drag).
    pub fn on_drag<F>(mut self, f: F) -> Self
    where
        F: Fn(f32, f32) + 'static,
    {
        self.on_drag = Some(Box::new(f));
        self
    }

    /// Set the drag end callback.
    pub fn on_drag_end<F>(mut self, f: F) -> Self
    where
        F: Fn() + 'static,
    {
        self.on_drag_end = Some(Box::new(f));
        self
    }
}

/// Create a drag handle with mouse event handlers.
///
/// # Arguments
/// * `state_arc` - The state to update on drag
/// * `drag_start_field` - Function to set the drag start coordinates
/// * `is_dragging_field` - Function to set the dragging flag
/// * `cx` - The window context
pub fn create_drag_handle<V, S>(
    state_arc: Arc<parking_lot::RwLock<S>>,
    drag_start_field: impl Fn(&mut S, Option<(f32, f32)>) + Clone + 'static,
    is_dragging_field: impl Fn(&mut S, bool) + Clone + 'static,
    cx: &Context<V>,
) -> impl IntoElement
where
    V: Render + 'static,
    S: 'static,
{
    div()
        .relative()
        .w(px(12.0))
        .h_full()
        .flex_shrink_0()
        .bg(cx.theme().background.opacity(0.9))
        .rounded_l(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .cursor(CursorStyle::PointingHand)
        .hover(|style| style.bg(cx.theme().background))
        .on_mouse_down(MouseButton::Left, {
            let state = state_arc.clone();
            let drag_start = drag_start_field.clone();
            let is_dragging = is_dragging_field.clone();
            move |event: &MouseDownEvent, _window, _cx| {
                let mut s = state.write();
                let x: f32 = event.position.x.into();
                let y: f32 = event.position.y.into();
                drag_start(&mut s, Some((x, y)));
                is_dragging(&mut s, true);
            }
        })
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_0p5()
                .child(div().w(px(2.0)).h(px(2.0)).rounded_full().bg(white()))
                .child(div().w(px(2.0)).h(px(2.0)).rounded_full().bg(white()))
                .child(div().w(px(2.0)).h(px(2.0)).rounded_full().bg(white())),
        )
}

/// Create a simple floating toolbar with standard styling.
///
/// This is a convenience function for the common case of a toolbar with
/// children elements and standard styling.
///
/// # Arguments
/// * `children` - The content elements to display in the toolbar
/// * `cx` - The window context
pub fn simple_floating_toolbar<V: 'static>(
    children: impl IntoElement,
    cx: &Context<V>,
) -> impl IntoElement
where
    V: Render,
{
    h_flex()
        .gap_2()
        .p_2()
        .bg(cx.theme().background.opacity(0.9))
        .rounded(cx.theme().radius)
        .border_1()
        .border_color(cx.theme().border)
        .items_center()
        .child(children)
}

/// Create a toolbar with drag handle and content.
///
/// # Arguments
/// * `drag_handle_content` - The drag handle element
/// * `toolbar_content` - The main toolbar content
/// * `cx` - The window context
pub fn toolbar_with_drag_handle<V: 'static>(
    drag_handle_content: impl IntoElement,
    toolbar_content: impl IntoElement,
    cx: &Context<V>,
) -> impl IntoElement
where
    V: Render,
{
    h_flex()
        .gap_0()
        .child(drag_handle_content)
        .child(
            h_flex()
                .h_full()
                .gap_2()
                .p_1()
                .bg(cx.theme().background.opacity(0.9))
                .rounded_r(cx.theme().radius)
                .border_y_1()
                .border_r_1()
                .border_color(cx.theme().border)
                .items_center()
                .child(toolbar_content),
        )
}
