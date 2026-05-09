/// Scene command system — single execution path for all scene mutations.
///
/// Both user-initiated GPUI actions and AI tool calls route through
/// `execute_command()`.  This guarantees:
///   • One place to add undo/redo later.
///   • Consistent has_unsaved_changes / scene_revision tracking.
///   • AI tools never hold the state write-lock during complex logic;
///     they build a `SceneCommand` first, then apply it atomically.

use serde_json::Value;

use crate::level_editor::scene_database::{ObjectType, SceneObjectData, Transform};
use crate::level_editor::LevelEditorState;

// ── Command types ─────────────────────────────────────────────────────────────

/// A self-contained scene mutation.  All fields use owned data so the command
/// can be constructed on a background thread and executed on the UI thread.
#[derive(Debug, Clone)]
pub enum SceneCommand {
    /// Add a new object.  `id` in `data` is ignored — the DB assigns one.
    AddObject {
        data: SceneObjectData,
        parent_id: Option<String>,
    },
    /// Remove an object and all descendants.
    RemoveObject { id: String },
    /// Overwrite all fields of an existing object (by id).
    UpdateObject { data: SceneObjectData },
    /// Move an object to a different parent (or root).
    ReparentObject {
        id: String,
        new_parent_id: Option<String>,
    },
    /// Clone an existing object `count` times, optionally spacing copies.
    /// `position_offset` is applied cumulatively: copy i → src_pos + offset * i.
    DuplicateObject {
        source_id: String,
        count: usize,
        position_offset: Option<[f32; 3]>,
    },
    /// Change the editor selection (None clears it).
    SelectObject { id: Option<String> },
    /// Set absolute world-space transform fields on an object.
    /// Fields left as `None` are unchanged.
    SetTransform {
        id: String,
        position: Option<[f32; 3]>,
        rotation: Option<[f32; 3]>,
        scale: Option<[f32; 3]>,
    },
}

// ── Result ────────────────────────────────────────────────────────────────────

/// Outcome of executing a `SceneCommand`.
#[derive(Debug)]
pub struct CommandResult {
    /// Whether any state changed (false → no-op, no scene_revision bump).
    pub changed: bool,
    /// ID(s) of objects created or affected.
    pub affected_ids: Vec<String>,
    /// Human-readable reason for a no-op, for AI tool response payloads.
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

/// Apply `cmd` to `state`.  Bumps `scene_revision` and sets
/// `has_unsaved_changes` when the scene is actually modified.
///
/// Callers on the GPUI thread should follow up with `cx.notify()`.
/// Callers on background threads (AI tools) rely on the revision-polling
/// task in `LevelEditorPanel` to propagate the notification.
pub fn execute_command(state: &mut LevelEditorState, cmd: SceneCommand) -> CommandResult {
    match cmd {
        SceneCommand::AddObject { data, parent_id } => {
            let new_id = state.scene_database.add_object(data, parent_id);
            bump(state, true);
            CommandResult::ok(vec![new_id])
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
            let updated = state.scene_database.update_object(data.clone());
            if updated {
                bump(state, true);
                CommandResult::ok(vec![data.id])
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
            let src_pos = state
                .scene_database
                .get_object(source_id)
                .map(|o| o.transform.position);

            let mut created = Vec::new();
            for i in 0..count {
                if let Some(new_id) = state.scene_database.duplicate_object(source_id) {
                    if let (Some(offset), Some(src)) = (position_offset, src_pos) {
                        let n = (i + 1) as f32;
                        if let Some(mut copy) = state.scene_database.get_object(&new_id) {
                            copy.transform.position = [
                                src[0] + offset[0] * n,
                                src[1] + offset[1] * n,
                                src[2] + offset[2] * n,
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

            if changed && state.scene_database.update_object(obj.clone()) {
                bump(state, true);
                CommandResult::ok(vec![id.clone()])
            } else if !changed {
                CommandResult::noop("No transform fields changed")
            } else {
                CommandResult::noop("Transform update failed in scene database")
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
