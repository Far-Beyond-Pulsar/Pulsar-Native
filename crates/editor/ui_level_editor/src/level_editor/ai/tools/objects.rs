//! Object tools: find, inspect, spawn, duplicate, delete, transform, rename,
//! show/hide/lock, select and rearrange the hierarchy.

use super::*;
use crate::level_editor::scene_edit::Transform;
use tool_registry_macros::tool;

// ── Spawning ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ComponentSpec {
    class_name: String,
    #[serde(default)]
    properties: Option<Value>,
    #[serde(default = "enabled_default")]
    enabled: bool,
}

fn enabled_default() -> bool {
    true
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnSpec {
    name: String,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    position: Option<[f32; 3]>,
    #[serde(default)]
    rotation: Option<[f32; 3]>,
    #[serde(default)]
    scale: Option<[f32; 3]>,
    #[serde(default)]
    visible: Option<bool>,
    #[serde(default)]
    locked: Option<bool>,
    #[serde(default)]
    components: Vec<ComponentSpec>,
}

/// The object an `AddObject` for `spec` needs. Components ride along inline,
/// so the object and its components are one undo step.
fn spawn_data(spec: SpawnSpec) -> Result<(SceneObjectData, Option<String>)> {
    let object_type = object_type_from_kind(spec.kind.as_deref().unwrap_or("empty"))?;
    let components = spec
        .components
        .into_iter()
        .map(|c| {
            let data = components::build_component_data(&c.class_name, c.properties.as_ref())?;
            Ok(json!({ "class_name": c.class_name, "enabled": c.enabled, "data": data }))
        })
        .collect::<Result<Vec<_>>>()?;
    let defaults = Transform::default();
    let data = SceneObjectData {
        id: String::new(),
        name: spec.name,
        object_type,
        transform: Transform {
            position: spec.position.unwrap_or(defaults.position),
            rotation: spec.rotation.unwrap_or(defaults.rotation),
            scale: spec.scale.unwrap_or(defaults.scale),
        },
        visible: spec.visible.unwrap_or(true),
        locked: spec.locked.unwrap_or(false),
        parent: spec.parent_id.clone(),
        children: vec![],
        scene_path: String::new(),
        props: Default::default(),
        component_instances: (!components.is_empty()).then(|| Value::Array(components)),
    };
    Ok((data, spec.parent_id))
}

fn spawn(state: &mut LevelEditorState, spec: SpawnSpec) -> Result<String> {
    if let Some(parent) = &spec.parent_id {
        require_object(state, parent)?;
    }
    let (data, parent_id) = spawn_data(spec)?;
    let result = execute_command(state, SceneCommand::AddObject { data, parent_id });
    result
        .affected_ids
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("Object could not be added: {}", result.no_op_reason))
}

/// Create a new object, optionally with components, in one undoable step.
///
/// An object's appearance and behaviour come from its components. For
/// example a visible cube is an object with a `StaticMeshComponent`; a lamp
/// is an object with a `LightComponent`. Look up classes with
/// level_editor_list_component_classes and their data with
/// level_editor_describe_component_class.
///
/// # Arguments
/// * `name` - Display name.
/// * `kind` - Editor tag for icon and picking: empty (default), folder,
///   camera, light_directional, light_point, light_spot, light_area,
///   mesh_cube, mesh_sphere, mesh_cylinder, mesh_plane, mesh_custom,
///   particle_system, audio_source. Use `folder` to group objects.
/// * `parent_id` - Parent object id; omit for a root object.
/// * `position` - `[x, y, z]` in metres. Default origin.
/// * `rotation` - Euler angles `[pitch, yaw, roll]` in degrees. Default zero.
/// * `scale` - `[x, y, z]` scale factors. Default `[1, 1, 1]`.
/// * `visible` - Default true.
/// * `locked` - Locked objects can't be picked in the viewport. Default false.
/// * `components` - Components to attach, each
///   `{"class_name": "...", "properties": {field path: value}}`. Field paths
///   come from level_editor_describe_component_class (call it first). A red
///   point light: `[{"class_name": "LightComponent", "properties":
///   {"color.color": [1.0, 0.1, 0.1, 1.0], "intensity.intensity": 2000.0}}]`.
#[tool(category = "level_editor")]
pub fn level_editor_spawn_object(
    ctx: &ToolContext,
    name: String,
    kind: Option<String>,
    parent_id: Option<String>,
    position: Option<[f32; 3]>,
    rotation: Option<[f32; 3]>,
    scale: Option<[f32; 3]>,
    visible: Option<bool>,
    locked: Option<bool>,
    components: Option<Vec<Value>>,
) -> Result<Value> {
    let components = components
        .unwrap_or_default()
        .into_iter()
        .map(serde_json::from_value)
        .collect::<Result<Vec<ComponentSpec>, _>>()
        .map_err(|e| anyhow!("Invalid component entry: {e}"))?;
    let spec = SpawnSpec {
        name,
        kind,
        parent_id,
        position,
        rotation,
        scale,
        visible,
        locked,
        components,
    };
    let state_arc = edit_scene(ctx)?;
    let mut state = state_arc.write();
    let id = spawn(&mut state, spec)?;
    let world = state.scene.world();
    let object = scene_edit::objects::get_object(&world, &id)
        .map(|o| object_summary(&world, &o));
    Ok(json!({ "created_id": id, "object": object }))
}

