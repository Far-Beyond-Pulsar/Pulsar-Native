//! Workspace dock panels for the Level Editor.
//!
//! One file per panel wrapper (each implements `ui::dock::Panel` and owns its
//! own frame pump — see `ui/frame_pump.rs`). `mode_tools` is the odd one out:
//! every other panel here corresponds to a fixed part of the editor's default
//! layout, while `ModeToolsPanel` is generic over whichever [`ToolMode`] is
//! active — see its own doc for why it lives here rather than under
//! `tool_modes/`.
//!
//! [`ToolMode`]: crate::level_editor::tool_modes::ToolMode

mod hierarchy;
mod mode_tools;
mod properties;
mod viewport;
mod world_settings;

pub use hierarchy::HierarchyPanelWrapper;
pub use mode_tools::ModeToolsPanel;
pub use properties::PropertiesPanelWrapper;
pub use viewport::ViewportPanelWrapper;
pub use world_settings::WorldSettingsPanel;
