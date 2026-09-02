/// Scene command system — single execution path for all scene mutations.
///
/// `SceneCommand` is a self-contained description of one editor operation.
/// `execute_command()` applies it through `SceneDatabase`, which in turn
/// writes to **both** `SceneDb` (the canonical store) and the Helio renderer
/// (immediate viewport update) in one call.
///
/// Both user GPUI action handlers and AI tool implementations call
/// `execute_command()`, giving a single auditable code path that is ready for
/// undo / redo to be layered on top.
use crate::level_editor::scene_database::SceneObjectData;
use crate::level_editor::state::LevelEditorState;
use std::any::Any;

// ── Command types ─────────────────────────────────────────────────────────────

/// A self-contained scene mutation.  All fields use owned data so the command
/// can be constructed on a background thread and executed on the UI thread.
///
/// Deliberately NOT `Clone` or `#[derive(Debug)]` (Pulsar-Native#561): no call
/// site anywhere in the codebase clones a `SceneCommand` value or
/// `{:?}`-prints one (checked -- every `execute_command` caller constructs a
/// command and immediately passes it by value; `undo`/`redo` are snapshot-
/// based, not command-replay-based, so they never touch `SceneCommand` at
/// all -- see `state/scene.rs`). Those derives existed only to justify
/// `SetComponentProperty` carrying `serde_json::Value` instead of the typed
/// `Box<dyn Any + Send>` the properties panel actually produces -- a JSON
/// round trip inserted into the live edit path purely to satisfy a trait
/// bound nothing downstream needed. A hand-written `Debug` impl below prints
/// enough to be useful in logs without requiring the payload itself to be
/// `Debug` (`Box<dyn Any>` isn't, and boxing a closure to fake it would be
/// its own complexity for zero real benefit).
pub enum SceneCommand {
    /// Add a new object.  The `id` field in `data` is ignored — SceneDb assigns it.
    AddObject {
        data: SceneObjectData,
        parent_id: Option<String>,
    },
    /// Remove an object and all descendants.
    RemoveObject { id: String },
    /// Overwrite all mutable fields of an existing object (looked up by `data.id`).
    UpdateObject { data: SceneObjectData },
    /// Move an object to a different parent (or root when `None`).
    ReparentObject {
        id: String,
        new_parent_id: Option<String>,
    },
    /// Clone an object `count` times.
    /// `position_offset` is applied cumulatively: copy i is at src_pos + offset × i.
    DuplicateObject {
        source_id: String,
        count: usize,
        position_offset: Option<[f32; 3]>,
    },
    /// Change the editor selection (`None` clears it).
    SelectObject { id: Option<String> },
    /// Set absolute world-space transform fields; `None` fields are unchanged.
    SetTransform {
        id: String,
        position: Option<[f32; 3]>,
        rotation: Option<[f32; 3]>,
        scale: Option<[f32; 3]>,
    },
    /// Rename an object.
    ///
    /// Pulsar-Native#561: added so the properties panel's name field can go
    /// through `execute_command` (undo-tracked) like every other edit,
    /// instead of calling `SceneDatabase::update_object` (whole-object
    /// overwrite, NOT undo-tracked despite a comment that used to claim
    /// otherwise) directly.
    SetName { id: String, name: String },
    /// Set an object's visible/locked flags; `None` fields are unchanged.
    ///
    /// Pulsar-Native#561, same reasoning as `SetName`.
    SetVisibility {
        id: String,
        visible: Option<bool>,
        locked: Option<bool>,
    },
    /// Set a single property on a reflected component, by class + property
    /// name, carrying the widget-produced value as `Box<dyn Any + Send>` --
    /// exactly what `update_live_component_property` needs, with zero JSON
    /// in between (Pulsar-Native#561: the previous `value_json:
    /// serde_json::Value` shape round-tripped every edit through
    /// `RUNTIME_TYPE_REGISTRY.serialize_json_for_any`/`deserialize_json_for_type`
    /// for no reason a real caller needed -- see this enum's top doc).
    ///
    /// The single, unified write path for every component-property edit in
    /// the properties panel -- replaces calling
    /// `SceneDatabase::update_live_component_property`/
    /// `update_component_property` directly from UI code, so every such
    /// edit is undo-tracked and goes through exactly one code path.
    ///
    /// `component_index` identifies WHICH instance of `class_name` is being
    /// edited (Pulsar-Native#519): an object can carry several instances of
    /// the same class, each with its own field values, and they are
    /// addressed by their index in the object's component list -- the same
    /// identity `remove_component`/`set_component_enabled`/
    /// `reorder_component` already use.
    SetComponentProperty {
        id: String,
        class_name: String,
        component_index: usize,
        prop_name: String,
        value: Box<dyn Any + Send>,
    },
}