/// Create many objects at once (e.g. a row of props or a whole layout).
///
/// Each entry takes the same fields as level_editor_spawn_object. An entry
/// may name a parent created earlier in the same call by referring to it as
/// `"$<index>"` (e.g. `"parent_id": "$0"`). Failed entries are reported and
/// the rest still get created.
///
/// # Arguments
/// * `objects` - List of `{name, kind?, parent_id?, position?, rotation?,
///   scale?, visible?, locked?, components?}`.
#[tool(category = "level_editor")]
pub fn level_editor_spawn_objects(ctx: &ToolContext, objects: Vec<Value>) -> Result<Value> {
    let state_arc = edit_scene(ctx)?;
    let mut state = state_arc.write();
    let mut created: Vec<Option<String>> = Vec::with_capacity(objects.len());
    let mut errors = Vec::new();
    for (index, entry) in objects.into_iter().enumerate() {
        let outcome = serde_json::from_value::<SpawnSpec>(entry)
            .map_err(|e| anyhow!("{e}"))
            .and_then(|mut spec| {
                if let Some(reference) = spec.parent_id.as_deref().and_then(|p| p.strip_prefix('$')) {
                    let parent = reference
                        .parse::<usize>()
                        .ok()
                        .and_then(|i| created.get(i).cloned().flatten())
                        .ok_or_else(|| anyhow!("parent ${reference} was not created"))?;
                    spec.parent_id = Some(parent);
                }
                spawn(&mut state, spec)
            });
        match outcome {
            Ok(id) => created.push(Some(id)),
            Err(error) => {
                errors.push(json!({ "index": index, "error": error.to_string() }));
                created.push(None);
            }
        }
    }
    Ok(json!({
        "created_ids": created,
        "created_count": created.iter().flatten().count(),
        "errors": errors,
    }))
}

// ── Finding ──────────────────────────────────────────────────────────────────

/// List objects, optionally filtered, in hierarchy order.
///
/// # Arguments
/// * `filter` - Object filter; keys (all optional, AND-combined): ids
///   [string], name_contains (case-insensitive), kind, has_component (class
///   name), parent_id (direct children of), root_only (bool), descendants_of
///   (id; whole subtree), visible, locked.
/// * `offset` - Skip this many matches. Default 0.
/// * `limit` - Maximum results. Default 100, max 1000.
#[tool(category = "level_editor")]
pub fn level_editor_list_objects(
    ctx: &ToolContext,
    filter: Option<Value>,
    offset: Option<usize>,
    limit: Option<usize>,
) -> Result<Value> {
    let filter = ObjectFilter::parse(filter)?;
    let state_arc = open_scene(ctx)?;
    let state = state_arc.read();
    let world = state.scene.world();
    let matched = filter.select(&world);
    let offset = offset.unwrap_or(0);
    let limit = limit.unwrap_or(100).clamp(1, 1000);
    let items: Vec<Value> = matched
        .iter()
        .skip(offset)
        .take(limit)
        .map(|o| object_summary(&world, o))
        .collect();
    Ok(json!({ "total_matches": matched.len(), "offset": offset, "items": items }))
}

