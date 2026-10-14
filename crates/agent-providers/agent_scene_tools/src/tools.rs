//! The scene tools the model sees.
//!
//! Naming: tools that only read are `scene_*`; tools that change the scene
//! are `scene_write_*`. Harness permissions key off the prefix
//! (`group:scene` selects them all, read-only harnesses lose the
//! `scene_write_*` ones) and the run extension classifies them read / write
//! for confirmation and taint.

use std::sync::Arc;

use agent_chat_tools::{run_scope, AgentRunScope, ToolContext, ToolRegistry};
use anyhow::{anyhow, bail, Result};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::ops::{self, ComponentEdit, ComponentSpec, ObjectEdit, QueryFilter, SpawnSpec, TransformEdit};
use crate::snapshot::SceneIndex;
use crate::toolbox::SceneToolbox;

pub const READ_TOOLS: [&str; 4] = [
    "scene_query",
    "scene_get_object",
    "scene_list_component_classes",
    "scene_describe_component",
];

pub const WRITE_TOOLS: [&str; 9] = [
    "scene_write_spawn_object",
    "scene_write_set_object",
    "scene_write_transform",
    "scene_write_add_component",
    "scene_write_set_component",
    "scene_write_remove_component",
    "scene_write_delete_object",
    "scene_write_apply_sandbox",
    "scene_write_discard_sandbox",
];

const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;

fn parse_args<T: DeserializeOwned>(tool: &str, args: Value, usage: &str) -> Result<T> {
    let args = if args.is_null() { json!({}) } else { args };
    serde_json::from_value(args)
        .map_err(|e| anyhow!("Invalid arguments for {tool}: {e}. Arguments: {usage}"))
}

fn scope_of(ctx: &ToolContext) -> AgentRunScope {
    run_scope(ctx).cloned().unwrap_or_default()
}

/// `id` and/or `ids` merged into one list.
fn id_list(id: Option<String>, ids: Option<Vec<String>>) -> Vec<String> {
    let mut all: Vec<String> = ids.unwrap_or_default();
    if let Some(id) = id {
        if !all.contains(&id) {
            all.insert(0, id);
        }
    }
    all
}

fn with_note(mut result: Value, note: &crate::toolbox::WriteNote) -> Value {
    result["ok"] = json!(true);
    result["transaction"] = note.to_json();
    result
}

// ── Argument shapes ──────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct QueryArgs {
    ids: Option<Vec<String>>,
    name_contains: Option<String>,
    kind: Option<String>,
    has_component: Option<String>,
    parent_id: Option<String>,
    root_only: Option<bool>,
    descendants_of: Option<String>,
    visible: Option<bool>,
    locked: Option<bool>,
    limit: Option<usize>,
    offset: Option<usize>,
    /// Include each object's full component data (large).
    include_component_data: Option<bool>,
}

