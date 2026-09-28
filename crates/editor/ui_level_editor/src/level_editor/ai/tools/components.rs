//! Component tools: discover classes, attach, edit, enable, reorder, remove.
//!
//! Component data is the class's whole-instance JSON -- the shape
//! `EngineClass::to_json` produces, `#[sub_props]` groups included as nested
//! objects. Edits are JSON merge patches over that shape, validated against
//! the class before they are written.

use super::*;
use pulsar_reflection::{EngineClass, REGISTRY, RUNTIME_TYPE_REGISTRY};
use tool_registry_macros::tool;

// ── Class catalogue ──────────────────────────────────────────────────────────

/// Every component class the Add Component menu offers: reflected engine
/// classes plus plugin-provided ones. `ClassInstance` is excluded -- it only
/// comes from placing a class.
fn component_classes() -> Vec<(String, Option<String>, Option<String>)> {
    let mut classes: Vec<(String, Option<String>, Option<String>)> = Vec::new();
    for category in REGISTRY.get_categories() {
        for name in REGISTRY.get_class_names_by_category(category) {
            classes.push((name.to_string(), Some(category.to_string()), None));
        }
    }
    for name in REGISTRY.get_class_names() {
        if !classes.iter().any(|(n, ..)| n == name) {
            classes.push((name.to_string(), None, None));
        }
    }
    if let Some(pm) = plugin_manager::global() {
        for def in pm.read().get_all_component_definitions() {
            if !classes.iter().any(|(n, ..)| *n == def.id) {
                classes.push((def.id, Some(def.category), Some(def.description)));
            }
        }
    }
    classes.retain(|(name, ..)| name != pulsar_class::CLASS_INSTANCE);
    classes.sort_by(|a, b| a.0.cmp(&b.0));
    classes
}

fn create_instance(class_name: &str) -> Option<Box<dyn EngineClass>> {
    REGISTRY.create_instance(class_name).or_else(|| {
        engine_backend::EngineBackend::global()
            .and_then(|b| b.read().plugin_components().create_instance(class_name))
    })
}

fn unknown_class(class_name: &str) -> anyhow::Error {
    let needle = class_name.to_lowercase();
    let similar: Vec<String> = component_classes()
        .into_iter()
        .map(|(name, ..)| name)
        .filter(|name| {
            let lower = name.to_lowercase();
            lower.contains(&needle) || needle.contains(lower.trim_end_matches("component"))
        })
        .take(8)
        .collect();
    anyhow!(
        "Unknown component class '{class_name}'. Similar: {similar:?}. \
         Use level_editor_list_component_classes for the full list."
    )
}

/// The class's default data, in the shape edits patch.
fn default_data(instance: &dyn EngineClass) -> Value {
    instance.to_json().unwrap_or_else(|_| {
        // Classes without `serialize`: the flat property map the properties
        // panel builds for them.
        Value::Object(
            instance
                .get_properties()
                .iter()
                .map(|prop| {
                    let value = RUNTIME_TYPE_REGISTRY
                        .serialize_json_for_any((prop.getter)(instance).as_ref())
                        .unwrap_or(Value::Null);
                    (prop.name.to_string(), value)
                })
                .collect(),
        )
    })
}

/// Reject data the class can't deserialize, so a bad patch comes back to the
/// model as an error instead of being dropped on hydration.
fn validate(class_name: &str, data: &Value) -> Result<()> {
    match REGISTRY.create_instance_from_json(class_name, data) {
        Some(Err(error)) => bail!(
            "Data does not fit {class_name}: {error}. \
             See level_editor_describe_component_class for its shape."
        ),
        _ => Ok(()),
    }
}