/// Everything about one object: transform, flags, parent/children, extra
/// properties, full component data and placed-class info.
///
/// # Arguments
/// * `id` - Object id.
#[tool(category = "level_editor")]
pub fn level_editor_get_object(ctx: &ToolContext, id: String) -> Result<Value> {
    let state_arc = open_scene(ctx)?;
    let state = state_arc.read();
    let object = require_object(&state, &id)?;
    let world = state.scene.world();
    let mut out = object_summary(&world, &object);
    out["children"] = json!(object.children);
    out["props"] = json!(object.props);
    out["components"] = Value::Array(
        scene_edit::components::get_components(&world, &id)
            .into_iter()
            .enumerate()
            .map(|(index, c)| {
                json!({ "index": index, "class_name": c.class_name, "enabled": c.enabled, "data": c.data })
            })
            .collect(),
    );
    out["is_class_instance"] = json!(scene_edit::classes::is_class_root(&world, &id));
    out["is_generated_by_class"] = json!(scene_edit::classes::is_generated_child(&world, &id));
    out["selected"] =
        json!(scene_edit::objects::get_selected_object_id(&world).as_deref() == Some(id.as_str()));
    Ok(out)
}

fn hierarchy_node(world: &World, id: &str, depth: usize, max_depth: usize) -> Value {
    let Some(object) = scene_edit::objects::get_object(world, id) else {
        return Value::Null;
    };
    let mut node = json!({
        "id": object.id,
        "name": object.name,
        "kind": object_kind(&object.object_type),
        "components": scene_edit::components::get_component_class_names(world, id),
    });
    if depth < max_depth {
        node["children"] = Value::Array(
            object
                .children
                .iter()
                .map(|child| hierarchy_node(world, child, depth + 1, max_depth))
                .collect(),
        );
    } else if !object.children.is_empty() {
        node["child_count"] = json!(object.children.len());
    }
    node
}

/// The object hierarchy as a tree (the Hierarchy panel).
///
/// # Arguments
/// * `root_id` - Start from this object; omit for the whole level.
/// * `max_depth` - Levels to expand. Default 4.
#[tool(category = "level_editor")]
pub fn level_editor_get_hierarchy(
    ctx: &ToolContext,
    root_id: Option<String>,
    max_depth: Option<usize>,
) -> Result<Value> {
    let state_arc = open_scene(ctx)?;
    let state = state_arc.read();
    let max_depth = max_depth.unwrap_or(4);
    let world = state.scene.world();
    let tree = match root_id {
        Some(id) => {
            require_object(&state, &id)?;
            vec![hierarchy_node(&world, &id, 0, max_depth)]
        }
        None => scene_edit::objects::get_root_objects(&world)
            .iter()
            .map(|o| hierarchy_node(&world, &o.id, 0, max_depth))
            .collect(),
    };
    Ok(json!({ "tree": tree }))
}

// ── Editing ──────────────────────────────────────────────────────────────────

/// Set an object's position, rotation and/or scale. Omitted fields are
/// unchanged.
///
/// # Arguments
/// * `id` - Object id.
/// * `position` - `[x, y, z]` in metres.
/// * `rotation` - Euler angles `[pitch, yaw, roll]` in degrees.
/// * `scale` - `[x, y, z]` scale factors.
#[tool(category = "level_editor")]
pub fn level_editor_set_transform(
    ctx: &ToolContext,
    id: String,
    position: Option<[f32; 3]>,
    rotation: Option<[f32; 3]>,
    scale: Option<[f32; 3]>,
) -> Result<Value> {
    let state_arc = edit_scene(ctx)?;
    let mut state = state_arc.write();
    require_object(&state, &id)?;
    let result = execute_command(
        &mut state,
        SceneCommand::SetTransform {
            id,
            position,
            rotation,
            scale,
        },
    );
    Ok(command_json(&result))
}

