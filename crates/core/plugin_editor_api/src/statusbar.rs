use serde::{Deserialize, Serialize};
use std::fmt;

// ============================================================================
// Statusbar Button System
// ============================================================================

/// Represents the position where a statusbar button should be placed
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StatusbarPosition {
    /// Left side of the statusbar (with drawer buttons)
    Left,
    /// Right side of the statusbar (with analyzer status)
    Right,
}

/// Action to perform when a statusbar button is clicked
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StatusbarAction {
    /// Open an editor by its EditorId in the tab system
    OpenEditor {
        editor_id: crate::identifiers::EditorId,
        /// Optional file path to open. If None, creates a new empty editor.
        file_path: Option<std::path::PathBuf>,
    },

    /// Toggle visibility of a drawer/panel
    ToggleDrawer {
        /// Unique identifier for the drawer
        drawer_id: String,
    },

    /// Execute a custom callback (function pointer provided by plugin)
    ///
    /// # Safety
    ///
    /// Because plugins are never unloaded, function pointers remain valid
    /// for the process lifetime. This is safe!
    Custom,
}

/// Unique identifier for a statusbar button
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StatusbarButtonId(String);

impl StatusbarButtonId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for StatusbarButtonId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Definition of a statusbar button that a plugin can register
#[derive(Clone)]
pub struct StatusbarButtonDefinition {
    /// Unique identifier for this button
    pub id: StatusbarButtonId,

    /// Icon to display
    pub icon: ui::IconName,

    /// Tooltip text shown on hover
    pub tooltip: String,

    /// Position in the statusbar
    pub position: StatusbarPosition,

    /// Optional badge count to display (e.g., error count)
    pub badge_count: Option<u32>,

    /// Optional badge color (if None, uses default theme color)
    pub badge_color: Option<gpui::Hsla>,

    /// Action to perform when clicked
    pub action: StatusbarAction,

    /// Optional custom callback for Custom action type
    ///
    /// # Safety
    ///
    /// This function pointer remains valid because plugins are never unloaded.
    /// The plugin code stays loaded for the process lifetime, so this pointer
    /// will always point to valid code.
    pub custom_callback: Option<fn(&mut gpui::Window, &mut gpui::App)>,

    /// Priority for ordering (higher = further right/left, depending on position)
    pub priority: i32,

    /// Whether the button is currently active/selected
    pub active: bool,

    /// Optional custom color for the icon
    pub icon_color: Option<gpui::Hsla>,
}

impl StatusbarButtonDefinition {
    /// Create a new statusbar button definition
    pub fn new(
        id: impl Into<String>,
        icon: ui::IconName,
        tooltip: impl Into<String>,
        position: StatusbarPosition,
        action: StatusbarAction,
    ) -> Self {
        Self {
            id: StatusbarButtonId::new(id),
            icon,
            tooltip: tooltip.into(),
            position,
            badge_count: None,
            badge_color: None,
            action,
            custom_callback: None,
            priority: 0,
            active: false,
            icon_color: None,
        }
    }

    /// Set the badge count
    pub fn with_badge(mut self, count: u32) -> Self {
        self.badge_count = Some(count);
        self
    }

    /// Set the badge color
    pub fn with_badge_color(mut self, color: gpui::Hsla) -> Self {
        self.badge_color = Some(color);
        self
    }

    /// Set the custom callback (for Custom action type)
    pub fn with_callback(mut self, callback: fn(&mut gpui::Window, &mut gpui::App)) -> Self {
        self.custom_callback = Some(callback);
        self
    }

    /// Set the priority
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Set whether the button is active
    pub fn with_active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Set a custom icon color
    pub fn with_icon_color(mut self, color: gpui::Hsla) -> Self {
        self.icon_color = Some(color);
        self
    }
}

/// Definition of a statusbar badge that a plugin can display.
#[derive(Debug, Clone)]
pub struct StatusbarBadgeDefinition {
    /// Unique identifier for this badge
    pub id: String,
    /// Text to display on the badge
    pub text: String,
    /// Optional color for the badge
    pub color: Option<gpui::Hsla>,
    /// Optional tooltip
    pub tooltip: Option<String>,
    /// Priority for ordering
    pub priority: i32,
}

// ============================================================================
// Statusbar Extension Trait
// ============================================================================

use crate::actions::AssetKind;

/// Optional trait for plugins that register statusbar buttons.
///
/// Implement this on your [`EditorPlugin`](crate::plugin::EditorPlugin) type
/// to provide statusbar buttons and/or declare accepted drop asset kinds.
pub trait EditorPluginStatusbar: crate::plugin::EditorPlugin {
    /// Get statusbar buttons this plugin wants to register.
    fn statusbar_buttons(&self) -> Vec<StatusbarButtonDefinition> {
        Vec::new()
    }

    /// Declare which [`AssetKind`]s this plugin's editors are willing to accept
    /// when an asset is dropped onto one of their panels.
    fn accepted_drop_kinds(&self) -> Vec<AssetKind> {
        Vec::new()
    }
}