impl QueryArgs {
    fn filter(&self) -> QueryFilter {
        QueryFilter {
            ids: self.ids.clone(),
            name_contains: self.name_contains.clone(),
            kind: self.kind.clone(),
            has_component: self.has_component.clone(),
            parent_id: self.parent_id.clone(),
            root_only: self.root_only,
            descendants_of: self.descendants_of.clone(),
            visible: self.visible,
            locked: self.locked,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GetObjectArgs {
    id: String,
    include_component_data: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct ListClassesArgs {
    name_contains: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DescribeArgs {
    class_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnArgs {
    name: String,
    id: Option<String>,
    kind: Option<String>,
    parent_id: Option<String>,
    position: Option<Value>,
    rotation: Option<Value>,
    scale: Option<Value>,
    visible: Option<bool>,
    #[serde(default)]
    components: Vec<ComponentSpec>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetObjectArgs {
    id: String,
    name: Option<String>,
    visible: Option<bool>,
    locked: Option<bool>,
    /// Present with `null` or "" moves the object to the root.
    #[serde(default, deserialize_with = "double_option")]
    parent_id: Option<Option<String>>,
}

fn double_option<'de, D>(d: D) -> std::result::Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(Some(Option::<String>::deserialize(d)?))
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TransformArgs {
    id: Option<String>,
    ids: Option<Vec<String>>,
    position: Option<Value>,
    rotation: Option<Value>,
    scale: Option<Value>,
    translate: Option<Value>,
    rotate_by: Option<Value>,
    scale_by: Option<Value>,
    include_children: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct AddComponentArgs {
    id: String,
    class_name: String,
    properties: Option<Value>,
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SetComponentArgs {
    id: String,
    component_index: Option<usize>,
    class_name: Option<String>,
    properties: Option<Value>,
    enabled: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RemoveComponentArgs {
    id: String,
    component_index: Option<usize>,
    class_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteArgs {
    id: Option<String>,
    ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ApplySandboxArgs {
    id: String,
    force: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DiscardSandboxArgs {
    id: String,
}

fn opt_vec(value: &Option<Value>, name: &str) -> Result<Option<[f32; 3]>> {
    value.as_ref().map(|v| ops::parse_vec3(v, name)).transpose()
}

// ── Handlers ─────────────────────────────────────────────────────────────────

impl SceneToolbox {
    pub fn tool_query(&self, scope: &AgentRunScope, args: Value) -> Result<Value> {
        let args: QueryArgs = parse_args(
            "scene_query",
            args,
            &format!("{}, limit (default {DEFAULT_LIMIT}, max {MAX_LIMIT}), offset, include_component_data", ops::FILTER_DOC),
        )?;
        let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
        let offset = args.offset.unwrap_or(0);
        let include = args.include_component_data.unwrap_or(false);
        self.read(scope, |world| {
            let index = SceneIndex::build(world);
            let matches = args.filter().select(world, &index)?;
            let total = matches.len();
            let objects: Vec<Value> = matches
                .iter()
                .skip(offset)
                .take(limit)
                .map(|&e| {
                    if include {
                        ops::object_detail(world, &index, e, true)
                    } else {
                        ops::object_summary(world, &index, e)
                    }
                })
                .collect();
            let returned = objects.len();
            let mut out = json!({
                "total_matches": total,
                "scene_object_count": index.len(),
                "offset": offset,
                "returned": returned,
                "objects": objects,
            });
            if offset + returned < total {
                out["next_offset"] = json!(offset + returned);
                out["note"] = json!("More matches exist; pass next_offset as offset, or narrow the filter.");
            }
            Ok(out)
        })
    }

    pub fn tool_get_object(&self, scope: &AgentRunScope, args: Value) -> Result<Value> {
        let args: GetObjectArgs =
            parse_args("scene_get_object", args, "id (required), include_component_data (default true)")?;
        self.read(scope, |world| {
            let index = SceneIndex::build(world);
            let entity = ops::require_object(&index, &args.id)?;
            Ok(ops::object_detail(
                world,
                &index,
                entity,
                args.include_component_data.unwrap_or(true),
            ))
        })
    }

    pub fn tool_list_component_classes(&self, args: Value) -> Result<Value> {
        let args: ListClassesArgs =
            parse_args("scene_list_component_classes", args, "name_contains (optional)")?;
        let needle = args.name_contains.map(|n| n.to_lowercase());
        let mut classes: Vec<&str> = pulsar_world_registry::registered_world_component_classes()
            .filter(|name| *name != "ClassInstance")
            .filter(|name| needle.as_ref().is_none_or(|n| name.to_lowercase().contains(n.as_str())))
            .collect();
        classes.sort_unstable();
        classes.dedup();
        Ok(json!({ "count": classes.len(), "classes": classes }))
    }

    pub fn tool_describe_component(&self, args: Value) -> Result<Value> {
        let args: DescribeArgs = parse_args("scene_describe_component", args, "class_name (required)")?;
        ops::describe_class(&args.class_name)
    }

    pub fn tool_spawn(&self, scope: &AgentRunScope, args: Value) -> Result<Value> {
        let args: SpawnArgs = parse_args(
            "scene_write_spawn_object",
            args,
            "name (required), id, kind, parent_id, position [x,y,z], rotation [x,y,z] degrees, scale [x,y,z], visible, components [{class_name, properties, enabled}]",
        )?;
        let spec = SpawnSpec {
            position: opt_vec(&args.position, "position")?,
            rotation: opt_vec(&args.rotation, "rotation")?,
            scale: opt_vec(&args.scale, "scale")?,
            id: args.id,
            name: args.name,
            kind: args.kind,
            parent_id: args.parent_id,
            visible: args.visible,
            components: args.components,
        };
        let (id, note) = self.write(scope, "scene_write_spawn_object", |world, rec, rules| {
            let name = spec.name.clone();
            let id = ops::spawn(world, rec, rules, spec)?;
            let summary = format!("spawned '{id}' (\"{name}\")");
            Ok((id, summary))
        })?;
        Ok(with_note(json!({ "id": id }), &note))
    }

    pub fn tool_set_object(&self, scope: &AgentRunScope, args: Value) -> Result<Value> {
        let args: SetObjectArgs = parse_args(
            "scene_write_set_object",
            args,
            "id (required), name, visible, locked, parent_id (null or \"\" = move to the root)",
        )?;
        let id = args.id.clone();
        let edit = ObjectEdit {
            name: args.name,
            visible: args.visible,
            locked: args.locked,
            parent: args.parent_id.map(|p| p.filter(|s| !s.is_empty())),
        };
        let (_, note) = self.write(scope, "scene_write_set_object", |world, rec, _| {
            ops::edit_object(world, rec, &args.id, edit)?;
            Ok(((), format!("edited object '{}'", args.id)))
        })?;
        Ok(with_note(json!({ "id": id }), &note))
    }

    pub fn tool_transform(&self, scope: &AgentRunScope, args: Value) -> Result<Value> {
        let args: TransformArgs = parse_args(
            "scene_write_transform",
            args,
            "id or ids; absolute position/rotation(degrees)/scale, or relative translate/rotate_by/scale_by, all [x,y,z]; include_children (moves descendants by the same position change)",
        )?;
        let edit = TransformEdit {
            position: opt_vec(&args.position, "position")?,
            rotation: opt_vec(&args.rotation, "rotation")?,
            scale: opt_vec(&args.scale, "scale")?,
            translate: opt_vec(&args.translate, "translate")?,
            rotate_by: opt_vec(&args.rotate_by, "rotate_by")?,
            scale_by: opt_vec(&args.scale_by, "scale_by")?,
            include_children: args.include_children.unwrap_or(false),
        };
        let ids = id_list(args.id, args.ids);
        let (changed, note) = self.write(scope, "scene_write_transform", |world, rec, _| {
            let changed = ops::transform_objects(world, rec, &ids, &edit)?;
            let summary = format!("transformed {}", changed.join(", "));
            Ok((changed, summary))
        })?;
        Ok(with_note(json!({ "changed": changed }), &note))
    }

    pub fn tool_add_component(&self, scope: &AgentRunScope, args: Value) -> Result<Value> {
        let args: AddComponentArgs = parse_args(
            "scene_write_add_component",
            args,
            "id, class_name (both required), properties {field.path: value}, enabled",
        )?;
        let spec = ComponentSpec {
            class_name: args.class_name.clone(),
            properties: args.properties,
            enabled: args.enabled,
        };
        let (index, note) = self.write(scope, "scene_write_add_component", |world, rec, rules| {
            let index = ops::add_component(world, rec, rules, &args.id, &spec)?;
            Ok((index, format!("added {} to '{}'", spec.class_name, args.id)))
        })?;
        Ok(with_note(
            json!({ "id": args.id, "class_name": args.class_name, "component_index": index }),
            &note,
        ))
    }

    pub fn tool_set_component(&self, scope: &AgentRunScope, args: Value) -> Result<Value> {
        let args: SetComponentArgs = parse_args(
            "scene_write_set_component",
            args,
            "id (required), component_index or class_name (the first of that class), properties {field.path: value} (only named fields change), enabled",
        )?;
        let ((index, class), note) =
            self.write(scope, "scene_write_set_component", |world, rec, rules| {
                let (index, class) = ops::edit_component(
                    world,
                    rec,
                    rules,
                    &args.id,
                    ComponentEdit {
                        component_index: args.component_index,
                        class_name: args.class_name.as_deref(),
                        properties: args.properties.as_ref(),
                        enabled: args.enabled,
                    },
                )?;
                let summary = format!("edited {class} on '{}'", args.id);
                Ok(((index, class), summary))
            })?;
        Ok(with_note(
            json!({ "id": args.id, "class_name": class, "component_index": index }),
            &note,
        ))
    }

    pub fn tool_remove_component(&self, scope: &AgentRunScope, args: Value) -> Result<Value> {
        let args: RemoveComponentArgs = parse_args(
            "scene_write_remove_component",
            args,
            "id (required), component_index or class_name",
        )?;
        let (class, note) = self.write(scope, "scene_write_remove_component", |world, rec, _| {
            let class = ops::remove_component(
                world,
                rec,
                &args.id,
                args.component_index,
                args.class_name.as_deref(),
            )?;
            let summary = format!("removed {class} from '{}'", args.id);
            Ok((class, summary))
        })?;
        Ok(with_note(json!({ "id": args.id, "removed": class }), &note))
    }

    pub fn tool_delete(&self, scope: &AgentRunScope, args: Value) -> Result<Value> {
        let args: DeleteArgs = parse_args("scene_write_delete_object", args, "id or ids")?;
        let ids = id_list(args.id, args.ids);
        let (removed, note) = self.write(scope, "scene_write_delete_object", |world, rec, _| {
            let removed = ops::delete_objects(world, rec, &ids)?;
            Ok((removed, format!("deleted {} (with descendants: {removed} object(s))", ids.join(", "))))
        })?;
        Ok(with_note(
            json!({ "deleted": ids, "objects_removed": removed }),
            &note,
        ))
    }

    pub fn tool_apply_sandbox(&self, args: Value) -> Result<Value> {
        let args: ApplySandboxArgs =
            parse_args("scene_write_apply_sandbox", args, "id (the sandbox review id), force (default false)")?;
        match self.apply_sandbox(&args.id, args.force.unwrap_or(false)) {
            Ok(message) => Ok(json!({ "ok": true, "message": message })),
            Err(err) => bail!("{err}"),
        }
    }

    pub fn tool_discard_sandbox(&self, args: Value) -> Result<Value> {
        let args: DiscardSandboxArgs =
            parse_args("scene_write_discard_sandbox", args, "id (the sandbox review id)")?;
        match self.discard_sandbox(&args.id) {
            Ok(message) => Ok(json!({ "ok": true, "message": message })),
            Err(err) => bail!("{err}"),
        }
    }
}

// ── Registration ─────────────────────────────────────────────────────────────

const VEC3: &str = "Three numbers [x, y, z].";

fn schema_vec3(description: &str) -> Value {
    json!({
        "type": "array",
        "items": {"type": "number"},
        "minItems": 3,
        "maxItems": 3,
        "description": format!("{description} {VEC3}"),
    })
}

fn component_selector() -> Value {
    json!({
        "component_index": {"type": "integer", "description": "Index from scene_get_object."},
        "class_name": {"type": "string", "description": "Alternative to component_index: the first component of this class."},
    })
}

/// Register the scene tools on `registry`. Idempotent per name.
pub fn register_tools(registry: &mut ToolRegistry, toolbox: &Arc<SceneToolbox>) {
    macro_rules! tool {
        ($name:expr, $desc:expr, $schema:expr, |$tb:ident, $scope:ident, $args:ident| $body:expr) => {{
            let $tb = toolbox.clone();
            registry.register_fn($name, $desc, $schema, move |$args, ctx| {
                let $scope = scope_of(ctx);
                let _ = &$scope;
                $body
            });
        }};
    }

    tool!(
        "scene_query",
        "List scene objects, optionally filtered. Returns compact summaries (id, name, kind, parent, transform, component class names) in hierarchy order, paged. Call this before editing to find ids.",
        json!({
            "type": "object",
            "properties": {
                "ids": {"type": "array", "items": {"type": "string"}, "description": "Only these object ids."},
                "name_contains": {"type": "string", "description": "Case-insensitive name filter."},
                "kind": {"type": "string", "description": "Object kind, e.g. mesh_cube, light_point, camera, empty, folder."},
                "has_component": {"type": "string", "description": "Component class name, e.g. LightComponent."},
                "parent_id": {"type": "string", "description": "Direct children of this object."},
                "root_only": {"type": "boolean", "description": "Only top-level objects."},
                "descendants_of": {"type": "string", "description": "Whole subtree under this object."},
                "visible": {"type": "boolean"},
                "locked": {"type": "boolean"},
                "limit": {"type": "integer", "description": "Page size (default 50, max 200)."},
                "offset": {"type": "integer", "description": "Skip this many matches."},
                "include_component_data": {"type": "boolean", "description": "Include full component data (large)."}
            }
        }),
        |tb, scope, args| tb.tool_query(&scope, args)
    );

    tool!(
        "scene_get_object",
        "Get one object in full: transform, parent, children, props and every component with its index, class, enabled flag and current data.",
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Object id from scene_query."},
                "include_component_data": {"type": "boolean", "description": "Default true."}
            },
            "required": ["id"]
        }),
        |tb, scope, args| tb.tool_get_object(&scope, args)
    );

    tool!(
        "scene_list_component_classes",
        "List the component classes that can be attached to scene objects (rendering, lighting, physics, audio, scripting, terrain...).",
        json!({
            "type": "object",
            "properties": {"name_contains": {"type": "string", "description": "Case-insensitive filter on the class name."}}
        }),
        |tb, scope, args| tb.tool_list_component_classes(args)
    );

    tool!(
        "scene_describe_component",
        "List a component class's fields with their dotted paths, types and defaults. Call this before setting component properties: the paths are the keys of `properties` in the scene_write_* tools.",
        json!({
            "type": "object",
            "properties": {"class_name": {"type": "string", "description": "e.g. LightComponent"}},
            "required": ["class_name"]
        }),
        |tb, scope, args| tb.tool_describe_component(args)
    );

    tool!(
        "scene_write_spawn_object",
        "Create a scene object, optionally with components. One undoable transaction. A mesh or light needs components to render or light anything: attach e.g. StaticMeshComponent or LightComponent (see scene_list_component_classes and scene_describe_component).",
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "Display name."},
                "id": {"type": "string", "description": "Stable id; generated when omitted."},
                "kind": {"type": "string", "description": format!("One of: {}. Default empty.", ops::KINDS)},
                "parent_id": {"type": "string", "description": "Parent object id; omit for the root."},
                "position": schema_vec3("World position."),
                "rotation": schema_vec3("Euler rotation in degrees."),
                "scale": schema_vec3("Scale."),
                "visible": {"type": "boolean"},
                "components": {
                    "type": "array",
                    "description": "Components to attach.",
                    "items": {
                        "type": "object",
                        "properties": {
                            "class_name": {"type": "string"},
                            "properties": {"type": "object", "description": "Fields to set by dotted path; the rest keep defaults."},
                            "enabled": {"type": "boolean"}
                        },
                        "required": ["class_name"]
                    }
                }
            },
            "required": ["name"]
        }),
        |tb, scope, args| tb.tool_spawn(&scope, args)
    );

    tool!(
        "scene_write_set_object",
        "Rename an object, show or hide it, lock it, or move it to another parent. One undoable transaction.",
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "name": {"type": "string"},
                "visible": {"type": "boolean"},
                "locked": {"type": "boolean"},
                "parent_id": {"type": ["string", "null"], "description": "New parent id; null or empty string moves the object to the root."}
            },
            "required": ["id"]
        }),
        |tb, scope, args| tb.tool_set_object(&scope, args)
    );

    tool!(
        "scene_write_transform",
        "Move, rotate or scale one or more objects, absolutely (position, rotation, scale) or relatively (translate, rotate_by, scale_by). Rotation is Euler degrees. Transforms are flat world-space values: children do not follow their parent unless include_children is set (it applies position changes only). One undoable transaction for all ids.",
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "ids": {"type": "array", "items": {"type": "string"}},
                "position": schema_vec3("Set the world position."),
                "rotation": schema_vec3("Set the rotation in degrees."),
                "scale": schema_vec3("Set the scale."),
                "translate": schema_vec3("Add to the position."),
                "rotate_by": schema_vec3("Add to the rotation in degrees."),
                "scale_by": schema_vec3("Multiply the scale."),
                "include_children": {"type": "boolean", "description": "Also move all descendants by the same position change."}
            }
        }),
        |tb, scope, args| tb.tool_transform(&scope, args)
    );

