//! The engine's scene: a `pulsar_scenedb::SceneDb` used directly.
//!
//! There is no store or facade around it. Objects are entities carrying the
//! plain components in [`components`]; [`world_ext`] adds stateless helpers
//! (identity lookup, hierarchy, selection) that are derived from those
//! components on demand. The editor, the renderer and the play-mode runtime
//! all share one [`SharedScene`].

// Persistent texture/material/sectioned-mesh/object resource components.

// Play-mode level bootstrap -- hydrates a `.level` file
// into SceneDb instead of pulsar_scene::SceneLoader's direct
// Helio Scene writes.
pub mod runtime_level;

/// Placed class instances follow class edits in every world (SceneDB#84).
pub mod classes;

// What a level may hold only one of (one sky), checked by editors
// before an edit and repaired on load.
pub mod level_rules;

// World/Entity-backed scene store -- the live
// authoritative store. See `world_store`'s own doc for the full picture.

// Script object model bridge -- `SceneDb` as
// the StableId⇄Entity resolver + duplicate-instance store the script-facing
// handles route through. Impls only; no new storage.
pub mod script_ref_bridge;
pub use script_ref_bridge::{entity_with_stable_id, first_entity_named};

pub mod component_events;
pub mod asset_event_sources;
// A plugin's component classes, registered from its root's build.
pub mod class_spec;
pub mod construct_pump;
#[cfg(feature = "render")]
pub use class_spec::ClassRows;
pub use class_spec::{ClassSpec, ComponentsBuilder};
pub use construct_pump::EditorConstructPump;
pub mod game_hooks;
pub use asset_event_sources::{
    asset_slots, component_events, component_events_with, node_title, AssetEventSource, AssetEvents, AssetPathSlot,
    AssetSlot, AssetUse,
    CacheStats,
    ComponentData, EventCache, SourceCx,
};
pub use component_events::{
    CatalogEvent, ComponentEventCatalog, EventFamily, EventView, FamilyOption, FamilyParam,
    ParamKind, ParamValues, ResolvedFamily, ResolvedOutput,
};
#[cfg(feature = "render")]
// Shared SceneDb <-> helio::Renderer seam: GPU mirror attach
// and Helio's scene join over the authored rows.
pub mod helio_bridge;
pub mod sublevel_display;
#[cfg(feature = "render")]
pub mod sublevels;

#[cfg(feature = "render")]
pub use helio_bridge::{
    ensure_gpu_mirror, environment_join, first_fixed_in_group, hierarchy_resolve,
    hierarchy_resolve_keys, object_join_context, row_join_keys, scene_join, ObjectJoinContext,
    ObjectJoinRegistration, SceneRowsRegistration, SpaceProducer, SpaceProducerRegistration,
    SpaceSync, SpaceTable, NO_OWNER_BOUNDS,
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
    pivots, world_ext, ComponentAttachments, ComponentInstance, EditorObjectId, Name, ObjectId, ObjectType,
    Parent, RenderProps, SceneError, SceneWorldExt, Selected, SiblingIndex, SpawnObject, StableId,
    Transform, Visibility,
};
pub use runtime_level::{
    EditorCamera, HydrationProgress, LevelExtras, LevelHydration, PreparedLevelFile, RuntimeLevel,
    RuntimeLevelError,
};

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
    scene.world.set_asset_priority(Some(asset_priority));
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

/// Advance asset loading in the shared scene within a frame's budget: start
/// the nearest loads, apply and bind the finished ones, and leave the rest for
/// the next frame (`pending` in the report says how much). Takes the scene's
/// write lock only when there is work. Called once per frame before the GPU
/// upload by every render loop (the editor viewport's, and the game's
/// windowed and embedded ones), and at the end of every game tick, which
/// also covers headless runs. See [`frame_asset_budget`].
pub fn pump_assets(scene: &SharedScene) -> pulsar_scenedb::AssetPumpReport {
    if !scene.read().world.asset_work_pending() {
        return pulsar_scenedb::AssetPumpReport::default();
    }
    profiling::profile_scope!("scene::pump_assets");
    let budget = frame_asset_budget();
    scene.write().world.pump_assets_within(budget)
}

