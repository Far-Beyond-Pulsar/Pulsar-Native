use gpui::{
    div, AnyView, App, Entity, IntoElement, ParentElement, RenderOnce, SharedString, Styled,
    Window,
};

use crate::{v_flex, ActiveTheme as _, TitleBar};

/// A reusable window wrapper for drawer windows.
///
/// This component provides a consistent layout with:
/// - Full size container with theme background
/// - Title bar at the top
/// - Content area below that takes remaining space
///
/// # Example
///
/// ```ignore
/// fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
///     drawer_window("Window.Title.FileManager", self.drawer.clone(), cx)
/// }
/// ```
pub fn drawer_window(
    title_key: impl Into<SharedString>,
    content: impl IntoElement,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let title = crate::translate(&title_key.into());

    v_flex()
        .size_full()
        .bg(theme.background)
        .child(TitleBar::new().child(title))
        .child(div().flex_1().overflow_hidden().child(content))
}

/// A reusable window wrapper for entity-based drawer windows.
///
/// This is a convenience wrapper for drawer windows that contain a single entity.
pub fn drawer_window_entity<T: 'static + gpui::Render>(
    title_key: impl Into<SharedString>,
    entity: Entity<T>,
    cx: &App,
) -> impl IntoElement {
    drawer_window(title_key, entity, cx)
}

/// A reusable window wrapper for view-based drawer windows.
pub fn drawer_window_view(
    title_key: impl Into<SharedString>,
    view: AnyView,
    cx: &App,
) -> impl IntoElement {
    drawer_window(title_key, view, cx)
}
