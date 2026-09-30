use crate::scene_edit::SceneObjectData;
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
    /// Place an instance of the class in `class_dir` (#921): the root with
    /// its `ClassInstance`, every prefab component and generated children.
    InstantiateClass {
        class_dir: std::path::PathBuf,
        transform: crate::scene_edit::Transform,
        parent_id: Option<String>,
    },
    /// Put one property of a component built from a class slot back to the
    /// class default (#921). The default is read from the current class
    /// definition; the write takes the same path as `SetComponentProperty`.
    RevertComponentProperty {
        id: String,
        class_name: String,
        component_index: usize,
        prop_name: String,
    },
    /// Set a placed class instance's script variable (`Some`), stored as an
    /// override only when it differs from the class default, or revert it
    /// to the class default (`None`).
    SetClassVariable {
        id: String,
        name: String,
        value: Option<serde_json::Value>,
    },
    RemoveObject {
        id: String,
    },
    /// Overwrite all mutable fields of an existing object (looked up by `data.id`).
    UpdateObject {
        data: SceneObjectData,
    },
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
    SelectObject {
        id: Option<String>,
    },
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
    SetName {
        id: String,
        name: String,
    },
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
    /// Attach a new component instance of `class_name`. `data` is the
    /// class's whole-instance JSON (the `EngineClass::to_json` shape,
    /// `#[sub_props]` nesting included).
    AddComponent {
        id: String,
        class_name: String,
        data: serde_json::Value,
    },
    /// Detach the component at `component_index`.
    RemoveComponent {
        id: String,
        component_index: usize,
    },
    /// Enable or disable the component at `component_index`.
    SetComponentEnabled {
        id: String,
        component_index: usize,
        enabled: bool,
    },
    /// Copy the component at `component_index`; the copy lands right after it.
    DuplicateComponent {
        id: String,
        component_index: usize,
    },
    /// Move a component from `from_index` to `to_index` in the object's list.
    ReorderComponent {
        id: String,
        from_index: usize,
        to_index: usize,
    },
    /// Nest a component under another one on the same object (`None` = top level).
    SetComponentParent {
        id: String,
        component_index: usize,
        parent_index: Option<usize>,
    },
    /// Replace one component instance's whole data (same shape as
    /// `AddComponent::data`). For callers holding JSON rather than a typed
    /// widget value -- the AI tools -- so nested fields need no per-property
    /// setter lookup.
    SetComponentData {
        id: String,
        component_index: usize,
        data: serde_json::Value,
    },
    /// Revert a placed class instance's slot to the class: one property
    /// (dot `path` into the component data) or, with `path: None`, the whole
    /// slot (which also restores a slot the instance removed).
    RevertClassSlot {
        id: String,
        slot_id: String,
        path: Option<String>,
    },
    /// Put a placed class instance entirely back to its class: every
    /// variable override and every slot override (including removed slots,
    /// and slots on generated children). One undo step.
    ResetClassOverrides {
        id: String,
    },
    /// Set the `movability` of every mesh and light component on each of
    /// `ids` (Pulsar-Native#837: "Mark selection Static"). One undo step;
    /// objects with neither component are skipped.
    SetMovability {
        ids: Vec<String>,
        movability: helio_component::components::ObjectMovability,
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
            Self::InstantiateClass {
                class_dir,
                parent_id,
                ..
            } => f
                .debug_struct("InstantiateClass")
                .field("class_dir", class_dir)
                .field("parent_id", parent_id)
                .finish(),
            Self::RevertComponentProperty {
                id,
                class_name,
                component_index,
                prop_name,
            } => f
                .debug_struct("RevertComponentProperty")
                .field("id", id)
                .field("class_name", class_name)
                .field("component_index", component_index)
                .field("prop_name", prop_name)
                .finish(),
            Self::SetClassVariable { id, name, value } => f
                .debug_struct("SetClassVariable")
                .field("id", id)
                .field("name", name)
                .field("value", value)
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
            Self::AddComponent { id, class_name, .. } => f
                .debug_struct("AddComponent")
                .field("id", id)
                .field("class_name", class_name)
                .finish(),
            Self::RemoveComponent {
                id,
                component_index,
            } => f
                .debug_struct("RemoveComponent")
                .field("id", id)
                .field("component_index", component_index)
                .finish(),
            Self::SetComponentEnabled {
                id,
                component_index,
                enabled,
            } => f
                .debug_struct("SetComponentEnabled")
                .field("id", id)
                .field("component_index", component_index)
                .field("enabled", enabled)
                .finish(),
            Self::DuplicateComponent {
                id,
                component_index,
            } => f
                .debug_struct("DuplicateComponent")
                .field("id", id)
                .field("component_index", component_index)
                .finish(),
            Self::ReorderComponent {
                id,
                from_index,
                to_index,
            } => f
                .debug_struct("ReorderComponent")
                .field("id", id)
                .field("from_index", from_index)
                .field("to_index", to_index)
                .finish(),
            Self::SetComponentParent {
                id,
                component_index,
                parent_index,
            } => f
                .debug_struct("SetComponentParent")
                .field("id", id)
                .field("component_index", component_index)
                .field("parent_index", parent_index)
                .finish(),
            Self::SetComponentData {
                id,
                component_index,
                ..
            } => f
                .debug_struct("SetComponentData")
                .field("id", id)
                .field("component_index", component_index)
                .finish(),
            Self::ResetClassOverrides { id } => f
                .debug_struct("ResetClassOverrides")
                .field("id", id)
                .finish(),
            Self::SetMovability { ids, movability } => f
                .debug_struct("SetMovability")
                .field("ids", ids)
                .field("movability", movability)
                .finish(),
            Self::RevertClassSlot { id, slot_id, path } => f
                .debug_struct("RevertClassSlot")
                .field("id", id)
                .field("slot_id", slot_id)
                .field("path", path)
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
