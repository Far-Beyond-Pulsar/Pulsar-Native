//! Placed-class tools: list the project's classes, place instances, and
//! edit an instance's variables and slot overrides.

use super::*;
use crate::scene_edit::Transform;
use tool_registry_macros::tool;

/// List the project's classes (Blueprint / script classes) that can be
/// placed in the level with level_editor_place_class.
#[tool(category = "level_editor")]
pub fn level_editor_list_classes() -> Result<Value> {
    let registry = scene_edit::classes::project_registry();
    let classes: Vec<Value> = registry
        .entries()
        .iter()
        .map(|entry| {
            json!({
                "name": entry.name,
                "id": entry.id.to_string(),
                "dir": entry.dir.display().to_string(),
            })
        })
        .collect();
    Ok(json!({ "count": classes.len(), "classes": classes }))
}

/// Place an instance of a class in the level (like dragging a class from
/// the file browser into the viewport). The instance gets the class's
/// components and generated child objects.
///
/// # Arguments
/// * `class` - Class name (from level_editor_list_classes) or class directory.
/// * `parent_id` - Parent object id; omit for a root object.
/// * `position` - `[x, y, z]` in metres. Default origin.
/// * `rotation` - Euler angles `[pitch, yaw, roll]` in degrees.
/// * `scale` - `[x, y, z]` scale factors. Default `[1, 1, 1]`.
#[tool(category = "level_editor")]
pub fn level_editor_place_class(
    ctx: &ToolContext,
    class: String,
    parent_id: Option<String>,
    position: Option<[f32; 3]>,
    rotation: Option<[f32; 3]>,
    scale: Option<[f32; 3]>,
) -> Result<Value> {
    let registry = scene_edit::classes::project_registry();
    let class_dir = match registry.by_name(&class) {
        Some(entry) => entry.dir.clone(),
        None if std::path::Path::new(&class).is_dir() => std::path::PathBuf::from(&class),
        None => bail!("Unknown class '{class}'. Use level_editor_list_classes."),
    };
    let state_arc = edit_scene(ctx)?;
    let mut state = state_arc.write();
    if let Some(parent) = &parent_id {
        require_object(&state, parent)?;
    }
    let defaults = Transform::default();
    let result = execute_command(
        &mut state,
        SceneCommand::InstantiateClass {
            class_dir,
            transform: Transform {
                position: position.unwrap_or(defaults.position),
                rotation: rotation.unwrap_or(defaults.rotation),
                scale: scale.unwrap_or(defaults.scale),
            },
            parent_id,
        },
    );
    if !result.changed {
        bail!("Class could not be placed: {}", result.no_op_reason);
    }
    Ok(json!({ "created_id": result.affected_ids.first(), "affected_ids": result.affected_ids }))
}

/// Show a placed class instance: its variables (default, current value,
/// whether overridden) and its component slots with their overrides.
///
/// # Arguments
/// * `id` - Id of the instance's root object.
#[tool(category = "level_editor")]
pub fn level_editor_get_class_instance(ctx: &ToolContext, id: String) -> Result<Value> {
    let state_arc = open_scene(ctx)?;
    let state = state_arc.read();
    require_object(&state, &id)?;
    let registry = scene_edit::classes::project_registry();
    let view = scene_edit::classes::class_instance_view(&state.scene.world(), &id, &registry)
        .ok_or_else(|| anyhow!("'{id}' is not a placed class instance"))?;
    Ok(json!({
        "class_name": view.class_name,
        "class_id": view.class_id,
        "resolved": view.resolved,
        "variables": view.variables.iter().map(|v| json!({
            "name": v.name,
            "kind": format!("{:?}", v.kind),
            "default": v.default,
            "value": v.value,
            "overridden": v.overridden,
        })).collect::<Vec<_>>(),
        "slots": view.slots.iter().map(|s| json!({
            "slot_id": s.slot_id,
            "class_name": s.class_name,
            "object_id": s.object_id,
            "removed": s.removed,
            "overridden": s.overridden.iter().map(|o| json!({
                "path": o.path, "default": o.default, "value": o.value,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
    }))
}

/// Set a placed class instance's variable, or revert it to the class
/// default.
///
/// # Arguments
/// * `id` - Id of the instance's root object.
/// * `name` - Variable name (from level_editor_get_class_instance).
/// * `value` - New value; `null` reverts to the class default.
#[tool(category = "level_editor")]
pub fn level_editor_set_class_variable(
    ctx: &ToolContext,
    id: String,
    name: String,
    value: Value,
) -> Result<Value> {
    let state_arc = edit_scene(ctx)?;
    let mut state = state_arc.write();
    require_object(&state, &id)?;
    let value = (!value.is_null()).then_some(value);
    let result = execute_command(
        &mut state,
        SceneCommand::SetClassVariable { id, name, value },
    );
    Ok(command_json(&result))
}

/// Revert a placed class instance's component slot to the class: a single
/// overridden property, or the whole slot (also restores a removed slot).
///
/// # Arguments
/// * `id` - Id of the instance's root object.
/// * `slot_id` - Slot id (from level_editor_get_class_instance).
/// * `path` - Dot path of the property to revert (e.g. `intensity.intensity`);
///   omit to revert the whole slot.
#[tool(category = "level_editor")]
pub fn level_editor_revert_class_slot(
    ctx: &ToolContext,
    id: String,
    slot_id: String,
    path: Option<String>,
) -> Result<Value> {
    let state_arc = edit_scene(ctx)?;
    let mut state = state_arc.write();
    require_object(&state, &id)?;
    let result = execute_command(
        &mut state,
        SceneCommand::RevertClassSlot { id, slot_id, path },
    );
    Ok(command_json(&result))
}
