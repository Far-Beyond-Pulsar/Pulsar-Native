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