/// Default data for `class_name` with `properties` merged over it.
pub(super) fn build_component_data(class_name: &str, properties: Option<&Value>) -> Result<Value> {
    let instance = create_instance(class_name).ok_or_else(|| unknown_class(class_name))?;
    let mut data = default_data(instance.as_ref());
    if let Some(patch) = properties {
        if !patch.is_object() {
            bail!("`properties` must be a JSON object (a patch over the class's default data)");
        }
        merge_json(&mut data, patch);
    }
    validate(class_name, &data)?;
    Ok(data)
}

/// Index of the addressed component: `component_index` wins, otherwise the
/// first instance of `class_name`.
fn resolve_index(
    world: &World,
    id: &str,
    component_index: Option<usize>,
    class_name: Option<&str>,
) -> Result<usize> {
    let components = scene_edit::components::get_components_metadata(world, id);
    match (component_index, class_name) {
        (Some(index), _) if index < components.len() => Ok(index),
        (Some(index), _) => bail!(
            "Object '{id}' has {} components; index {index} is out of range",
            components.len()
        ),
        (None, Some(class)) => components
            .iter()
            .position(|c| c.class_name == class)
            .ok_or_else(|| anyhow!("Object '{id}' has no {class} component")),
        (None, None) => bail!("Pass `component_index` or `class_name` to choose the component"),
    }
}

fn run(ctx: &ToolContext, id: &str, cmd: impl FnOnce(&World) -> Result<SceneCommand>) -> Result<Value> {
    let state_arc = open_scene(ctx)?;
    let mut state = state_arc.write();
    require_object(&state, id)?;
    let cmd = cmd(&state.scene.world())?;
    let result = execute_command(&mut state, cmd);
    let components = components_json(&state.scene.world(), id, false);
    let mut out = command_json(&result);
    out["components"] = components;
    Ok(out)
}

fn components_json(world: &World, id: &str, include_data: bool) -> Value {
    let components = if include_data {
        scene_edit::components::get_components(world, id)
    } else {
        scene_edit::components::get_components_metadata(world, id)
    };
    Value::Array(
        components
            .into_iter()
            .enumerate()
            .map(|(index, c)| {
                let mut entry = json!({
                    "index": index,
                    "class_name": c.class_name,
                    "enabled": c.enabled,
                });
                if include_data {
                    entry["data"] = c.data;
                }
                entry
            })
            .collect(),
    )
}

// ── Tools ────────────────────────────────────────────────────────────────────

/// List the component classes that can be attached to objects.
///
/// Rendering, lighting, physics, audio, scripting and terrain are all
/// components (e.g. StaticMeshComponent, LightComponent, RigidbodyComponent,
/// PlanetTerrainComponent).
///
/// # Arguments
/// * `name_contains` - Case-insensitive filter on the class name.
#[tool(category = "level_editor")]
pub fn level_editor_list_component_classes(name_contains: Option<String>) -> Result<Value> {
    let needle = name_contains.map(|n| n.to_lowercase());
    let classes: Vec<Value> = component_classes()
        .into_iter()
        .filter(|(name, ..)| needle.as_ref().is_none_or(|n| name.to_lowercase().contains(n)))
        .map(|(name, category, description)| {
            json!({ "class_name": name, "category": category, "description": description })
        })
        .collect();
    Ok(json!({ "count": classes.len(), "classes": classes }))
}

/// Show a component class's fields and default data.
///
/// `default_data` is the exact JSON shape used by `properties` in
/// level_editor_add_component / level_editor_spawn_object and by
/// level_editor_set_component_properties; nested objects are property groups.
///
/// # Arguments
/// * `class_name` - Component class, e.g. `LightComponent`.
#[tool(category = "level_editor")]
pub fn level_editor_describe_component_class(class_name: String) -> Result<Value> {
    let instance = create_instance(&class_name).ok_or_else(|| unknown_class(&class_name))?;
    let properties: Vec<Value> = instance
        .get_properties()
        .iter()
        .map(|prop| {
            json!({
                "name": prop.name,
                "display_name": prop.display_name,
                "category": prop.category,
                "type": prop.type_info.type_name,
            })
        })
        .collect();
    Ok(json!({
        "class_name": class_name,
        "default_data": default_data(instance.as_ref()),
        "properties": properties,
    }))
}

