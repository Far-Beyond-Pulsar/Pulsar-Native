use super::{CommandResult, SceneCommand};
use crate::level_editor::state::LevelEditorState;


// ── Executor ──────────────────────────────────────────────────────────────────

/// Apply `cmd` to `state`.
///
/// Mutations go through `state.scene.database`, which writes to the shared
/// `WorldSceneStore` the Helio renderer reads every frame.  `scene_revision`
/// is bumped on every mutation, causing the polling task in
/// `LevelEditorPanel` to notify the GPUI hierarchy and properties panels.
///
/// GPUI-thread callers (panel action handlers) should additionally call
/// `cx.notify()` after this returns.
///
/// Also the sole write path onto the undo stack (Pulsar-Native#554): for
/// every variant except `SelectObject` (selection isn't undo-worthy), the
/// scene's pre-command state is captured before running `cmd` and committed
/// to `state.scene`'s undo history only if the command actually changed
/// something (`CommandResult::changed`) -- a no-op command shouldn't leave a
/// stale checkpoint an undo would just restore right back to. The match body
/// below is unchanged from before this wiring; it's wrapped in an immediately-
/// invoked closure purely so its several `return CommandResult::noop(...)`
/// early-exits stay scoped to computing `result` instead of returning from
/// this whole function before the checkpoint-commit step below runs.
pub fn execute_command(state: &mut LevelEditorState, cmd: SceneCommand) -> CommandResult {
    let is_undoable = !matches!(cmd, SceneCommand::SelectObject { .. });
    let pre_state = is_undoable.then(|| state.scene.capture_history_snapshot());

    // Reborrowed (not the outer `state` itself) so the `move` closure below
    // can take ownership of this reborrow without moving the actual `state`
    // parameter -- which is used again after the closure returns, to commit
    // the checkpoint captured above.
    let state_ref = &mut *state;
    let result = (move || -> CommandResult {
        let state = state_ref;
        match cmd {
            SceneCommand::AddObject { data, parent_id } => {
                let id = state.scene.database.add_object(data, parent_id);
                if id.is_empty() {
                    return CommandResult::noop("Object could not be added");
                }
                state.scene.bump_revision(true);
                CommandResult::ok(vec![id])
            }

            SceneCommand::RemoveObject { ref id } => {
                let removed = state.scene.database.remove_object(id);
                if removed {
                    if state.scene.database.get_selected_object_id().as_deref() == Some(id) {
                        state.scene.database.select_object(None);
                    }
                    state.scene.bump_revision(true);
                    CommandResult::ok(vec![id.clone()])
                } else {
                    CommandResult::noop("Object not found")
                }
            }

            SceneCommand::UpdateObject { data } => {
                let id = data.id.clone();
                if state.scene.database.update_object(data) {
                    state.scene.bump_revision(true);
                    CommandResult::ok(vec![id])
                } else {
                    CommandResult::noop("Object not found")
                }
            }

            SceneCommand::ReparentObject {
                ref id,
                ref new_parent_id,
            } => {
                let moved = state
                    .scene
                    .database
                    .reparent_object(id, new_parent_id.clone());
                if moved {
                    state.scene.bump_revision(true);
                    CommandResult::ok(vec![id.clone()])
                } else {
                    CommandResult::noop("Object not found or reparent rejected")
                }
            }

            SceneCommand::DuplicateObject {
                ref source_id,
                count,
                position_offset,
            } => {
                let src_pos = state
                    .scene
                    .database
                    .get_object(source_id)
                    .map(|o| o.transform.position);
                let mut created = Vec::new();
                for i in 0..count {
                    if let Some(new_id) = state.scene.database.duplicate_object(source_id) {
                        if let (Some(off), Some(src)) = (position_offset, src_pos) {
                            let n = (i + 1) as f32;
                            if let Some(mut copy) = state.scene.database.get_object(&new_id) {
                                copy.transform.position = [
                                    src[0] + off[0] * n,
                                    src[1] + off[1] * n,
                                    src[2] + off[2] * n,
                                ];
                                state.scene.database.update_object(copy);
                            }
                        }
                        created.push(new_id);
                    } else {
                        break;
                    }
                }
                if created.is_empty() {
                    CommandResult::noop("Source object not found")
                } else {
                    state.scene.bump_revision(true);
                    CommandResult::ok(created)
                }
            }

            SceneCommand::SelectObject { id } => {
                state.scene.database.select_object(id.clone());
                state.scene.bump_revision(false);
                CommandResult::ok(id.into_iter().collect())
            }

            SceneCommand::SetTransform {
                ref id,
                position,
                rotation,
                scale,
            } => {
                // `SceneDatabase::set_transform`, NOT `get_object`+`update_object`
                // (Pulsar-Native#561): the old whole-object round trip triggered
                // `sync_registered_component_props_to_scene_db` -- a full
                // re-serialize/re-hydrate of every component on the object --
                // on every keystroke of a position/rotation/scale field, for a
                // change that has nothing to do with component data at all.
                if state
                    .scene
                    .database
                    .set_transform(id, position, rotation, scale)
                {
                    state.scene.bump_revision(true);
                    CommandResult::ok(vec![id.clone()])
                } else {
                    CommandResult::noop("Object not found or no transform fields changed")
                }
            }

            SceneCommand::SetName { ref id, name } => {
                if state.scene.database.set_name(id, name) {
                    state.scene.bump_revision(true);
                    CommandResult::ok(vec![id.clone()])
                } else {
                    CommandResult::noop("Object not found")
                }
            }

            SceneCommand::SetVisibility {
                ref id,
                visible,
                locked,
            } => {
                let mut changed = false;
                if let Some(v) = visible {
                    changed |= state.scene.database.set_visible(id, v);
                }
                if let Some(l) = locked {
                    changed |= state.scene.database.set_locked(id, l);
                }
                if changed {
                    state.scene.bump_revision(true);
                    CommandResult::ok(vec![id.clone()])
                } else {
                    CommandResult::noop("Object not found or no visibility fields changed")
                }
            }

            SceneCommand::SetComponentProperty {
                ref id,
                ref class_name,
                component_index,
                ref prop_name,
                value,
            } => {
                // Typed path first -- `update_live_component_property` writes
                // `value` straight onto the live `World` component via its
                // reflected setter closure, no JSON anywhere (Pulsar-Native#561).
                //
                // `component_index` targets the exact instance being edited
                // (Pulsar-Native#519): only the object's first enabled instance
                // of a World-registered class is live-typed in `World` (one
                // storage slot per `(entity, type)`), so that instance takes the
                // typed path and every OTHER instance -- plus classes with no
                // World registration at all (`LODComponent`/
                // `MaterialOverrideComponent`) -- is handled inside as an
                // indexed metadata_db JSON write, so each duplicate keeps its
                // own field values instead of every edit landing in instance 0.
                match state.scene.database.update_live_component_property(
                    id,
                    class_name,
                    component_index,
                    prop_name,
                    value,
                ) {
                    Ok(()) => {
                        state.scene.bump_revision(true);
                        CommandResult::ok(vec![id.clone()])
                    }
                    Err(value) => {
                        match pulsar_reflection::RUNTIME_TYPE_REGISTRY
                            .serialize_json_for_any(value.as_ref())
                        {
                            Ok(value_json) => {
                                state.scene.database.update_component_property(
                                    id, class_name, prop_name, value_json,
                                );
                                state.scene.bump_revision(true);
                                CommandResult::ok(vec![id.clone()])
                            }
                            Err(error) => {
                                // Not `World`-registered AND not in
                                // `RUNTIME_TYPE_REGISTRY` either -- nothing this
                                // command can do with the value. Surfaced loudly
                                // rather than silently dropping the edit: this
                                // should only happen for a genuinely new/
                                // misconfigured property type, not real usage.
                                tracing::error!(
                                "[SetComponentProperty] '{class_name}.{prop_name}' on '{id}' has \
                                 no live World value and its type isn't registered for JSON \
                                 fallback either -- edit dropped: {error}"
                            );
                                CommandResult::noop(
                                    "Property type not registered for World or JSON fallback",
                                )
                            }
                        }
                    }
                }
            }
        }
    })();

    if result.changed {
        if let Some(pre) = pre_state {
            state.scene.commit_undo_checkpoint(pre);
        }
    }
    result
}
