//! Component-instance record types shared by the editor and the level format.

use serde::{Deserialize, Serialize};

/// Editor-side unique identifier for scene objects.
///
/// This is separate from Helio's internal IDs to allow for folders and other
/// organizational constructs that don't exist in Helio.
pub type EditorObjectId = String;

/// The JSON record of one component instance attached to a scene object.
///
/// A boundary format only -- level files, history snapshots, external tools.
/// The live instance is its own entity ([`crate::attachments`]) holding the
/// typed value; `pulsar_world_registry::instances` converts between the two.
/// `data` is the class's JSON plus `__`-prefixed attachment metadata
/// (`__instance_id`, `__slot_id`, `__transform`, `__parent_index`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentInstance {
    /// Class name from the component registry (e.g., "PhysicsComponent")
    pub class_name: String,

    /// Whether the component is active.
    ///
    /// A disabled instance keeps its typed value; runtime consumers skip it.
    #[serde(default = "default_component_enabled")]
    pub enabled: bool,

    /// The class's JSON representation plus attachment metadata keys.
    pub data: serde_json::Value,
}

fn default_component_enabled() -> bool {
    true
}
