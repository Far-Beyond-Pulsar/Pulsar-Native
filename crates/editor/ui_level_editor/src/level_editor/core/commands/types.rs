use crate::level_editor::scene_edit::SceneObjectData;
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