//! The engine's scene: a `pulsar_scenedb::SceneDb` used directly.
//!
//! There is no store or facade around it. Objects are entities carrying the
//! plain components in [`components`]; [`world_ext`] adds stateless helpers
//! (identity lookup, hierarchy, selection) that are derived from those
//! components on demand. The editor, the renderer and the play-mode runtime
//! all share one [`SharedScene`].

// Resolved per-light GPU frames (Pulsar-Native#636) -- transform-folded
// light state maintained at change time from World subscriptions, replacing
// rebuild_light_frame's per-frame CPU combine.
pub mod light_frame;

// Resolved per-instance mesh frames (Pulsar-Native#638) -- the transform-
// derived half of each static-mesh instance, same subscription-maintained
// pattern as light_frame.
pub mod mesh_frame;
#[cfg(feature = "render")]
pub mod voxel_frame;
#[cfg(feature = "render")]
#[cfg(feature = "render")]
pub mod voxel_source;

// Persistent texture/material/sectioned-mesh/object resource components.
pub mod render_resources;

// Play-mode level bootstrap (Pulsar-Native#637) -- hydrates a `.level` file
// into WorldSceneStore/SceneDb instead of pulsar_scene::SceneLoader's direct
// Helio Scene writes.
pub mod runtime_level;

// World/Entity-backed scene store (Phase B1, Pulsar-Native#553) -- the live
// authoritative store. See `world_store`'s own doc for the full picture.

// Script object model bridge (Pulsar-Native#639) -- `WorldSceneStore` as
// the StableId⇄Entity resolver + duplicate-instance store the script-facing
// handles route through. Impls only; no new storage.
pub mod script_ref_bridge;
pub use script_ref_bridge::{entity_with_stable_id, first_entity_named};

#[cfg(feature = "render")]
pub mod editor_rows;
#[cfg(feature = "render")]
// Shared WorldSceneStore <-> helio::Renderer operations (#637): GPU seam
// attach + per-frame static-mesh/light frame assembly.
pub mod helio_bridge;

// Re-export new system types for convenience
#[cfg(feature = "render")]
pub use helio_bridge::{
    arm_render_row_subscriptions, ensure_gpu_mirror, retire_gpu_rows_for_entity,
    sync_static_mesh_rows,
};

/// Hook a `World` up to the SceneDB Inspector (CPU + GPU live view). Inert
/// unless this process was launched by `scenedb_inspector`; safe to call for
/// every replacement world (undo/redo, level load).
#[cfg(feature = "render")]
pub fn install_scenedb_inspector(world: &mut pulsar_scenedb::World) -> bool {
    scenedb_inspector_agent::install_world(world)
}

#[cfg(not(feature = "render"))]
pub fn install_scenedb_inspector(_world: &mut pulsar_scenedb::World) -> bool {
    false
}
pub use light_frame::{LightFrameMaintainer, ResolvedLightFrame};
pub use mesh_frame::{MeshFrameMaintainer, ResolvedMeshFrame};
pub use pulsar_scene_model::{
    attachments, components, instance, world_ext, ComponentAttachments, ComponentInstance,
    EditorObjectId, LightType, MeshType, Name, ObjectId, ObjectType, Parent, RenderProps,
    SceneError, SceneWorldExt, Selected, SiblingIndex, SpawnObject, StableId, Transform,
    Visibility,
};
pub use render_resources::{
    insert_render_resources, MaterialComponent, MaterialResource, MaterialTextureResource,
    MeshObjectComponent, MeshObjectResource, MeshSectionResource, SectionedMeshComponent,
    SectionedMeshResource, TextureComponent, TextureResource,
};
pub use runtime_level::{EditorCamera, LevelExtras, RuntimeLevel, RuntimeLevelError};

/// The scene is a `pulsar_scenedb::SceneDb` shared between the editor UI, the
/// renderer and the play-mode runtime. This alias only names the sharing.
pub type SharedScene = std::sync::Arc<parking_lot::RwLock<pulsar_scenedb::SceneDb>>;

/// A fresh scene with a change tracker attached, so [`pulsar_scenedb::World::revision`]
/// and any subsystem reading `world.change_tracker()` work from the first mutation.
pub fn new_scene() -> pulsar_scenedb::SceneDb {
    let mut scene = pulsar_scenedb::SceneDb::new();
    scene
        .world
        .attach_change_tracker(pulsar_scenedb::SharedChangeTracker::new());
    scene
}

use glam::Mat4;

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
