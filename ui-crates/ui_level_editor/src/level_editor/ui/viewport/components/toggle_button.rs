//! Reusable toggle button component.
//!
//! This component eliminates the repetitive boilerplate for creating toggle buttons
//! with icons, tooltips, and state management.

use gpui::*;
use ui::{button::Button, IconName, Selectable};

/// A reusable toggle button that manages state through a callback.
///
/// The `on_toggle` callback receives `&mut App` so callers can use
/// GPUI entity updates without holding any locks.
pub struct ToggleButton {
    id: SharedString,
    icon: IconName,
    tooltip: Option<SharedString>,
    selected: bool,
    on_toggle: Option<Box<dyn Fn(&mut App) + 'static>>,
}

impl ToggleButton {
    /// Create a new toggle button with the given ID.
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            icon: IconName::Circle,
            tooltip: None,
            selected: false,
            on_toggle: None,
        }
    }

    /// Set the icon for this button.
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = icon;
        self
    }

    /// Set the tooltip text.
    pub fn tooltip(mut self, tooltip: impl Into<SharedString>) -> Self {
        self.tooltip = Some(tooltip.into());
        self
    }

    /// Set whether this button is selected/active.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// Set the callback to invoke when the button is toggled.
    pub fn on_toggle<F>(mut self, f: F) -> Self
    where
        F: Fn(&mut App) + 'static,
    {
        self.on_toggle = Some(Box::new(f));
        self
    }

    /// Build the button element.
    pub fn build(self) -> Button {
        let mut button = Button::new(self.id).icon(self.icon).selected(self.selected);

        if let Some(tooltip) = self.tooltip {
            button = button.tooltip(tooltip);
        }

        if let Some(on_toggle) = self.on_toggle {
            button = button.on_click(move |_, _, cx| {
                on_toggle(cx);
            });
        }

        button
    }
}

/// Helper function to create a toggle button with state management.
///
/// This is a convenience function that reduces boilerplate for the common pattern
/// of toggling a boolean value in shared state.
///
/// # Example
/// ```rust
/// create_state_toggle(
///     "toggle_grid",
///     IconName::LayoutDashboard,
///     "Toggle Grid",
///     state.show_grid,
///     state_arc.clone(),
///     |s| s.toggle_grid()
/// )
/// ```
pub fn create_state_toggle<S>(
    id: impl Into<SharedString>,
    icon: IconName,
    tooltip: impl Into<SharedString>,
    selected: bool,
    state: Entity<S>,
    toggle_fn: impl Fn(&mut S) + 'static,
) -> Button
where
    S: 'static,
{
    ToggleButton::new(id)
        .icon(icon)
        .tooltip(tooltip)
        .selected(selected)
        .on_toggle(move |cx| {
            state.update(cx, |s, cx| { toggle_fn(s); cx.notify(); });
        })
        .build()
}
