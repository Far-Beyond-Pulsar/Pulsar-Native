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

use engine_backend::scene::SceneWorldExt;
use parking_lot::RwLock;
use plugin_editor_api::{AssetKind, AssetSubscription, AssetUpdated};

use crate::level_editor::scene_edit::{classes, ObjectId};
use crate::level_editor::state::LevelEditorState;

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
        let touched = classes::apply_class_asset_update(&mut world, event);
        // #935: rebuilt generated children are new entities, and their
        // components were inserted before anything watched them, so the
        // renderer saw no change for them. Arm their render-row
        // subscriptions, then report every rebuilt component as changed
        // (GPU mirror refresh + a `Mut` write, what a property edit does),
        // so each instance's light / mesh rows are re-derived at the next
        // frame, not when the object is next touched.
        for id in &touched {
            if let Some(entity) = world.entity_for(id) {
                engine_backend::scene::arm_render_row_subscriptions_for_entity(&mut world, entity);
                engine_backend::scene::mark_render_components_changed(&mut world, entity);
            }
        }
        touched
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