impl std::fmt::Debug for SceneCommand {
    /// Hand-written because `SetComponentProperty`'s payload is
    /// `Box<dyn Any + Send>`, which isn't `Debug` -- see this type's doc for
    /// why that's the right trade (nothing needs `SceneCommand: Debug` for
    /// more than an occasional log line, and nothing needs `Clone` at all).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AddObject { data, parent_id } => f
                .debug_struct("AddObject")
                .field("data.id", &data.id)
                .field("data.name", &data.name)
                .field("parent_id", parent_id)
                .finish(),
            Self::RemoveObject { id } => f.debug_struct("RemoveObject").field("id", id).finish(),
            Self::UpdateObject { data } => f
                .debug_struct("UpdateObject")
                .field("data.id", &data.id)
                .finish(),
            Self::ReparentObject { id, new_parent_id } => f
                .debug_struct("ReparentObject")
                .field("id", id)
                .field("new_parent_id", new_parent_id)
                .finish(),
            Self::DuplicateObject {
                source_id,
                count,
                position_offset,
            } => f
                .debug_struct("DuplicateObject")
                .field("source_id", source_id)
                .field("count", count)
                .field("position_offset", position_offset)
                .finish(),
            Self::SelectObject { id } => f.debug_struct("SelectObject").field("id", id).finish(),
            Self::SetTransform {
                id,
                position,
                rotation,
                scale,
            } => f
                .debug_struct("SetTransform")
                .field("id", id)
                .field("position", position)
                .field("rotation", rotation)
                .field("scale", scale)
                .finish(),
            Self::SetName { id, name } => f
                .debug_struct("SetName")
                .field("id", id)
                .field("name", name)
                .finish(),
            Self::SetVisibility {
                id,
                visible,
                locked,
            } => f
                .debug_struct("SetVisibility")
                .field("id", id)
                .field("visible", visible)
                .field("locked", locked)
                .finish(),
            Self::SetComponentProperty {
                id,
                class_name,
                component_index,
                prop_name,
                value,
            } => f
                .debug_struct("SetComponentProperty")
                .field("id", id)
                .field("class_name", class_name)
                .field("component_index", component_index)
                .field("prop_name", prop_name)
                .field("value_type", &value.type_id())
                .finish(),
        }
    }
}

// ── Outcome ───────────────────────────────────────────────────────────────────

/// Outcome of executing a `SceneCommand`.
#[derive(Debug)]
pub struct CommandResult {
    /// Whether any state was actually modified.
    pub changed: bool,
    /// IDs of objects that were created or meaningfully affected.
    pub affected_ids: Vec<String>,
    /// Human-readable reason when `changed` is false.
    pub no_op_reason: &'static str,
}

impl CommandResult {
    pub fn noop(reason: &'static str) -> Self {
        Self {
            changed: false,
            affected_ids: vec![],
            no_op_reason: reason,
        }
    }
    pub fn ok(ids: Vec<String>) -> Self {
        Self {
            changed: true,
            affected_ids: ids,
            no_op_reason: "",
        }
    }
}

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

#[cfg(test)]
mod undo_redo_tests {
    use super::*;
    use crate::level_editor::scene_database::{ObjectType, SceneObjectData, Transform};

    fn object(name: &str) -> SceneObjectData {
        SceneObjectData {
            id: String::new(),
            name: name.to_string(),
            object_type: ObjectType::Empty,
            transform: Transform::default(),
            visible: true,
            locked: false,
            parent: None,
            children: vec![],
            scene_path: String::new(),
            props: Default::default(),
            component_instances: None,
        }
    }

