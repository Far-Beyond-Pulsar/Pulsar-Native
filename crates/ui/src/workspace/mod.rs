//! Modular Workspace System
//!
//! A complete, reusable docking workspace similar to VS Code, Unreal Engine, etc.
//! Features:
//! - Fully draggable tabs between any panel
//! - Dynamic panel creation/removal
//! - Panels auto-collapse when empty
//! - State persistence
//! - Customizable panel content
//!
//! ## Example
//! ```ignore
//! // Create a workspace
//! let workspace = Workspace::new("my-workspace", window, cx);
//!
//! // Add panels
//! workspace.add_panel(MyPanel::new(), DockPlacement::Left, window, cx);
//! workspace.add_panel(AnotherPanel::new(), DockPlacement::Right, window, cx);
//!
//! // Panels can now be dragged between docks!
//! ```

mod workspace_panel;

pub use workspace_panel::WorkspacePanel;

use crate::dock::{DockArea, DockChannel, DockItem, DockPlacement, Panel, PanelView};
use gpui::*;
use std::sync::Arc;

/// A complete modular workspace with docking support
pub struct Workspace {
    dock_area: Entity<DockArea>,
}

impl Workspace {
    /// Create a new workspace
    pub fn new(id: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let dock_area = cx.new(|cx| DockArea::new(id, None, window, cx));

        Self { dock_area }
    }

    /// Create a new workspace with a specific dock channel
    pub fn new_with_channel(
        id: impl Into<SharedString>,
        channel: DockChannel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let dock_area = cx.new(|cx| DockArea::new_with_channel(id, None, channel, window, cx));

        Self { dock_area }
    }

    /// Add a panel to the workspace at the specified placement
    pub fn add_panel<P: Panel + 'static>(
        &mut self,
        panel: Entity<P>,
        placement: DockPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dock_area.update(cx, |dock_area, cx| {
            dock_area.add_panel(Arc::new(panel), placement, None, window, cx);
        });
    }

    /// Remove a panel from the workspace
    pub fn remove_panel<P: Panel + 'static>(
        &mut self,
        panel: Entity<P>,
        placement: DockPlacement,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dock_area.update(cx, |dock_area, cx| {
            dock_area.remove_panel(Arc::new(panel), placement, window, cx);
        });
    }

    /// Initialize the workspace with a center panel and optional side panels
    pub fn initialize(
        &mut self,
        center: DockItem,
        left: Option<DockItem>,
        right: Option<DockItem>,
        bottom: Option<DockItem>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.dock_area.update(cx, |dock_area, cx| {
            // Set center
            dock_area.set_center(center, window, cx);

            // Set docks if provided
            if let Some(left_item) = left {
                dock_area.set_left_dock(left_item, None, true, window, cx);
            }

            if let Some(right_item) = right {
                dock_area.set_right_dock(right_item, None, true, window, cx);
            }

            if let Some(bottom_item) = bottom {
                dock_area.set_bottom_dock(bottom_item, None, true, window, cx);
            }
        });
    }

    /// Get the dock area entity for advanced operations
    pub fn dock_area(&self) -> &Entity<DockArea> {
        &self.dock_area
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.dock_area.clone()
    }
}
