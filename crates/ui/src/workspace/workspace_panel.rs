//! Workspace Panel - A helper for creating custom dockable panels
//!
//! This provides a simple way to create panels with custom content.

use crate::dock::{Panel, PanelEvent, PanelState};
use gpui::*;
use std::rc::Rc;

/// A builder for creating custom workspace panels
pub struct WorkspacePanel {
    id: SharedString,
    title: SharedString,
    closable: bool,
    focus_handle: FocusHandle,
    render_fn: Rc<dyn Fn(&mut Window, &mut Context<Self>) -> AnyElement>,
}

impl WorkspacePanel {
    /// Create a new workspace panel (call within cx.new())
    pub fn new(
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        render_fn: impl Fn(&mut Window, &mut Context<Self>) -> AnyElement + 'static,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            closable: true,
            focus_handle: cx.focus_handle(),
            render_fn: Rc::new(render_fn),
        }
    }

    /// Set whether this panel can be closed
    pub fn closable(mut self, closable: bool) -> Self {
        self.closable = closable;
        self
    }
}

impl Panel for WorkspacePanel {
    fn panel_name(&self) -> &'static str {
        // Leak the string so it has 'static lifetime
        Box::leak(self.id.to_string().into_boxed_str())
    }

    fn title(&self, _window: &Window, _cx: &App) -> AnyElement {
        self.title.clone().into_any_element()
    }

    fn closable(&self, _cx: &App) -> bool {
        self.closable
    }

    fn dump(&self, _cx: &App) -> PanelState {
        PanelState::new(self)
    }
}

impl Focusable for WorkspacePanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for WorkspacePanel {}

impl Render for WorkspacePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        (self.render_fn)(window, cx)
    }
}