    tool!(
        "scene_write_add_component",
        "Attach a component to an object. Fields you do not set keep the class defaults; unknown or ill-typed fields are rejected with the list of valid field paths. One undoable transaction.",
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "Object id."},
                "class_name": {"type": "string", "description": "Component class, e.g. LightComponent."},
                "properties": {"type": "object", "description": "Fields to set, keyed by dotted path from scene_describe_component."},
                "enabled": {"type": "boolean"}
            },
            "required": ["id", "class_name"]
        }),
        |tb, scope, args| tb.tool_add_component(&scope, args)
    );

    let mut set_props = json!({
        "id": {"type": "string", "description": "Object id."},
        "properties": {"type": "object", "description": "Fields to change, keyed by dotted path from scene_describe_component. Only named fields change."},
        "enabled": {"type": "boolean", "description": "Enable or disable the component."}
    });
    if let (Some(base), Some(extra)) = (set_props.as_object_mut(), component_selector().as_object()) {
        for (key, value) in extra {
            base.insert(key.clone(), value.clone());
        }
    }
    tool!(
        "scene_write_set_component",
        "Change fields of a component (and/or enable or disable it). The edit is validated against the class: an unknown field or a value of the wrong type is rejected, nothing is written, and the error lists the valid field paths. One undoable transaction.",
        json!({"type": "object", "properties": set_props, "required": ["id"]}),
        |tb, scope, args| tb.tool_set_component(&scope, args)
    );

    let mut remove_props = json!({"id": {"type": "string", "description": "Object id."}});
    if let (Some(base), Some(extra)) = (remove_props.as_object_mut(), component_selector().as_object()) {
        for (key, value) in extra {
            base.insert(key.clone(), value.clone());
        }
    }
    tool!(
        "scene_write_remove_component",
        "Remove a component from an object. One undoable transaction.",
        json!({"type": "object", "properties": remove_props, "required": ["id"]}),
        |tb, scope, args| tb.tool_remove_component(&scope, args)
    );

    tool!(
        "scene_write_delete_object",
        "Delete objects and everything below them. One undoable transaction.",
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string"},
                "ids": {"type": "array", "items": {"type": "string"}}
            }
        }),
        |tb, scope, args| tb.tool_delete(&scope, args)
    );

    tool!(
        "scene_write_apply_sandbox",
        "Apply a finished sandbox run's scene diff to the live scene as one undoable transaction. Refused when the live scene changed the same objects since the sandbox was created, unless force is true.",
        json!({
            "type": "object",
            "properties": {
                "id": {"type": "string", "description": "The sandbox review id from the sandbox run's report."},
                "force": {"type": "boolean", "description": "Overwrite conflicting live changes."}
            },
            "required": ["id"]
        }),
        |tb, scope, args| tb.tool_apply_sandbox(args)
    );

    tool!(
        "scene_write_discard_sandbox",
        "Discard a finished sandbox run's scene diff without applying it.",
        json!({
            "type": "object",
            "properties": {"id": {"type": "string", "description": "The sandbox review id."}},
            "required": ["id"]
        }),
        |tb, scope, args| tb.tool_discard_sandbox(args)
    );
}