/// List an object's components with their index, enabled state and current
/// data. Indices address components in the other component tools.
///
/// # Arguments
/// * `id` - Object id.
#[tool(category = "level_editor")]
pub fn level_editor_get_components(ctx: &ToolContext, id: String) -> Result<Value> {
    let state_arc = open_scene(ctx)?;
    let state = state_arc.read();
    require_object(&state, &id)?;
    Ok(json!({ "id": id, "components": components_json(&state.scene.world(), &id, true) }))
}

/// Attach a new component to an object (Add Component in the details panel).
///
/// # Arguments
/// * `id` - Object id.
/// * `class_name` - Component class, e.g. `StaticMeshComponent`.
/// * `properties` - Optional patch over the class's default data (see
///   level_editor_describe_component_class), e.g.
///   `{"intensity": {"intensity": 5000}}`.
#[tool(category = "level_editor")]
pub fn level_editor_add_component(
    ctx: &ToolContext,
    id: String,
    class_name: String,
    properties: Option<Value>,
) -> Result<Value> {
    let data = build_component_data(&class_name, properties.as_ref())?;
    run(ctx, &id.clone(), |_| {
        Ok(SceneCommand::AddComponent { id, class_name, data })
    })
}

/// Change fields of a component with a JSON merge patch.
///
/// Objects in the patch merge into the current data and any other value
/// replaces the field. Don't send `null` to reset a field -- use
/// level_editor_revert_component_property. Invalid data is rejected with the
/// reason, and nothing is written.
///
/// # Arguments
/// * `id` - Object id.
/// * `properties` - Patch over the component's current data, e.g.
///   `{"color": {"color": [1.0, 0.5, 0.2]}}`.
/// * `component_index` - Which component (from level_editor_get_components).
/// * `class_name` - Alternative to `component_index`: the first component of this class.
#[tool(category = "level_editor")]
pub fn level_editor_set_component_properties(
    ctx: &ToolContext,
    id: String,
    properties: Value,
    component_index: Option<usize>,
    class_name: Option<String>,
) -> Result<Value> {
    if !properties.is_object() {
        bail!("`properties` must be a JSON object");
    }
    run(ctx, &id.clone(), |world| {
        let index = resolve_index(world, &id, component_index, class_name.as_deref())?;
        let component = scene_edit::components::get_components(world, &id)
            .into_iter()
            .nth(index)
            .ok_or_else(|| anyhow!("Component {index} vanished"))?;
        let mut data = component.data;
        merge_json(&mut data, &properties);
        validate(&component.class_name, &data)?;
        Ok(SceneCommand::SetComponentData {
            id,
            component_index: index,
            data,
        })
    })
}

/// Reset one component field to its default (the class default for
/// components that come from a placed class).
///
/// # Arguments
/// * `id` - Object id.
/// * `property` - Property name as listed by level_editor_describe_component_class.
/// * `component_index` - Which component.
/// * `class_name` - Alternative to `component_index`: the first component of this class.
#[tool(category = "level_editor")]
pub fn level_editor_revert_component_property(
    ctx: &ToolContext,
    id: String,
    property: String,
    component_index: Option<usize>,
    class_name: Option<String>,
) -> Result<Value> {
    run(ctx, &id.clone(), |world| {
        let index = resolve_index(world, &id, component_index, class_name.as_deref())?;
        let class_name = scene_edit::components::get_component_class_names(world, &id)
            .swap_remove(index);
        let slot_default = scene_edit::classes::slot_defaults(
            world,
            &id,
            &scene_edit::classes::project_registry(),
        )
        .remove(&index)
        .is_some();
        if slot_default {
            return Ok(SceneCommand::RevertComponentProperty {
                id,
                class_name,
                component_index: index,
                prop_name: property,
            });
        }
        // Not from a class: the class's own default value.
        let instance = create_instance(&class_name).ok_or_else(|| unknown_class(&class_name))?;
        let default = default_data(instance.as_ref());
        let value = default
            .get(&property)
            .cloned()
            .ok_or_else(|| anyhow!("{class_name} has no top-level property '{property}'"))?;
        let mut data = scene_edit::components::get_components(world, &id)
            .swap_remove(index)
            .data;
        data[property.as_str()] = value;
        Ok(SceneCommand::SetComponentData {
            id,
            component_index: index,
            data,
        })
    })
}

