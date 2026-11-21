//! The dockable panels of the mocap editor; each reads the shared session model.

mod inspector;
mod skeleton;
mod sources;
mod timeline;
mod viewport;

pub use inspector::InspectorPanel;
pub use skeleton::SkeletonPanel;
pub use sources::SourcesPanel;
pub use timeline::TimelinePanel;
pub use viewport::ViewportPanel;
