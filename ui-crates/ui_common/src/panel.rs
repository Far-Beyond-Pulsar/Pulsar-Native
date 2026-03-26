use gpui::{App, AnyElement, EventEmitter};

/// Common lifecycle events all panels can emit
#[derive(Clone, Debug)]
pub enum PanelEvent {
    Close,
    Focus,
    TitleChanged(String),
}

/// Base trait all editor panels should implement
pub trait PanelBase: gpui::Focusable + EventEmitter<PanelEvent> {
    /// Unique identifier for this panel type
    fn panel_id() -> &'static str
    where
        Self: Sized;
    /// Display title shown in the tab/titlebar
    fn panel_title(&self, cx: &App) -> AnyElement;
    /// Called when panel visibility changes
    fn on_visibility_changed(&mut self, _visible: bool)
    where
        Self: Sized,
    {
    }
}

/// Generates the boilerplate `Focusable` + `EventEmitter<PanelEvent>` impls for a panel type.
/// The type must have a `focus_handle: FocusHandle` field.
///
/// Usage:
/// ```rust,ignore
/// panel_boilerplate!(MyPanel);
/// ```
#[macro_export]
macro_rules! panel_boilerplate {
    ($type_name:ty) => {
        impl gpui::Focusable for $type_name {
            fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
                self.focus_handle.clone()
            }
        }
        impl gpui::EventEmitter<$crate::panel::PanelEvent> for $type_name {}
    };
}
