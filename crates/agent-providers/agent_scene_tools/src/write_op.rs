//! A validated scene write as data.
//!
//! Every `scene_write_*` tool parses its arguments into a [`WriteOp`] and
//! executes it through [`WriteOp::exec`]. Live runs execute it at once; an
//! overlay sandbox keeps the op and replays it later (see
//! [`crate::overlay`]), which only works because an op is deterministic once
//! executed: [`WriteOp::exec`] fixes everything generated while running (the
//! id of a spawned object) into the op itself.

use anyhow::Result;
use serde_json::{json, Value};

use crate::journal::Recorder;
use crate::ops::{self, ComponentEdit, ComponentSpec, ObjectEdit, SpawnSpec, TransformEdit};
use crate::source::SceneSource;
use pulsar_scenedb::World;

#[derive(Clone, Debug)]
pub enum WriteOp {
    Spawn(SpawnSpec),
    SetObject {
        id: String,
        edit: ObjectEdit,
    },
    Transform {
        ids: Vec<String>,
        edit: TransformEdit,
    },
    AddComponent {
        id: String,
        spec: ComponentSpec,
    },
    SetComponent {
        id: String,
        component_index: Option<usize>,
        class_name: Option<String>,
        properties: Option<Value>,
        enabled: Option<bool>,
    },
    RemoveComponent {
        id: String,
        component_index: Option<usize>,
        class_name: Option<String>,
    },
    Delete {
        ids: Vec<String>,
    },
}

impl WriteOp {
    /// The tool this op came from, for the journal.
    pub fn tool(&self) -> &'static str {
        match self {
            WriteOp::Spawn(_) => "scene_write_spawn_object",
            WriteOp::SetObject { .. } => "scene_write_set_object",
            WriteOp::Transform { .. } => "scene_write_transform",
            WriteOp::AddComponent { .. } => "scene_write_add_component",
            WriteOp::SetComponent { .. } => "scene_write_set_component",
            WriteOp::RemoveComponent { .. } => "scene_write_remove_component",
            WriteOp::Delete { .. } => "scene_write_delete_object",
        }
    }

    /// Run the op against `world`, recording what it replaced in `rec`.
    /// Returns the tool's result fields and a one-line summary. On success a
    /// spawn without an id is pinned to the id it got, so running the op
    /// again produces the same object.
    pub fn exec(
        &mut self,
        world: &mut World,
        rec: &mut Recorder,
        rules: &dyn SceneSource,
    ) -> Result<(Value, String)> {
        match self {
            WriteOp::Spawn(spec) => {
                let name = spec.name.clone();
                let id = ops::spawn(world, rec, rules, spec.clone())?;
                spec.id = Some(id.clone());
                let summary = format!("spawned '{id}' (\"{name}\")");
                Ok((json!({ "id": id }), summary))
            }
            WriteOp::SetObject { id, edit } => {
                ops::edit_object(world, rec, id, edit.clone())?;
                Ok((json!({ "id": id }), format!("edited object '{id}'")))
            }
            WriteOp::Transform { ids, edit } => {
                let changed = ops::transform_objects(world, rec, ids, edit)?;
                let summary = format!("transformed {}", changed.join(", "));
                Ok((json!({ "changed": changed }), summary))
            }
            WriteOp::AddComponent { id, spec } => {
                let index = ops::add_component(world, rec, rules, id, spec)?;
                Ok((
                    json!({ "id": id, "class_name": spec.class_name, "component_index": index }),
                    format!("added {} to '{id}'", spec.class_name),
                ))
            }
            WriteOp::SetComponent { id, component_index, class_name, properties, enabled } => {
                let (index, class) = ops::edit_component(
                    world,
                    rec,
                    rules,
                    id,
                    ComponentEdit {
                        component_index: *component_index,
                        class_name: class_name.as_deref(),
                        properties: properties.as_ref(),
                        enabled: *enabled,
                    },
                )?;
                Ok((
                    json!({ "id": id, "class_name": class, "component_index": index }),
                    format!("edited {class} on '{id}'"),
                ))
            }
            WriteOp::RemoveComponent { id, component_index, class_name } => {
                let class = ops::remove_component(
                    world,
                    rec,
                    id,
                    *component_index,
                    class_name.as_deref(),
                )?;
                let summary = format!("removed {class} from '{id}'");
                Ok((json!({ "id": id, "removed": class }), summary))
            }
            WriteOp::Delete { ids } => {
                let removed = ops::delete_objects(world, rec, ids)?;
                Ok((
                    json!({ "deleted": ids, "objects_removed": removed }),
                    format!("deleted {} (with descendants: {removed} object(s))", ids.join(", ")),
                ))
            }
        }
    }
}
