//! Scene metadata types that bridge UI layer to Helio Scene
//!
//! This module defines the lightweight metadata layer that sits on top of Helio Scene,
//! providing organizational features (folders, hierarchy) and component storage while
//! Helio Scene remains the single source of truth for render data.

use serde::{Deserialize, Serialize};

/// Editor-side unique identifier for scene objects
///
/// This is separate from Helio's internal IDs to allow for folders and other
/// organizational constructs that don't exist in Helio.
pub type EditorObjectId = String;

/// Metadata for a scene object - links organizational data to Helio actors
///
/// This struct bridges the gap between the UI's organizational needs (folders,
/// names, hierarchy) and Helio Scene's pure render data. Transform data lives
/// in Helio Scene, not here.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneObjectMetadata {
    /// Unique editor identifier
    pub editor_id: EditorObjectId,

    /// Reference to the corresponding Helio scene actor
    pub helio_handle: HelioActorHandle,

    /// Display name for UI
    pub name: String,

    /// Type of object for UI display
    pub object_type: ObjectType,

    /// Parent object ID for hierarchy (None = root level)
    pub parent: Option<EditorObjectId>,

    /// Canonical path from scene root (e.g., "Geometry/Spheres/Blue Sphere")
    /// Automatically computed from parent chain
    pub scene_path: String,

    /// UI visibility state
    pub visible: bool,

    /// UI lock state (prevents editing)
    pub locked: bool,
}

/// Reference to a Helio scene actor
///
/// Maps editor objects to their corresponding Helio Scene representations.
/// Folders and empty objects don't have Helio equivalents.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum HelioActorHandle {
    /// Regular mesh object
    Object(HelioObjectId),

    /// Light source
    Light(HelioLightId),

    /// Virtual object (billboard, particle system, etc.)
    VirtualObject(HelioVirtualObjectId),

    /// Water volume
    Water(HelioWaterVolumeId),

    /// Folder (organizational only, no Helio representation)
    Folder,

    /// Empty object (transform-only, no Helio representation)
    Empty,
}

/// Helio object ID wrapper
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HelioObjectId(pub u64);

/// Helio light ID wrapper
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HelioLightId(pub u64);

/// Helio virtual object ID wrapper
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HelioVirtualObjectId(pub u64);

/// Helio water volume ID wrapper
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HelioWaterVolumeId(pub u64);

/// Object type for UI categorization
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectType {
    /// Empty transform node
    Empty,

    /// Folder for organization
    Folder,

    /// Camera
    Camera,

    /// Light source
    Light(LightType),

    /// Mesh object
    Mesh(MeshType),

    /// Particle system
    ParticleSystem,

    /// Audio source
    AudioSource,

    /// Water volume
    Water,
}

/// Light type categorization
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightType {
    Directional,
    Point,
    Spot,
    Area,
}

/// Mesh type categorization
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshType {
    Cube,
    Sphere,
    Cylinder,
    Plane,
    Custom,
}

/// Component instance attached to a scene object
///
/// Uses the reflection system for property inspection and editing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ComponentInstance {
    /// Class name from the component registry (e.g., "PhysicsComponent")
    pub class_name: String,

    /// Serialized component data
    ///
    /// NOTE: In the full implementation, this would be Box<dyn EngineClass>,
    /// but that's not directly serializable. For now, we store serialized JSON
    /// and reconstruct via the registry on load.
    pub data: serde_json::Value,
}

impl SceneObjectMetadata {
    /// Create new metadata for a Helio object
    pub fn new_object(
        editor_id: EditorObjectId,
        helio_id: HelioObjectId,
        name: String,
        mesh_type: MeshType,
    ) -> Self {
        Self {
            editor_id,
            helio_handle: HelioActorHandle::Object(helio_id),
            name,
            object_type: ObjectType::Mesh(mesh_type),
            parent: None,
            scene_path: String::new(),
            visible: true,
            locked: false,
        }
    }

    /// Create new metadata for a Helio light
    pub fn new_light(
        editor_id: EditorObjectId,
        helio_id: HelioLightId,
        name: String,
        light_type: LightType,
    ) -> Self {
        Self {
            editor_id,
            helio_handle: HelioActorHandle::Light(helio_id),
            name,
            object_type: ObjectType::Light(light_type),
            parent: None,
            scene_path: String::new(),
            visible: true,
            locked: false,
        }
    }

    /// Create new metadata for a folder
    pub fn new_folder(editor_id: EditorObjectId, name: String) -> Self {
        Self {
            editor_id,
            helio_handle: HelioActorHandle::Folder,
            name,
            object_type: ObjectType::Folder,
            parent: None,
            scene_path: String::new(),
            visible: true,
            locked: false,
        }
    }

    /// Create new metadata for an empty object
    pub fn new_empty(editor_id: EditorObjectId, name: String) -> Self {
        Self {
            editor_id,
            helio_handle: HelioActorHandle::Empty,
            name,
            object_type: ObjectType::Empty,
            parent: None,
            scene_path: String::new(),
            visible: true,
            locked: false,
        }
    }

    /// Check if this object has a Helio representation
    pub fn has_helio_actor(&self) -> bool {
        !matches!(
            self.helio_handle,
            HelioActorHandle::Folder | HelioActorHandle::Empty
        )
    }

    /// Get Helio object ID if this is a mesh object
    pub fn helio_object_id(&self) -> Option<HelioObjectId> {
        match self.helio_handle {
            HelioActorHandle::Object(id) => Some(id),
            _ => None,
        }
    }

    /// Get Helio light ID if this is a light
    pub fn helio_light_id(&self) -> Option<HelioLightId> {
        match self.helio_handle {
            HelioActorHandle::Light(id) => Some(id),
            _ => None,
        }
    }
}
