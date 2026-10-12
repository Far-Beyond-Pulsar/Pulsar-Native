//! Asset-update notifications in the level editor (#921).
//!
//! Plugins publish `plugin_editor_api::AssetUpdated` when they rewrite an
//! asset. The level editor subscribes to class assets (the Blueprint kind):
//! every placed instance of the updated class is rebuilt from the new class
//! definition, keeping its overrides, and a running Play-In-Editor game gets
//! the event forwarded so its script runtime reloads the class.
//!
//! The forward is queued on the editor state and delivered by the game
//! viewport on the render thread, the only thread the embedded game may be
//! called from.

use std::sync::Arc;

use parking_lot::RwLock;
use plugin_editor_api::{AssetKind, AssetSubscription, AssetUpdated};

use crate::scene_edit::{classes, ObjectId};
use crate::state::LevelEditorState;

/// Subscribe the editor at `state` to class asset updates for as long as
/// the returned subscription lives.
pub fn subscribe_class_updates(state: Arc<RwLock<LevelEditorState>>) -> AssetSubscription {
    plugin_editor_api::subscribe_asset_updates(Some(AssetKind::Blueprint), move |event| {
        let touched = handle_asset_update(&state, event);
        if !touched.is_empty() {
            tracing::info!(
                objects = touched.len(),
                "Rebuilt placed class instances after a class update"
            );
        }
    })
}

/// Apply one asset update to the editor: rebuild the placed instances of an
/// updated class and queue the event for a running game. Returns the ids of
/// the objects rebuilt.
pub fn handle_asset_update(
    state: &Arc<RwLock<LevelEditorState>>,
    event: &AssetUpdated,
) -> Vec<ObjectId> {
    let touched = {
        let state = state.read();
        let mut world = state.scene.world_mut();
        // Rebuilt instances are ordinary component writes: their GPU rows
        // follow through SceneDB's own write path, and the renderer's scene
        // join picks them up with nothing armed or marked here (#935).
        classes::apply_class_asset_update(&mut world, event)
    };
    let mut state = state.write();
    if !touched.is_empty() {
        // The instances still match what a save would write (overrides are
        // unchanged), so this is not an unsaved level edit.
        state.scene.bump_revision(false);
        // Wake the level editor (this runs on the publisher's thread,
        // outside GPUI): its poller notifies the panel when this moves.
        state.scene.class_updates = state.scene.class_updates.wrapping_add(1);
    }
    if state.play.pie.active {
        state.play.pie.pending_asset_updates.push(event.clone());
    }
    touched
}

/// Subscribe the editor at `state` to mesh asset updates (a re-import): every
/// static mesh showing the updated file reloads it.
pub fn subscribe_mesh_updates(state: Arc<RwLock<LevelEditorState>>) -> AssetSubscription {
    plugin_editor_api::subscribe_asset_updates(Some(AssetKind::Mesh), move |event| {
        let reloaded = handle_mesh_update(&state, event);
        if reloaded > 0 {
            tracing::info!(
                meshes = reloaded,
                "Reloading meshes after a mesh asset update"
            );
        }
    })
}

/// Reload the mesh asset of the updated file in the editor's world
/// (SceneDB#84): the world loads it again off the write path and rebinds
/// every `StaticMeshComponent` showing it when the new data arrives.
/// Returns how many assets were queued (0 or 1).
pub fn handle_mesh_update(state: &Arc<RwLock<LevelEditorState>>, event: &AssetUpdated) -> usize {
    let Some(path) = event.path.as_ref() else {
        return 0;
    };
    let source = helio_component::components::mesh_source_for_file(path);
    let state = state.read();
    let mut world = state.scene.world_mut();
    world.asset_source_changed(&source)
}
