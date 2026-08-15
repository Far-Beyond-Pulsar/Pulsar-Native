//! Component-instance storage types for the level editor.
//!
//! `EditorObjectId`/`ComponentInstance` are the only types still live here --
//! `SceneObjectMetadata`/`HelioActorHandle`/`HelioObjectId`/`HelioLightId`/
//! `HelioVirtualObjectId`/`HelioWaterVolumeId` and this module's own
//! `ObjectType`/`LightType`/`MeshType` (shadow duplicates of the real,
//! actually-used ones in `scene::mod`) were deleted as confirmed-dead code:
//! zero callers outside `SceneMetadataDb`'s own now-removed object/
//! hierarchy surface. See `scene::mod`'s module doc.

use serde::{Deserialize, Serialize};

/// Editor-side unique identifier for scene objects.
///
/// This is separate from Helio's internal IDs to allow for folders and other
/// organizational constructs that don't exist in Helio.
pub type EditorObjectId = String;

/// Component instance attached to a scene object
///
/// Uses the reflection system for property inspection and editing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentInstance {
    /// Class name from the component registry (e.g., "PhysicsComponent")
    pub class_name: String,

    /// Whether the component is active.
    ///
    /// Disabled components remain serialized but are ignored by scene-property
    /// projection and behave as if they were absent.
    #[serde(default = "default_component_enabled")]
    pub enabled: bool,

    /// Serialized component data
    ///
    /// NOTE: In the full implementation, this would be Box<dyn EngineClass>,
    /// but that's not directly serializable. For now, we store serialized JSON
    /// and reconstruct via the registry on load.
    pub data: serde_json::Value,
}

fn default_component_enabled() -> bool {
    true
}