    #[test]
    fn undo_reverts_an_add_object_command() {
        let mut state = LevelEditorState::new();
        assert!(!state.scene.can_undo());

        let result = execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: object("Cube"),
                parent_id: None,
            },
        );
        assert!(result.changed);
        assert!(state.scene.can_undo());
        assert_eq!(state.scene.database.get_all_objects().len(), 1);

        assert!(state.scene.undo());
        assert!(state.scene.database.get_all_objects().is_empty());
        assert!(!state.scene.can_undo());
        assert!(state.scene.can_redo());
    }

    #[test]
    fn redo_reapplies_an_undone_command() {
        let mut state = LevelEditorState::new();
        execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: object("Cube"),
                parent_id: None,
            },
        );
        state.scene.undo();
        assert!(state.scene.database.get_all_objects().is_empty());

        assert!(state.scene.redo());

        assert_eq!(state.scene.database.get_all_objects().len(), 1);
        assert!(state.scene.can_undo());
        assert!(!state.scene.can_redo());
    }

    #[test]
    fn a_new_mutating_command_clears_the_redo_stack() {
        let mut state = LevelEditorState::new();
        execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: object("A"),
                parent_id: None,
            },
        );
        state.scene.undo();
        assert!(state.scene.can_redo());

        execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: object("B"),
                parent_id: None,
            },
        );

        assert!(!state.scene.can_redo());
    }

    #[test]
    fn a_noop_command_does_not_push_an_undo_checkpoint() {
        let mut state = LevelEditorState::new();
        let result = execute_command(
            &mut state,
            SceneCommand::RemoveObject {
                id: "nope".to_string(),
            },
        );
        assert!(!result.changed);
        assert!(!state.scene.can_undo());
    }

    #[test]
    fn selecting_an_object_does_not_push_an_undo_checkpoint() {
        let mut state = LevelEditorState::new();
        let add = execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: object("Cube"),
                parent_id: None,
            },
        );
        let id = add.affected_ids[0].clone();
        assert!(state.scene.can_undo()); // the AddObject checkpoint

        execute_command(&mut state, SceneCommand::SelectObject { id: Some(id) });

        // Still exactly the one checkpoint from AddObject -- undoing once
        // now must remove the object, not merely revert the selection.
        assert!(state.scene.undo());
        assert!(state.scene.database.get_all_objects().is_empty());
        assert!(!state.scene.can_undo());
    }

    #[test]
    fn undo_and_redo_on_an_empty_history_are_no_ops() {
        let mut state = LevelEditorState::new();
        assert!(!state.scene.undo());
        assert!(!state.scene.redo());
    }

    // ── SetComponentProperty: end-to-end through the command layer ─────────
    //
    // Pulsar-Native#561: proves the whole live-edit call graph a real
    // properties-panel color/intensity edit takes -- widget produces a typed
    // `Box<dyn Any + Send>`, `SceneCommand::SetComponentProperty` carries it
    // unchanged, `execute_command` applies it -- actually reaches the live
    // `World` value and is undo-tracked, with no `serde_json::Value`
    // anywhere on this call path (unlike the tests in `scene_database.rs`,
    // which exercise `SceneDatabase` methods directly, this goes through the
    // actual `SceneCommand` enum + `execute_command` UI code calls).
    #[test]
    fn set_component_property_reaches_the_live_world_value_with_no_json() {
        let mut state = LevelEditorState::new();
        let id = execute_command(
            &mut state,
            SceneCommand::AddObject {
                data: SceneObjectData {
                    id: String::new(),
                    name: "Light".to_string(),
                    object_type: crate::level_editor::scene_database::ObjectType::Light(
                        crate::level_editor::scene_database::LightType::Point,
                    ),
                    transform: crate::level_editor::scene_database::Transform::default(),
                    visible: true,
                    locked: false,
                    parent: None,
                    children: vec![],
                    scene_path: String::new(),
                    props: Default::default(),
                    component_instances: None,
                },
                parent_id: None,
            },
        )
        .affected_ids[0]
            .clone();

        let default_light_json =
            serde_json::to_value(helio_component::LightComponent::default()).unwrap();
        state
            .scene
            .database
            .add_component(&id, "LightComponent".to_string(), default_light_json);

        // The widget layer's actual contract: a boxed, already-typed value --
        // never JSON. `intensity` is a leaf of the `#[sub_props]`-nested
        // `IntensityLightProps`, so this also exercises the nested getter/
        // setter closure chain, not just a top-level field.
        let result = execute_command(
            &mut state,
            SceneCommand::SetComponentProperty {
                id: id.clone(),
                class_name: "LightComponent".to_string(),
                component_index: 0,
                prop_name: "intensity".to_string(),
                value: Box::new(4242.0_f32),
            },
        );
        assert!(
            result.changed,
            "typed live write must succeed, not fall through to the JSON path"
        );

        let live = state
            .scene
            .database
            .read_live_component_property(&id, "LightComponent", "intensity")
            .expect("intensity must be live-readable after the edit");
        assert_eq!(live.downcast_ref::<f32>(), Some(&4242.0));

        // Undo-tracked like every other command: restoring the pre-edit
        // snapshot must revert the live World value too, not just
        // `metadata_db`'s mirror.
        assert!(state.scene.undo());
        let reverted = state
            .scene
            .database
            .read_live_component_property(&id, "LightComponent", "intensity")
            .expect("intensity must still be live-readable after undo");
        assert_eq!(reverted.downcast_ref::<f32>(), Some(&1000.0)); // IntensityLightProps::default()
    }
}