/// Remove a component from an object.
///
/// # Arguments
/// * `id` - Object id.
/// * `component_index` - Which component.
/// * `class_name` - Alternative to `component_index`: the first component of this class.
#[tool(category = "level_editor")]
pub fn level_editor_remove_component(
    ctx: &ToolContext,
    id: String,
    component_index: Option<usize>,
    class_name: Option<String>,
) -> Result<Value> {
    run(ctx, &id.clone(), |world| {
        let component_index = resolve_index(world, &id, component_index, class_name.as_deref())?;
        Ok(SceneCommand::RemoveComponent { id, component_index })
    })
}

/// Enable or disable a component. Disabled components keep their data but
/// have no effect.
///
/// # Arguments
/// * `id` - Object id.
/// * `enabled` - New state.
/// * `component_index` - Which component.
/// * `class_name` - Alternative to `component_index`: the first component of this class.
#[tool(category = "level_editor")]
pub fn level_editor_set_component_enabled(
    ctx: &ToolContext,
    id: String,
    enabled: bool,
    component_index: Option<usize>,
    class_name: Option<String>,
) -> Result<Value> {
    run(ctx, &id.clone(), |world| {
        let component_index = resolve_index(world, &id, component_index, class_name.as_deref())?;
        Ok(SceneCommand::SetComponentEnabled {
            id,
            component_index,
            enabled,
        })
    })
}

/// Duplicate a component on the same object; the copy is inserted right
/// after the original.
///
/// # Arguments
/// * `id` - Object id.
/// * `component_index` - Component to copy.
#[tool(category = "level_editor")]
pub fn level_editor_duplicate_component(
    ctx: &ToolContext,
    id: String,
    component_index: usize,
) -> Result<Value> {
    run(ctx, &id.clone(), |world| {
        resolve_index(world, &id, Some(component_index), None)?;
        Ok(SceneCommand::DuplicateComponent { id, component_index })
    })
}

/// Move a component to another position in the object's component list, or
/// nest it under another component (component hierarchy).
///
/// # Arguments
/// * `id` - Object id.
/// * `component_index` - Component to move.
/// * `to_index` - New position in the list.
/// * `parent_index` - Nest under this component instead. Use -1 to un-nest
///   to the top level.
#[tool(category = "level_editor")]
pub fn level_editor_move_component(
    ctx: &ToolContext,
    id: String,
    component_index: usize,
    to_index: Option<usize>,
    parent_index: Option<i64>,
) -> Result<Value> {
    run(ctx, &id.clone(), |world| {
        resolve_index(world, &id, Some(component_index), None)?;
        match (to_index, parent_index) {
            (Some(to_index), None) => {
                resolve_index(world, &id, Some(to_index), None)?;
                Ok(SceneCommand::ReorderComponent {
                    id,
                    from_index: component_index,
                    to_index,
                })
            }
            (None, Some(parent)) => {
                let parent_index = usize::try_from(parent).ok();
                if let Some(parent) = parent_index {
                    resolve_index(world, &id, Some(parent), None)?;
                }
                Ok(SceneCommand::SetComponentParent {
                    id,
                    component_index,
                    parent_index,
                })
            }
            _ => bail!("Pass exactly one of `to_index` or `parent_index`"),
        }
    })
}
