//! Shared scene database for the Pulsar engine.
//!
//! `WorldSceneStore` (`pulsar_scenedb::World`, plus the stable-id/hierarchy/
//! dirty-tracking bookkeeping `World` itself doesn't provide) is the live
//! authoritative store, shared by the editor UI and the renderer.
//! `SceneMetadataDb`/`ComponentDb` hold per-object JSON component-instance
//! data for any class not yet migrated onto a typed `World` component via
//! `pulsar_world_registry`.
//!
//! Two earlier, fully-superseded systems used to live in this module and
//! were deleted rather than kept as dead weight: a lock-free-atomics
//! `SceneDb`/`SceneEntry` store (confirmed zero production construction
//! sites) and `HierarchyManager`/`SceneMetadataDb`'s own object/hierarchy/
//! selection surface (confirmed zero external callers -- `WorldSceneStore`
//! already reimplements the same hierarchy shape, keyed by `Entity` instead
//! of `String`, and is the one actually in use).

// New metadata system modules
pub mod component_db;
pub mod metadata;
pub mod metadata_db;

// World/Entity-backed scene store (Phase B1, Pulsar-Native#553) -- the live
// authoritative store. See `world_store`'s own doc for the full picture.
pub mod world_store;

// Re-export new system types for convenience
pub use component_db::ComponentDb;
pub use metadata::{ComponentInstance, EditorObjectId};
pub use metadata_db::SceneMetadataDb;
pub use world_store::{
    Name, ObjectSnapshot, Parent, RenderProps, StableId, Transform, Visibility, WorldSceneStore,
    WorldSceneStoreError,
};

use bitflags::bitflags;
use glam::Mat4;
use serde::{Deserialize, Serialize};

// ─── Public types ────────────────────────────────────────────────────────────

/// Same underlying type as [`EditorObjectId`] -- there was never a real
/// distinction between the two, just two independently-declared aliases
/// that could drift. `EditorObjectId` (paired with `ComponentInstance`) is
/// the canonical declaration; this is the alias the rest of the editor
/// (`SceneDatabase` and its ~50 call sites) already spells it as.
pub type ObjectId = EditorObjectId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObjectType {
    Empty,
    Folder,
    Camera,
    Light(LightType),
    Mesh(MeshType),
    ParticleSystem,
    AudioSource,
    Blueprint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LightType {
    Directional,
    Point,
    Spot,
    Area,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MeshType {
    Cube,
    Sphere,
    Cylinder,
    Plane,
    Custom,
}

bitflags! {
    // `Debug`/`PartialEq`/`Eq` weren't previously derived -- added so
    // `WorldSceneStore`'s tests (`scene::world_store`) can assert on flag
    // values directly instead of poking at `.bits()`. Safe, additive:
    // bitflags-generated types are plain integer wrappers, so these derives
    // can't change existing behavior anywhere else.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub struct ObjectDirtyFlags: u8 {
        const TRANSFORM = 1;
        const PROPS = 2;
        const HIERARCHY = 4;
        const COMPONENTS = 8;
        const VISIBILITY = 16;
        const NAME = 32;
    }
}

/// A single object update within a SceneDbDelta.
pub struct ObjectUpdate {
    pub id: String,
    pub transform: Option<Mat4>,
    pub visible: Option<bool>,
    pub name: Option<String>,
}

/// Delta snapshot of changes since the last drain.
pub struct SceneDbDelta {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub updated: Vec<ObjectUpdate>,
    pub revision: u64,
}

// ─── Gizmo state ─────────────────────────────────────────────────────────────

/// Gizmo state for the level editor
#[derive(Clone, Debug, PartialEq)]
pub struct GizmoState {
    pub gizmo_type: GizmoType,
    pub highlighted_axis: Option<GizmoAxis>,
    pub scale_factor: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoType {
    None,
    Translate,
    Rotate,
    Scale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GizmoAxis {
    X,
    Y,
    Z,
}

impl Default for GizmoState {
    fn default() -> Self {
        Self {
            gizmo_type: GizmoType::None,
            highlighted_axis: None,
            scale_factor: 1.0,
        }
    }
}

