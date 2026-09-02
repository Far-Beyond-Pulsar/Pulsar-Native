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

// Resolved per-light GPU frames (Pulsar-Native#636) -- transform-folded
// light state maintained at change time from World subscriptions, replacing
// rebuild_light_frame's per-frame CPU combine.
pub mod light_frame;

// Resolved per-instance mesh frames (Pulsar-Native#638) -- the transform-
// derived half of each static-mesh instance, same subscription-maintained
// pattern as light_frame.
pub mod mesh_frame;

// Play-mode level bootstrap (Pulsar-Native#637) -- hydrates a `.level` file
// into WorldSceneStore/SceneDb instead of pulsar_scene::SceneLoader's direct
// Helio Scene writes.
pub mod runtime_level;

// World/Entity-backed scene store (Phase B1, Pulsar-Native#553) -- the live
// authoritative store. See `world_store`'s own doc for the full picture.
pub mod world_store;

// Script object model bridge (Pulsar-Native#639) -- `WorldSceneStore` as
// the StableId⇄Entity resolver + duplicate-instance store the script-facing
// handles route through. Impls only; no new storage.
pub mod script_ref_bridge;
pub use script_ref_bridge::{entity_with_stable_id, first_entity_named};

#[cfg(feature = "render")]
// Shared WorldSceneStore <-> helio::Renderer operations (#637): GPU seam
// attach + per-frame static-mesh/light frame assembly.
pub mod helio_bridge;

// Re-export new system types for convenience
pub use component_db::ComponentDb;
#[cfg(feature = "render")]
pub use helio_bridge::{
    attach_gpu_render_seam, rebuild_light_frame, rebuild_static_mesh_frame, step_scene_for_render,
};
pub use light_frame::{LightFrameMaintainer, ResolvedLightFrame};
pub use mesh_frame::{MeshFrameMaintainer, ResolvedMeshFrame};
pub use metadata::{ComponentInstance, EditorObjectId};
pub use metadata_db::SceneMetadataDb;
pub use runtime_level::{EditorCamera, LevelExtras, RuntimeLevel, RuntimeLevelError};
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
