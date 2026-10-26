//! The engine's scene: a `pulsar_scenedb::SceneDb` used directly.
//!
//! There is no store or facade around it. Objects are entities carrying the
//! plain components in [`components`]; [`world_ext`] adds stateless helpers
//! (identity lookup, hierarchy, selection) that are derived from those
//! components on demand. The editor, the renderer and the play-mode runtime
//! all share one [`SharedScene`].

#[cfg(feature = "render")]
pub mod voxel_frame;
#[cfg(feature = "render")]
#[cfg(feature = "render")]
pub mod voxel_source;

// Persistent texture/material/sectioned-mesh/object resource components.
pub mod render_resources;

// Play-mode level bootstrap (Pulsar-Native#637) -- hydrates a `.level` file
// into SceneDb instead of pulsar_scene::SceneLoader's direct
// Helio Scene writes.
pub mod runtime_level;

/// Placed class instances follow class edits in every world (SceneDB#84).
pub mod classes;

// What a level may hold only one of (one sky, #1057), checked by editors
// before an edit and repaired on load.
pub mod level_rules;

// World/Entity-backed scene store (Phase B1, Pulsar-Native#553) -- the live
// authoritative store. See `world_store`'s own doc for the full picture.

// Script object model bridge (Pulsar-Native#639) -- `SceneDb` as
// the StableId⇄Entity resolver + duplicate-instance store the script-facing
// handles route through. Impls only; no new storage.
pub mod script_ref_bridge;
pub use script_ref_bridge::{entity_with_stable_id, first_entity_named};

#[cfg(feature = "render")]
mod authored_lod;
#[cfg(feature = "render")]
// Shared SceneDb <-> helio::Renderer seam (#637): GPU mirror attach
// and Helio's scene join over the authored rows.
pub mod helio_bridge;
#[cfg(feature = "render")]
pub mod portals;
pub mod sublevel_display;
#[cfg(feature = "render")]
pub mod sublevels;