/// Tell the scene where the viewer is, so loads start nearest first
/// (`new_scene` installs the ranking). Free when no asset work is pending.
pub fn set_asset_focus(scene: &SharedScene, eye: [f64; 3]) {
    if scene.read().world.asset_work_pending() {
        scene.write().world.set_asset_focus(eye);
    }
}

/// What one frame may spend in [`pump_assets`]: project settings
/// `streaming.asset_bind_budget_ms` (default 2) and
/// `streaming.asset_bind_max_per_frame` (default 8); 0 lifts that limit.
pub fn frame_asset_budget() -> pulsar_scenedb::AssetBudget {
    let setting = |key: &str, default: i64| {
        match engine_state::settings::global_config().get(
            engine_state::settings::NS_PROJECT,
            "streaming",
            key,
        ) {
            Ok(engine_state::settings::ConfigValue::Int(value)) => value.max(0),
            _ => default,
        }
    };
    let (ms, max) = (
        setting("asset_bind_budget_ms", 2),
        setting("asset_bind_max_per_frame", 8),
    );
    pulsar_scenedb::AssetBudget {
        max_assets: if max == 0 { usize::MAX } else { max as usize },
        time: (ms > 0).then(|| std::time::Duration::from_millis(ms as u64)),
    }
}

/// How soon an entity's assets are wanted: its distance from the viewer
/// ([`pulsar_scenedb::World::asset_focus`]). Parent-relative transforms rank
/// by their local position, which is the nearest guess before a world
/// transform exists.
fn asset_priority(world: &pulsar_scenedb::World, entity: pulsar_scenedb::Entity) -> Option<f32> {
    let position = world.get::<pulsar_scene_model::Transform>(entity)?.position;
    let focus = world.asset_focus();
    let delta = [0, 1, 2].map(|axis| position[axis] - focus[axis]);
    Some((delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt() as f32)
}

/// Apply one asset update to `world` (SceneDB#84): a changed file reloads
/// the world assets it is or feeds, as each asset kind's owner maps it
/// (`helio_component::AssetReloadRegistration`: a mesh file its mesh, an
/// image its texture and every material graph sampling it, a material
/// document its material), and the world rebinds every component showing
/// them when the new data arrives. A class (Blueprint) update rebuilds every
/// placed instance of the class from its new definition, keeping each
/// instance's overrides (see [`classes`]). Returns how many assets were
/// queued plus how many entities were rebuilt.
pub fn reload_updated_asset(
    world: &mut pulsar_scenedb::World,
    event: &pulsar_events::AssetUpdated,
) -> usize {
    match (&event.kind, event.path.as_deref()) {
        (pulsar_events::AssetKind::Blueprint, _) => {
            classes::reload_class_instances(world, event).len()
        }
        (kind, Some(path)) => {
            let reloaded: usize = helio_component::asset_sources_for_file(kind, path)
                .iter()
                .map(|source| world.asset_source_changed(source))
                .sum();
            // what depends on the file without being the asset it loads
            // (a running animation graph's sequences)
            reloaded + game_hooks::notify_asset_changed(world, path)
        }
        _ => 0,
    }
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

#[cfg(test)]
mod asset_pump_tests {
    use super::*;

    #[test]
    fn a_frame_pumps_a_bounded_share_of_the_assets() {
        let budget = frame_asset_budget();
        assert_eq!(budget.max_assets, 8);
        assert_eq!(budget.time, Some(std::time::Duration::from_millis(2)));
    }

    #[test]
    fn assets_rank_by_distance_from_the_viewer() {
        let mut scene = new_scene();
        let world = &mut scene.world;
        let near = world.spawn();
        let far = world.spawn();
        let at = |x: f64| pulsar_scene_model::Transform {
            position: [x, 0.0, 0.0],
            rotation: [0.0; 3],
            scale: [1.0; 3],
        };
        world.insert(near, at(3.0));
        world.insert(far, at(300.0));
        world.set_asset_focus([0.0; 3]);
        assert_eq!(asset_priority(world, near), Some(3.0));
        assert_eq!(asset_priority(world, far), Some(300.0));
        world.set_asset_focus([300.0, 0.0, 0.0]);
        assert_eq!(asset_priority(world, far), Some(0.0));
        let bare = world.spawn();
        assert_eq!(asset_priority(world, bare), None, "no transform, no rank");
    }
}