/// Move, rotate and/or scale several objects relative to where they are
/// now (e.g. "raise all the lamps by 2 m").
///
/// # Arguments
/// * `ids` - Objects to change. Give this or `filter`.
/// * `filter` - Select objects instead (same keys as level_editor_list_objects).
/// * `translate` - `[x, y, z]` metres added to the position.
/// * `rotate` - `[pitch, yaw, roll]` degrees added to the rotation.
/// * `scale_by` - `[x, y, z]` factors multiplied into the scale.
#[tool(category = "level_editor")]
pub fn level_editor_move_objects(
    ctx: &ToolContext,
    ids: Option<Vec<String>>,
    filter: Option<Value>,
    translate: Option<[f32; 3]>,
    rotate: Option<[f32; 3]>,
    scale_by: Option<[f32; 3]>,
) -> Result<Value> {
    let state_arc = edit_scene(ctx)?;
    let mut state = state_arc.write();
    let ids = target_ids(&state.scene.world(), ids, filter)?;
    let add = |a: [f32; 3], b: [f32; 3]| [a[0] + b[0], a[1] + b[1], a[2] + b[2]];
    let mut changed = Vec::new();
    let mut errors = Vec::new();
    for id in ids {
        let Some(object) = scene_edit::objects::get_object(&state.scene.world(), &id) else {
            errors.push(json!({ "id": id, "error": "not found" }));
            continue;
        };
        let t = object.transform;
        let result = execute_command(
            &mut state,
            SceneCommand::SetTransform {
                id: id.clone(),
                position: translate.map(|d| add(t.position, d)),
                rotation: rotate.map(|d| add(t.rotation, d)),
                scale: scale_by.map(|f| [t.scale[0] * f[0], t.scale[1] * f[1], t.scale[2] * f[2]]),
            },
        );
        if result.changed {
            changed.push(id);
        }
    }
    Ok(json!({ "changed_ids": changed, "errors": errors }))
}

/// Rename an object.
///
/// # Arguments
/// * `id` - Object id.
/// * `name` - New display name.
#[tool(category = "level_editor")]
pub fn level_editor_rename_object(ctx: &ToolContext, id: String, name: String) -> Result<Value> {
    let state_arc = edit_scene(ctx)?;
    let mut state = state_arc.write();
    require_object(&state, &id)?;
    Ok(command_json(&execute_command(&mut state, SceneCommand::SetName { id, name })))
}

/// Show/hide and lock/unlock objects.
///
/// # Arguments
/// * `ids` - Objects to change. Give this or `filter`.
/// * `filter` - Select objects instead (same keys as level_editor_list_objects).
/// * `visible` - Show (true) or hide (false).
/// * `locked` - Lock (true) prevents picking in the viewport.
#[tool(category = "level_editor")]
pub fn level_editor_set_object_flags(
    ctx: &ToolContext,
    ids: Option<Vec<String>>,
    filter: Option<Value>,
    visible: Option<bool>,
    locked: Option<bool>,
) -> Result<Value> {
    if visible.is_none() && locked.is_none() {
        bail!("Pass `visible` and/or `locked`");
    }
    let state_arc = edit_scene(ctx)?;
    let mut state = state_arc.write();
    let ids = target_ids(&state.scene.world(), ids, filter)?;
    let changed: Vec<String> = ids
        .into_iter()
        .filter(|id| {
            execute_command(
                &mut state,
                SceneCommand::SetVisibility {
                    id: id.clone(),
                    visible,
                    locked,
                },
            )
            .changed
        })
        .collect();
    Ok(json!({ "changed_ids": changed }))
}

/// Duplicate an object (with its components; a placed class duplicates as a
/// new instance of the class).
///
/// # Arguments
/// * `id` - Object to copy.
/// * `count` - Number of copies. Default 1, max 500.
/// * `offset` - `[x, y, z]` metres between successive copies: copy n is
///   placed at the original position + n × offset. Great for rows and stacks.
#[tool(category = "level_editor")]
pub fn level_editor_duplicate_object(
    ctx: &ToolContext,
    id: String,
    count: Option<usize>,
    offset: Option<[f32; 3]>,
) -> Result<Value> {
    let state_arc = edit_scene(ctx)?;
    let mut state = state_arc.write();
    require_object(&state, &id)?;
    let result = execute_command(
        &mut state,
        SceneCommand::DuplicateObject {
            source_id: id,
            count: count.unwrap_or(1).clamp(1, 500),
            position_offset: offset,
        },
    );
    Ok(command_json(&result))
}