#[cfg(feature = "render")]
pub use authored_lod::{
    AuthoredLodKeys, AuthoredLodSelection, LodSelectionRow, LOD_SELECTIONS_KEY,
};
#[cfg(feature = "render")]
pub use helio_bridge::{
    authored_lod_selection, ensure_gpu_mirror, environment_join, environment_join_keys,
    first_fixed_in_group, hierarchy_resolve, hierarchy_resolve_keys, portal_projection, scene_join,
    scene_join_keys, sync_virtual_geometry, virtual_geometry_asset_ready,
    virtual_geometry_instances, virtual_geometry_runtime_settings, SpaceSync, SpaceTable,
    VgAssetReadyPublisher, VirtualGeometryHost, VirtualGeometryInstances,
};
#[cfg(feature = "render")]
pub mod editor_postprocess;
#[cfg(feature = "render")]
pub use editor_postprocess::EditorPostProcess;

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
pub use pulsar_scene_model::{
    attachments, components, frames, hierarchy_transform, instance, level_instances, partitions,
    world_ext, ComponentAttachments, ComponentInstance, EditorObjectId, LightType, MeshType, Name,
    ObjectId, ObjectType, Parent, RenderProps, SceneError, SceneWorldExt, Selected, SiblingIndex,
    SpawnObject, StableId, Transform, Visibility,
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

/// A fresh scene with a change tracker attached, so any subsystem reading
/// `world.change_tracker()` works from the first mutation.
///
/// The tracker records every spawn, insert, change and removal until it is
/// drained. The renderer and the game tick each close the window once per
/// frame ([`end_change_window`]), so it only ever holds the current
/// frame's changes.
pub fn new_scene() -> pulsar_scenedb::SceneDb {
    let mut scene = pulsar_scenedb::SceneDb::new();
    scene
        .world
        .attach_change_tracker(pulsar_scenedb::SharedChangeTracker::new());
    scene
}

/// How long a level load waits for its assets before showing the level with
/// the rest still loading.
pub const LEVEL_ASSET_WAIT: std::time::Duration = std::time::Duration::from_secs(120);

/// Load the assets a just-hydrated level references (its meshes) before it
/// is shown, so the first frame is complete (SceneDB#84). The loads run in
/// parallel on SceneDB's workers; anything still loading after
/// [`LEVEL_ASSET_WAIT`] finishes through the renderer's per-frame pump.
pub fn resolve_level_assets(world: &mut pulsar_scenedb::World) {
    profiling::profile_scope!("scene::resolve_level_assets");
    if !world.resolve_assets_blocking(LEVEL_ASSET_WAIT) {
        tracing::warn!(
            "Level assets still loading after {:?}; they appear as they finish",
            LEVEL_ASSET_WAIT
        );
    }
}

/// Advance asset loading in the shared scene: start loads for newly
/// referenced assets and bind the finished ones. Takes the scene's write
/// lock only when there is work. Called once per frame before the GPU
/// upload by every render loop (the editor viewport's, and the game's
/// windowed and embedded ones), and at the end of every game tick, which
/// also covers headless runs.
pub fn pump_assets(scene: &SharedScene) -> pulsar_scenedb::AssetPumpReport {
    if !scene.read().world.asset_work_pending() {
        return pulsar_scenedb::AssetPumpReport::default();
    }
    profiling::profile_scope!("scene::pump_assets");
    scene.write().world.pump_assets()
}

/// Apply one asset update to `world` (SceneDB#84): a mesh, texture or
/// material update reloads that file's assets, and the world rebinds every
/// component showing them when the new data arrives. A texture is also
/// reloaded as part of every shader-graph material that samples it. A class
/// (Blueprint) update rebuilds every placed instance of the class from its
/// new definition, keeping each instance's overrides (see [`classes`]).
/// Returns how many assets were queued plus how many entities were rebuilt.
/// Other kinds are not world assets yet and are left to their own handlers.
pub fn reload_updated_asset(
    world: &mut pulsar_scenedb::World,
    event: &pulsar_events::AssetUpdated,
) -> usize {
    use helio_component::material_assets::material_sources_for_file;
    match (&event.kind, event.path.as_deref()) {
        (pulsar_events::AssetKind::Mesh, Some(path)) => {
            world.asset_source_changed(&helio_component::components::mesh_source_for_file(path))
        }
        (pulsar_events::AssetKind::Texture, Some(path)) => {
            world.asset_source_changed(&helio_component::texture_assets::texture_source_for_file(
                path,
            )) + reload_materials(world, material_sources_for_file(path))
        }
        (pulsar_events::AssetKind::Material, Some(path)) => {
            reload_materials(world, material_sources_for_file(path))
        }
        (pulsar_events::AssetKind::Blueprint, _) => {
            classes::reload_class_instances(world, event).len()
        }
        _ => 0,
    }
}

fn reload_materials(world: &mut pulsar_scenedb::World, sources: Vec<String>) -> usize {
    sources
        .iter()
        .map(|source| world.asset_source_changed(source))
        .sum()
}

/// Keep `scene` current with updated assets for as long as the returned
/// subscription lives: each [`AssetUpdated`](pulsar_events::AssetUpdated)
/// on the host bus goes through [`reload_updated_asset`]. Whoever creates a
/// world holds one for it (the editor for its level, a game loop for the
/// world it builds), so a re-import reaches every world, headless ones
/// included, without the editor. Holds the scene weakly.
pub fn subscribe_asset_reloads(scene: &SharedScene) -> pulsar_events::AssetSubscription {
    let scene = std::sync::Arc::downgrade(scene);
    pulsar_events::subscribe_asset_updates(None, move |event| {
        let Some(scene) = scene.upgrade() else {
            return;
        };
        let queued = reload_updated_asset(&mut scene.write().world, event);
        if queued > 0 {
            tracing::info!(kind = ?event.kind, path = ?event.path, "Reloading an updated asset");
        }
    })
}

/// Close this frame's change window: drop everything the world's change
/// tracker recorded since the last call and start a new frame.
///
/// Nothing in the engine consumes the tracker's history (replication will
/// drain it itself; component lifecycles and other incremental readers use
/// change-journal cursors), and an undrained tracker grows with every write,
/// forever: component deltas, spawns, despawns and removals. It also slows
/// every change record down, since each one searches the frame's list.
/// Called once per frame by the game tick and the renderer; calling it
/// more often is harmless.
pub fn end_change_window(world: &pulsar_scenedb::World) {
    if let Some(tracker) = world.change_tracker() {
        profiling::profile_scope!("scene::end_change_window");
        drop(tracker.drain_with_world(world));
        drop(tracker.drain_component_removals());
        tracker.end_frame();
    }
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

#[cfg(test)]
mod change_window_tests {
    use pulsar_scene_model::components::{Name, Visibility};

    /// The tracker only holds one frame of history: after
    /// `end_change_window`, spawns, changes, despawns and removals recorded
    /// before it are gone.
    #[test]
    fn end_change_window_empties_the_change_tracker() {
        let mut scene = super::new_scene();
        let world = &mut scene.world;
        for i in 0..100 {
            let entity = world.spawn();
            world.insert(entity, Name(format!("e{i}")));
            world.insert(entity, Visibility::default());
            world.remove::<Visibility>(entity);
            if i % 2 == 0 {
                world.despawn(entity);
            }
        }
        let tracker = world.change_tracker().expect("attached").clone();
        assert!(
            !tracker.lock().drain_component_removals().is_empty(),
            "removals were recorded"
        );

        let entity = world.spawn();
        world.insert(entity, Visibility::default());
        world.remove::<Visibility>(entity);
        super::end_change_window(world);

        let delta = tracker.drain_with_world(world);
        assert!(
            delta.spawned.is_empty()
                && delta.despawned.is_empty()
                && delta.component_deltas.is_empty()
        );
        assert!(tracker.drain_component_removals().is_empty());
    }
}
