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
use crate::level_editor::LevelEditorState;

// ── Command types ─────────────────────────────────────────────────────────────

/// A self-contained scene mutation.  All fields use owned data so the command
/// can be constructed on a background thread and executed on the UI thread.
#[derive(Debug, Clone)]
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
        Self { changed: false, affected_ids: vec![], no_op_reason: reason }
    }
    pub fn ok(ids: Vec<String>) -> Self {
        Self { changed: true, affected_ids: ids, no_op_reason: "" }
    }
}

// ── Executor ──────────────────────────────────────────────────────────────────

/// Apply `cmd` to `state`.
///
/// Mutations go through `state.scene_database`, which writes to **both**
/// `SceneDb` and the Helio renderer atomically.  `scene_revision` is bumped
/// on every mutation, causing the polling task in `LevelEditorPanel` to notify
/// the GPUI hierarchy and properties panels.
///
/// GPUI-thread callers (panel action handlers) should additionally call
/// `cx.notify()` after this returns.
pub fn execute_command(state: &mut LevelEditorState, cmd: SceneCommand) -> CommandResult {
    match cmd {
        SceneCommand::AddObject { data, parent_id } => {
            let id = state.scene_database.add_object(data, parent_id);
            bump(state, true);
            CommandResult::ok(vec![id])
        }

        SceneCommand::RemoveObject { ref id } => {
            let removed = state.scene_database.remove_object(id);
            if removed {
                if state.scene_database.get_selected_object_id().as_deref() == Some(id) {
                    state.scene_database.select_object(None);
                }
                bump(state, true);
                CommandResult::ok(vec![id.clone()])
            } else {
                CommandResult::noop("Object not found")
            }
        }

        SceneCommand::UpdateObject { data } => {
            let id = data.id.clone();
            if state.scene_database.update_object(data) {
                bump(state, true);
                CommandResult::ok(vec![id])
            } else {
                CommandResult::noop("Object not found")
            }
        }

        SceneCommand::ReparentObject { ref id, ref new_parent_id } => {
            let moved = state.scene_database.reparent_object(id, new_parent_id.clone());
            if moved {
                bump(state, true);
                CommandResult::ok(vec![id.clone()])
            } else {
                CommandResult::noop("Object not found or reparent rejected")
            }
        }

        SceneCommand::DuplicateObject { ref source_id, count, position_offset } => {
            let src_pos = state.scene_database.get_object(source_id).map(|o| o.transform.position);
            let mut created = Vec::new();
            for i in 0..count {
                if let Some(new_id) = state.scene_database.duplicate_object(source_id) {
                    if let (Some(off), Some(src)) = (position_offset, src_pos) {
                        let n = (i + 1) as f32;
                        if let Some(mut copy) = state.scene_database.get_object(&new_id) {
                            copy.transform.position = [
                                src[0] + off[0] * n,
                                src[1] + off[1] * n,
                                src[2] + off[2] * n,
                            ];
                            state.scene_database.update_object(copy);
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
                bump(state, true);
                CommandResult::ok(created)
            }
        }

        SceneCommand::SelectObject { id } => {
            state.scene_database.select_object(id.clone());
            bump(state, false);
            CommandResult::ok(id.into_iter().collect())
        }

        SceneCommand::SetTransform { ref id, position, rotation, scale } => {
            let Some(mut obj) = state.scene_database.get_object(id) else {
                return CommandResult::noop("Object not found");
            };
            let mut changed = false;
            if let Some(p) = position { if obj.transform.position != p { obj.transform.position = p; changed = true; } }
            if let Some(r) = rotation { if obj.transform.rotation != r { obj.transform.rotation = r; changed = true; } }
            if let Some(s) = scale    { if obj.transform.scale != s    { obj.transform.scale = s;    changed = true; } }

            if !changed {
                return CommandResult::noop("No transform fields changed");
            }
            if state.scene_database.update_object(obj.clone()) {
                bump(state, true);
                CommandResult::ok(vec![id.clone()])
            } else {
                CommandResult::noop("Transform update failed")
            }
        }
    }
}

fn bump(state: &mut LevelEditorState, marks_unsaved: bool) {
    state.scene_revision = state.scene_revision.saturating_add(1);
    if marks_unsaved {
        state.has_unsaved_changes = true;
    }
}