/// Delete objects. Deleting an object also deletes everything under it.
///
/// # Arguments
/// * `ids` - Objects to delete. Give this or `filter`.
/// * `filter` - Select objects instead (same keys as level_editor_list_objects).
#[tool(category = "level_editor")]
pub fn level_editor_delete_objects(
    ctx: &ToolContext,
    ids: Option<Vec<String>>,
    filter: Option<Value>,
) -> Result<Value> {
    let state_arc = edit_scene(ctx)?;
    let mut state = state_arc.write();
    let ids = target_ids(&state.scene.world(), ids, filter)?;
    let mut deleted = Vec::new();
    let mut not_found = Vec::new();
    for id in ids {
        if scene_edit::objects::get_object(&state.scene.world(), &id).is_none() {
            // Either unknown, or already gone with a deleted ancestor.
            not_found.push(id);
            continue;
        }
        if execute_command(&mut state, SceneCommand::RemoveObject { id: id.clone() }).changed {
            deleted.push(id);
        }
    }
    Ok(json!({ "deleted_ids": deleted, "not_found_or_already_deleted": not_found }))
}

/// Select an object in the editor (or clear the selection), so the user
/// sees it in the details panel and viewport gizmo.
///
/// # Arguments
/// * `id` - Object to select; omit or null to clear the selection.
#[tool(category = "level_editor")]
pub fn level_editor_select_object(ctx: &ToolContext, id: Option<String>) -> Result<Value> {
    let state_arc = open_scene(ctx)?;
    let mut state = state_arc.write();
    if let Some(id) = &id {
        require_object(&state, id)?;
    }
    execute_command(&mut state, SceneCommand::SelectObject { id: id.clone() });
    crate::level_editor::core::splines::sync_selection(&mut state);
    Ok(json!({ "selected_id": id }))
}

// ── Hierarchy ────────────────────────────────────────────────────────────────

/// Move an object under a new parent, or to the root.
///
/// # Arguments
/// * `id` - Object to move.
/// * `new_parent_id` - New parent's id; omit or null for the root.
#[tool(category = "level_editor")]
pub fn level_editor_reparent_object(
    ctx: &ToolContext,
    id: String,
    new_parent_id: Option<String>,
) -> Result<Value> {
    let state_arc = edit_scene(ctx)?;
    let mut state = state_arc.write();
    require_object(&state, &id)?;
    if let Some(parent) = &new_parent_id {
        require_object(&state, parent)?;
        if !scene_edit::objects::can_reparent(&state.scene.world(), &id, Some(parent)) {
            bail!("Can't move '{id}' under '{parent}': that would create a cycle");
        }
    }
    let result = execute_command(&mut state, SceneCommand::ReparentObject { id, new_parent_id });
    Ok(command_json(&result))
}

/// Change an object's order among its siblings in the hierarchy.
///
/// Sibling order is not recorded in undo history, like the Hierarchy panel's
/// own reordering.
///
/// # Arguments
/// * `id` - Object to move.
/// * `direction` - `up` or `down` one step, or `swap` with `target_id`.
/// * `target_id` - Sibling to swap places with (for `swap`).
#[tool(category = "level_editor")]
pub fn level_editor_reorder_object(
    ctx: &ToolContext,
    id: String,
    direction: String,
    target_id: Option<String>,
) -> Result<Value> {
    let state_arc = edit_scene(ctx)?;
    let mut state = state_arc.write();
    require_object(&state, &id)?;
    let changed = {
        let mut world = state.scene.world_mut();
        match direction.as_str() {
            "up" => {
                scene_edit::objects::move_object_up(&mut world, &id);
                true
            }
            "down" => {
                scene_edit::objects::move_object_down(&mut world, &id);
                true
            }
            "swap" => {
                let target = target_id.ok_or_else(|| anyhow!("`swap` needs `target_id`"))?;
                scene_edit::objects::reorder_object_siblings(&mut world, &id, &target)
            }
            other => bail!("Unknown direction '{other}'. Use up, down or swap."),
        }
    };
    if changed {
        state.scene.bump_revision(true);
    }
    let world = state.scene.world();
    let siblings: Vec<String> = match scene_edit::objects::get_object(&world, &id).and_then(|o| o.parent) {
        Some(parent) => scene_edit::objects::get_children(&world, &parent),
        None => scene_edit::objects::get_root_objects(&world).into_iter().map(|o| o.id).collect(),
    };
    Ok(json!({ "changed": changed, "sibling_order": siblings }))
}
