//! The dockable panels of the mocap editor; each reads the shared session model.

mod calibration;
mod inspector;
mod open;
mod scene;
mod sources;
mod timeline;
mod viewport;

pub use inspector::InspectorPanel;
pub(crate) use open::{pick_pose_model, pick_take};
pub use scene::ScenePanel;
pub use sources::SourcesPanel;
pub use timeline::TimelinePanel;
pub use viewport::ViewportPanel;
