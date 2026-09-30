//! Workspace dock panels for the Level Editor.
//!
//! One file per panel wrapper (each implements `ui::dock::Panel` and owns its
//! own frame pump — see `ui/frame_pump.rs`). Each corresponds to a fixed part
//! of the editor's default layout; panels a tool mode adds live with that mode.

mod hierarchy;
mod properties;
mod viewport;
mod world_settings;

pub use hierarchy::HierarchyPanelWrapper;
pub use properties::PropertiesPanelWrapper;
pub use viewport::ViewportPanelWrapper;
pub use world_settings::WorldSettingsPanel;
