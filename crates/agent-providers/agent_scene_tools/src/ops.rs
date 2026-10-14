//! Scene reads and edits on a `World`, with model-readable errors.
//!
//! Every function here works on a world it is handed, so the same code runs
//! on the live scene and on a sandbox fork. Edits validate everything before
//! the first write and record what they replace in the [`Recorder`]; a call
//! that fails after writing is rolled back by its caller.

use std::any::Any;
use std::collections::HashSet;

use anyhow::{anyhow, bail, Result};
use pulsar_scene_model::attachments::{self, NewInstance};
use pulsar_scene_model::{
    LightType, MeshType, Name, ObjectType, SceneWorldExt, SpawnObject, Transform, Visibility,
};
use pulsar_scenedb::{Entity, World};
use pulsar_world_registry::{ComponentPayload, InstanceValue};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::journal::Recorder;
use crate::snapshot::{class_json, SceneIndex};
use crate::source::SceneSource;

/// The class the level editor creates by placing a class; not attachable by
/// hand.
const CLASS_INSTANCE: &str = "ClassInstance";

// ── Kinds ────────────────────────────────────────────────────────────────────

pub const KINDS: &str = "empty, folder, camera, light_directional, light_point, light_spot, light_area, mesh_cube, mesh_sphere, mesh_cylinder, mesh_plane, mesh_custom, particle_system, audio_source";

pub fn parse_kind(kind: &str) -> Result<ObjectType> {
    Ok(match kind {
        "empty" => ObjectType::Empty,
        "folder" => ObjectType::Folder,
        "camera" => ObjectType::Camera,
        "light_directional" => ObjectType::Light(LightType::Directional),
        "light_point" => ObjectType::Light(LightType::Point),
        "light_spot" => ObjectType::Light(LightType::Spot),
        "light_area" => ObjectType::Light(LightType::Area),
        "mesh_cube" => ObjectType::Mesh(MeshType::Cube),
        "mesh_sphere" => ObjectType::Mesh(MeshType::Sphere),
        "mesh_cylinder" => ObjectType::Mesh(MeshType::Cylinder),
        "mesh_plane" => ObjectType::Mesh(MeshType::Plane),
        "mesh_custom" => ObjectType::Mesh(MeshType::Custom),
        "particle_system" => ObjectType::ParticleSystem,
        "audio_source" => ObjectType::AudioSource,
        other => bail!("Unknown kind '{other}'. Valid kinds: {KINDS}"),
    })
}

pub fn kind_of(object_type: &ObjectType) -> &'static str {
    match object_type {
        ObjectType::Empty => "empty",
        ObjectType::Folder => "folder",
        ObjectType::Camera => "camera",
        ObjectType::Light(LightType::Directional) => "light_directional",
        ObjectType::Light(LightType::Point) => "light_point",
        ObjectType::Light(LightType::Spot) => "light_spot",
        ObjectType::Light(LightType::Area) => "light_area",
        ObjectType::Mesh(MeshType::Cube) => "mesh_cube",
        ObjectType::Mesh(MeshType::Sphere) => "mesh_sphere",
        ObjectType::Mesh(MeshType::Cylinder) => "mesh_cylinder",
        ObjectType::Mesh(MeshType::Plane) => "mesh_plane",
        ObjectType::Mesh(MeshType::Custom) => "mesh_custom",
        ObjectType::ParticleSystem => "particle_system",
        ObjectType::AudioSource => "audio_source",
        ObjectType::Blueprint => "blueprint",
    }
}

// ── Lookups ──────────────────────────────────────────────────────────────────

pub fn require_object(index: &SceneIndex, id: &str) -> Result<Entity> {
    index.entity(id).ok_or_else(|| {
        let similar = index.similar_ids(id, 5);
        if similar.is_empty() {
            anyhow!("No object with id '{id}'. Use scene_query to find ids.")
        } else {
            anyhow!(
                "No object with id '{id}'. Similar ids: {}. Use scene_query to list objects.",
                similar.join(", ")
            )
        }
    })
}

/// One attached component instance of an object.
pub struct ComponentRef {
    pub index: usize,
    pub instance: Entity,
    pub class_name: String,
    pub id: String,
    pub enabled: bool,
    pub unresolved: bool,
}

pub fn list_components(world: &World, owner: Entity) -> Vec<ComponentRef> {
    attachments::instances(world, owner)
        .into_iter()
        .enumerate()
        .filter_map(|(index, instance)| {
            let meta = attachments::meta(world, instance)?;
            Some(ComponentRef {
                index,
                instance,
                class_name: meta.class_name.clone(),
                id: meta.id.to_string(),
                enabled: attachments::is_enabled(world, instance),
                unresolved: world
                    .get::<attachments::UnresolvedComponent>(instance)
                    .is_some(),
            })
        })
        .collect()
}

/// Pick a component by `component_index` or by `class_name` (the first of
/// that class).
pub fn resolve_component(
    world: &World,
    object_id: &str,
    owner: Entity,
    component_index: Option<usize>,
    class_name: Option<&str>,
) -> Result<ComponentRef> {
    let mut components = list_components(world, owner);
    match (component_index, class_name) {
        (Some(index), class) => {
            if index >= components.len() {
                bail!(
                    "Object '{object_id}' has {} component(s); index {index} is out of range. \
                     Use scene_get_object to see them.",
                    components.len()
                );
            }
            let found = components.swap_remove(index);
            // swap_remove moved the last element; the index is still the
            // position in the original list.
            if let Some(class) = class {
                if found.class_name != class {
                    bail!(
                        "Component {index} of '{object_id}' is {}, not {class}.",
                        found.class_name
                    );
                }
            }
            Ok(found)
        }
        (None, Some(class)) => {
            let position = components.iter().position(|c| c.class_name == class);
            match position {
                Some(position) => Ok(components.swap_remove(position)),
                None => {
                    let have: Vec<&str> = components.iter().map(|c| c.class_name.as_str()).collect();
                    bail!(
                        "Object '{object_id}' has no {class} component. It has: {}.",
                        if have.is_empty() { "none".to_string() } else { have.join(", ") }
                    )
                }
            }
        }
        (None, None) => bail!("Pass `component_index` or `class_name` to choose the component."),
    }
}

// ── Views ────────────────────────────────────────────────────────────────────

pub fn object_summary(world: &World, index: &SceneIndex, entity: Entity) -> Value {
    let transform = world.get::<Transform>(entity).copied().unwrap_or_default();
    let visibility = world.get::<Visibility>(entity).copied().unwrap_or_default();
    let object_type = world.get::<ObjectType>(entity).copied().unwrap_or(ObjectType::Empty);
    json!({
        "id": index.id_of(entity),
        "name": world.get::<Name>(entity).map(|n| n.0.as_str()).unwrap_or(""),
        "kind": kind_of(&object_type),
        "parent_id": index.parent_id(entity),
        "child_count": index.children(Some(entity)).len(),
        "position": transform.position,
        "rotation": transform.rotation,
        "scale": transform.scale,
        "visible": visibility.visible,
        "locked": visibility.locked,
        "components": list_components(world, entity).into_iter().map(|c| c.class_name).collect::<Vec<_>>(),
    })
}

pub fn component_json(world: &World, component: &ComponentRef) -> Value {
    if component.unresolved {
        return world
            .get::<attachments::UnresolvedComponent>(component.instance)
            .map(|u| u.data.clone())
            .unwrap_or(Value::Null);
    }
    pulsar_world_registry::instance_engine_class(world, component.instance)
        .map(class_json)
        .unwrap_or(Value::Null)
}

pub fn object_detail(world: &World, index: &SceneIndex, entity: Entity, include_data: bool) -> Value {
    let mut out = object_summary(world, index, entity);
    out["children"] = json!(index
        .children(Some(entity))
        .iter()
        .filter_map(|c| index.id_of(*c))
        .collect::<Vec<_>>());
    out["components"] = Value::Array(
        list_components(world, entity)
            .into_iter()
            .map(|c| {
                let mut entry = json!({
                    "index": c.index,
                    "class_name": c.class_name,
                    "instance_id": c.id,
                    "enabled": c.enabled,
                });
                if c.unresolved {
                    entry["unresolved"] = json!(true);
                }
                if include_data {
                    entry["data"] = component_json(world, &c);
                }
                entry
            })
            .collect(),
    );
    if let Some(props) = world.get::<pulsar_scene_model::RenderProps>(entity) {
        if !props.props.is_empty() {
            out["props"] = json!(props.props);
        }
    }
    out
}

// ── Queries ──────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
pub struct QueryFilter {
    pub ids: Option<Vec<String>>,
    pub name_contains: Option<String>,
    pub kind: Option<String>,
    pub has_component: Option<String>,
    pub parent_id: Option<String>,
    pub root_only: Option<bool>,
    pub descendants_of: Option<String>,
    pub visible: Option<bool>,
    pub locked: Option<bool>,
}

pub const FILTER_DOC: &str = "ids [string], name_contains (case-insensitive), kind, has_component (class name), parent_id (direct children of), root_only (bool), descendants_of (id; whole subtree), visible (bool), locked (bool)";

impl QueryFilter {
    /// Matching objects in hierarchy order.
    pub fn select(&self, world: &World, index: &SceneIndex) -> Result<Vec<Entity>> {
        let ancestor = match &self.descendants_of {
            Some(id) => Some(require_object(index, id)?),
            None => None,
        };
        let parent = match &self.parent_id {
            Some(id) => Some(require_object(index, id)?),
            None => None,
        };
        if let Some(kind) = &self.kind {
            parse_kind(kind).map(|_| ()).or_else(|err| {
                if kind == "blueprint" {
                    Ok(())
                } else {
                    Err(err)
                }
            })?;
        }
        let needle = self.name_contains.as_ref().map(|n| n.to_lowercase());
        let wanted_ids: Option<HashSet<&str>> = self
            .ids
            .as_ref()
            .map(|ids| ids.iter().map(String::as_str).collect());
        Ok(index
            .all_ordered()
            .into_iter()
            .filter(|&entity| {
                if let Some(ids) = &wanted_ids {
                    if !index.id_of(entity).is_some_and(|id| ids.contains(id)) {
                        return false;
                    }
                }
                if let Some(needle) = &needle {
                    let name = world.get::<Name>(entity).map(|n| n.0.to_lowercase());
                    if !name.is_some_and(|n| n.contains(needle.as_str())) {
                        return false;
                    }
                }
                if let Some(kind) = &self.kind {
                    let object_type =
                        world.get::<ObjectType>(entity).copied().unwrap_or(ObjectType::Empty);
                    if kind_of(&object_type) != kind {
                        return false;
                    }
                }
                if let Some(class) = &self.has_component {
                    if !list_components(world, entity).iter().any(|c| &c.class_name == class) {
                        return false;
                    }
                }
                if let Some(parent) = parent {
                    if index.parent_of(entity) != Some(parent) {
                        return false;
                    }
                }
                if self.root_only == Some(true) && index.parent_of(entity).is_some() {
                    return false;
                }
                if let Some(ancestor) = ancestor {
                    if !index.is_descendant(entity, ancestor) {
                        return false;
                    }
                }
                let visibility = world.get::<Visibility>(entity).copied().unwrap_or_default();
                if self.visible.is_some_and(|v| v != visibility.visible) {
                    return false;
                }
                if self.locked.is_some_and(|l| l != visibility.locked) {
                    return false;
                }
                true
            })
            .collect())
    }
}

// ── Component data ───────────────────────────────────────────────────────────

fn similar_classes(class_name: &str) -> Vec<String> {
    let needle = class_name.to_lowercase();
    let mut similar: Vec<String> = pulsar_world_registry::registered_world_component_classes()
        .filter(|name| {
            let lower = name.to_lowercase();
            lower.contains(&needle) || needle.contains(lower.trim_end_matches("component"))
        })
        .map(str::to_string)
        .collect();
    similar.sort();
    similar.truncate(8);
    similar
}

pub fn unknown_class(class_name: &str) -> anyhow::Error {
    anyhow!(
        "Unknown component class '{class_name}'. Similar: {:?}. \
         Use scene_list_component_classes for the full list.",
        similar_classes(class_name)
    )
}

/// `path (type)` for every field, for error messages and `describe`.
pub fn field_paths(data: &Value) -> Vec<(String, String)> {
    fn walk(value: &Value, prefix: &str, depth: usize, out: &mut Vec<(String, String)>) {
        if out.len() >= 200 {
            return;
        }
        match value {
            Value::Object(map) if depth < 6 && !map.is_empty() => {
                for (key, child) in map {
                    let path = if prefix.is_empty() {
                        key.clone()
                    } else {
                        format!("{prefix}.{key}")
                    };
                    walk(child, &path, depth + 1, out);
                }
            }
            other => out.push((prefix.to_string(), type_name(other))),
        }
    }
    let mut out = Vec::new();
    walk(data, "", 0, &mut out);
    out
}

fn type_name(value: &Value) -> String {
    match value {
        Value::Null => "null (optional)".into(),
        Value::Bool(_) => "boolean".into(),
        Value::Number(_) => "number".into(),
        Value::String(_) => "string".into(),
        Value::Array(items) => {
            if items.iter().all(Value::is_number) {
                format!("array of {} numbers", items.len())
            } else {
                format!("array of {}", items.len())
            }
        }
        Value::Object(_) => "object".into(),
    }
}

fn field_list(data: &Value) -> String {
    let fields = field_paths(data);
    let shown: Vec<String> = fields
        .iter()
        .take(60)
        .map(|(path, ty)| format!("{path} ({ty})"))
        .collect();
    let mut text = shown.join(", ");
    if fields.len() > 60 {
        text.push_str(&format!(", ... {} more", fields.len() - 60));
    }
    text
}

fn set_path(root: &mut Value, path: &str, value: &Value) -> std::result::Result<(), String> {
    let segments: Vec<&str> = path.split('.').collect();
    let mut current = root;
    for (position, segment) in segments.iter().enumerate() {
        let walked = segments[..position].join(".");
        let Some(map) = current.as_object_mut() else {
            return Err(format!(
                "`{walked}` is a value, not a group, so it has no field `{segment}`"
            ));
        };
        let Some(slot) = map.get_mut(*segment) else {
            return Err(format!("there is no field `{path}`"));
        };
        if position + 1 == segments.len() {
            if value.is_object() && slot.is_object() {
                for (key, child) in value.as_object().into_iter().flatten() {
                    set_path(slot, key, child).map_err(|e| format!("{path}.{key}: {e}"))?;
                }
            } else {
                *slot = value.clone();
            }
            return Ok(());
        }
        current = slot;
    }
    Ok(())
}

/// Apply `patch` (keyed by dotted field paths; nested objects merge) to
/// `data`. A key that names no field is an error that lists the real ones.
pub fn apply_patch(class_name: &str, data: &mut Value, patch: &Value) -> Result<()> {
    let Some(patch) = patch.as_object() else {
        bail!("`properties` must be a JSON object keyed by field path");
    };
    for (key, value) in patch {
        if let Err(reason) = set_path(data, key, value) {
            bail!(
                "{class_name}: {reason}. Fields (path, type): {}",
                field_list(data)
            );
        }
    }
    Ok(())
}

/// Decode `data` as a `class_name` value, reporting the failing path.
pub fn decode_component(class_name: &str, data: &Value) -> Result<Box<dyn Any + Send + Sync>> {
    match pulsar_world_registry::decode_world_component_value(class_name, data) {
        Some(Ok(value)) => Ok(value),
        Some(Err(error)) => bail!(
            "Data does not fit {class_name}: {error}. Fields (path, type): {}",
            field_list(data)
        ),
        None => Err(unknown_class(class_name)),
    }
}

/// A new value of `class_name`: the class default with `properties` patched
/// over it, validated by decoding.
pub fn build_component_value(
    class_name: &str,
    properties: Option<&Value>,
) -> Result<Box<dyn Any + Send + Sync>> {
    if class_name == CLASS_INSTANCE {
        bail!("ClassInstance comes from placing a class and cannot be attached by hand.");
    }
    let default = pulsar_world_registry::new_world_component_value(class_name)
        .ok_or_else(|| unknown_class(class_name))?;
    let Some(patch) = properties else {
        return Ok(default);
    };
    let Some(class) = pulsar_world_registry::value_engine_class(class_name, default.as_ref())
    else {
        return Ok(default);
    };
    let mut data = class_json(class);
    apply_patch(class_name, &mut data, patch)?;
    decode_component(class_name, &data)
}

/// Default data and field list of a class, for `scene_describe_component`.
pub fn describe_class(class_name: &str) -> Result<Value> {
    let default = pulsar_world_registry::new_world_component_value(class_name)
        .ok_or_else(|| unknown_class(class_name))?;
    let class = pulsar_world_registry::value_engine_class(class_name, default.as_ref())
        .ok_or_else(|| anyhow!("{class_name} cannot be described in this build"))?;
    let data = class_json(class);
    let fields = field_paths(&data);
    let mut example = Map::new();
    for wanted in ["array of", "number", "boolean", "string"] {
        for (path, ty) in &fields {
            if example.len() >= 3 {
                break;
            }
            if ty.starts_with(wanted) && !example.contains_key(path) {
                if let Some(value) = pointer(&data, path) {
                    example.insert(path.clone(), value.clone());
                }
            }
        }
    }
    Ok(json!({
        "class_name": class_name,
        "fields": fields.iter().map(|(path, ty)| json!({
            "path": path,
            "type": ty,
            "default": pointer(&data, path).cloned().unwrap_or(Value::Null),
        })).collect::<Vec<_>>(),
        "example_properties": example,
    }))
}

fn pointer<'a>(data: &'a Value, dotted: &str) -> Option<&'a Value> {
    let mut current = data;
    for segment in dotted.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

// ── Edits ────────────────────────────────────────────────────────────────────

pub fn parse_vec3(value: &Value, name: &str) -> Result<[f32; 3]> {
    let numbers: Vec<f64> = match value {
        Value::Array(items) if items.len() == 3 => items
            .iter()
            .map(|v| v.as_f64())
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| anyhow!("`{name}` must hold three numbers, got {value}"))?,
        Value::Object(map) => ["x", "y", "z"]
            .iter()
            .map(|k| map.get(*k).and_then(Value::as_f64))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| anyhow!("`{name}` object needs numeric x, y and z, got {value}"))?,
        Value::Number(n) if name.contains("scale") => {
            let v = n.as_f64().unwrap_or(1.0);
            vec![v, v, v]
        }
        _ => bail!("`{name}` must be an array of three numbers [x, y, z], got {value}"),
    };
    let out = [numbers[0] as f32, numbers[1] as f32, numbers[2] as f32];
    if out.iter().any(|v| !v.is_finite()) {
        bail!("`{name}` must be finite numbers, got {value}");
    }
    Ok(out)
}

#[derive(Debug, Default, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ComponentSpec {
    pub class_name: String,
    pub properties: Option<Value>,
    pub enabled: Option<bool>,
}

pub struct SpawnSpec {
    pub id: Option<String>,
    pub name: String,
    pub kind: Option<String>,
    pub parent_id: Option<String>,
    pub position: Option<[f32; 3]>,
    pub rotation: Option<[f32; 3]>,
    pub scale: Option<[f32; 3]>,
    pub visible: Option<bool>,
    pub components: Vec<ComponentSpec>,
}

fn attach_checked(
    world: &mut World,
    rules: &dyn SceneSource,
    owner: Entity,
    object_id: &str,
    class_name: &str,
    value: Box<dyn Any + Send + Sync>,
    enabled: bool,
) -> Result<Entity> {
    let mut spec = NewInstance::new(class_name);
    spec.enabled = enabled;
    let instance =
        pulsar_world_registry::attach_component(world, owner, spec, ComponentPayload::Value(value))
            .map_err(|e| anyhow!("Could not attach {class_name} to '{object_id}': {e}"))?;
    rules
        .check_instance(world, instance)
        .map_err(|reason| anyhow!("{class_name} was refused on '{object_id}': {reason}"))?;
    Ok(instance)
}

/// Create an object. Returns its stable id.
pub fn spawn(
    world: &mut World,
    rec: &mut Recorder,
    rules: &dyn SceneSource,
    spec: SpawnSpec,
) -> Result<String> {
    let index = SceneIndex::build(world);
    let name = spec.name.trim().to_string();
    if name.is_empty() {
        bail!("`name` must not be empty.");
    }
    if let Some(id) = &spec.id {
        if id.trim().is_empty() {
            bail!("`id` must not be empty when given; leave it out to have one generated.");
        }
        if index.entity(id).is_some() {
            bail!("An object with id '{id}' already exists. Pick another id or omit it.");
        }
    }
    let object_type = match &spec.kind {
        Some(kind) => parse_kind(kind)?,
        None => ObjectType::Empty,
    };
    let parent = match &spec.parent_id {
        Some(parent) if !parent.is_empty() => Some(require_object(&index, parent)?),
        _ => None,
    };
    // Decode every component before creating anything.
    let mut components = Vec::new();
    for component in &spec.components {
        if component.class_name.trim().is_empty() {
            bail!("Every entry of `components` needs a `class_name`.");
        }
        let value = build_component_value(&component.class_name, component.properties.as_ref())?;
        components.push((component.class_name.clone(), value, component.enabled.unwrap_or(true)));
    }
    let mut transform = Transform::default();
    if let Some(p) = spec.position {
        transform.position = p;
    }
    if let Some(r) = spec.rotation {
        transform.rotation = r;
    }
    if let Some(s) = spec.scale {
        transform.scale = s;
    }
    let mut visibility = Visibility::default();
    if let Some(v) = spec.visible {
        visibility.visible = v;
    }
    let entity = world
        .spawn_object(SpawnObject {
            stable_id: spec.id.clone(),
            name,
            parent,
            transform,
            visibility,
            object_type,
        })
        .map_err(|e| anyhow!("Could not create the object: {e}"))?;
    let id = world
        .stable_id_of(entity)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("The new object has no id"))?;
    rec.note_new(&id);
    for (class, value, enabled) in components {
        attach_checked(world, rules, entity, &id, &class, value, enabled)?;
    }
    Ok(id)
}

pub struct ObjectEdit {
    pub name: Option<String>,
    pub visible: Option<bool>,
    pub locked: Option<bool>,
    /// `Some(None)` moves the object to the root.
    pub parent: Option<Option<String>>,
}

pub fn edit_object(world: &mut World, rec: &mut Recorder, id: &str, edit: ObjectEdit) -> Result<()> {
    let index = SceneIndex::build(world);
    let entity = require_object(&index, id)?;
    if edit.name.is_none() && edit.visible.is_none() && edit.locked.is_none() && edit.parent.is_none() {
        bail!("Nothing to change: pass at least one of name, visible, locked, parent_id.");
    }
    if let Some(name) = &edit.name {
        if name.trim().is_empty() {
            bail!("`name` must not be empty.");
        }
    }
    let new_parent = match &edit.parent {
        Some(Some(parent_id)) if !parent_id.is_empty() => {
            let parent = require_object(&index, parent_id)?;
            if parent == entity || index.is_descendant(parent, entity) {
                bail!("Cannot move '{id}' under '{parent_id}': that would make it its own ancestor.");
            }
            Some(Some(parent))
        }
        Some(_) => Some(None),
        None => None,
    };
    rec.note_existing(world, &index, id);
    if let Some(name) = edit.name {
        world.insert(entity, Name(name.trim().to_string()));
    }
    if edit.visible.is_some() || edit.locked.is_some() {
        let mut visibility = world.get::<Visibility>(entity).copied().unwrap_or_default();
        if let Some(v) = edit.visible {
            visibility.visible = v;
        }
        if let Some(l) = edit.locked {
            visibility.locked = l;
        }
        world.insert(entity, visibility);
    }
    if let Some(parent) = new_parent {
        if world.parent_of(entity) != parent {
            world
                .reparent(entity, parent)
                .map_err(|e| anyhow!("Could not reparent '{id}': {e}"))?;
        }
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct TransformEdit {
    pub position: Option<[f32; 3]>,
    pub rotation: Option<[f32; 3]>,
    pub scale: Option<[f32; 3]>,
    pub translate: Option<[f32; 3]>,
    pub rotate_by: Option<[f32; 3]>,
    pub scale_by: Option<[f32; 3]>,
    /// Apply the position change to every descendant too (transforms are
    /// flat world-space values, so children do not follow on their own).
    pub include_children: bool,
}

/// Move, rotate or scale objects. Returns the ids changed.
pub fn transform_objects(
    world: &mut World,
    rec: &mut Recorder,
    ids: &[String],
    edit: &TransformEdit,
) -> Result<Vec<String>> {
    if ids.is_empty() {
        bail!("Pass `id` or `ids`.");
    }
    for (absolute, relative, name) in [
        (edit.position.is_some(), edit.translate.is_some(), "position/translate"),
        (edit.rotation.is_some(), edit.rotate_by.is_some(), "rotation/rotate_by"),
        (edit.scale.is_some(), edit.scale_by.is_some(), "scale/scale_by"),
    ] {
        if absolute && relative {
            bail!("Give either the absolute or the relative form of {name}, not both.");
        }
    }
    if edit.position.is_none()
        && edit.rotation.is_none()
        && edit.scale.is_none()
        && edit.translate.is_none()
        && edit.rotate_by.is_none()
        && edit.scale_by.is_none()
    {
        bail!("Nothing to change: pass position, rotation, scale, translate, rotate_by or scale_by.");
    }
    let index = SceneIndex::build(world);
    let mut targets: Vec<Entity> = Vec::new();
    for id in ids {
        let entity = require_object(&index, id)?;
        if !targets.contains(&entity) {
            targets.push(entity);
        }
    }
    let mut changed = Vec::new();
    let mut writes: Vec<(Entity, Transform)> = Vec::new();
    for &entity in &targets {
        let old = world.get::<Transform>(entity).copied().unwrap_or_default();
        let mut next = old;
        if let Some(p) = edit.position {
            next.position = p;
        }
        if let Some(d) = edit.translate {
            for axis in 0..3 {
                next.position[axis] += d[axis];
            }
        }
        if let Some(r) = edit.rotation {
            next.rotation = r;
        }
        if let Some(d) = edit.rotate_by {
            for axis in 0..3 {
                next.rotation[axis] += d[axis];
            }
        }
        if let Some(s) = edit.scale {
            next.scale = s;
        }
        if let Some(f) = edit.scale_by {
            for axis in 0..3 {
                next.scale[axis] *= f[axis];
            }
        }
        writes.push((entity, next));
        if edit.include_children && (edit.position.is_some() || edit.translate.is_some()) {
            let delta = [
                next.position[0] - old.position[0],
                next.position[1] - old.position[1],
                next.position[2] - old.position[2],
            ];
            for descendant in index.subtree(entity).into_iter().skip(1) {
                if targets.contains(&descendant) {
                    continue;
                }
                let mut child = world.get::<Transform>(descendant).copied().unwrap_or_default();
                for axis in 0..3 {
                    child.position[axis] += delta[axis];
                }
                writes.push((descendant, child));
            }
        }
    }
    for (entity, next) in writes {
        let Some(id) = index.id_of(entity) else {
            continue;
        };
        rec.note_existing(world, &index, id);
        world.insert(entity, next);
        changed.push(id.to_string());
    }
    Ok(changed)
}

pub fn add_component(
    world: &mut World,
    rec: &mut Recorder,
    rules: &dyn SceneSource,
    id: &str,
    spec: &ComponentSpec,
) -> Result<usize> {
    let index = SceneIndex::build(world);
    let owner = require_object(&index, id)?;
    if spec.class_name.trim().is_empty() {
        bail!("`class_name` is required.");
    }
    let value = build_component_value(&spec.class_name, spec.properties.as_ref())?;
    rec.note_existing(world, &index, id);
    let instance = attach_checked(
        world,
        rules,
        owner,
        id,
        &spec.class_name,
        value,
        spec.enabled.unwrap_or(true),
    )?;
    Ok(attachments::instances(world, owner)
        .iter()
        .position(|e| *e == instance)
        .unwrap_or(0))
}

pub struct ComponentEdit<'a> {
    pub component_index: Option<usize>,
    pub class_name: Option<&'a str>,
    pub properties: Option<&'a Value>,
    pub enabled: Option<bool>,
}

/// Edit fields and/or the enabled flag of one component. Returns the
/// component's index and its class.
pub fn edit_component(
    world: &mut World,
    rec: &mut Recorder,
    rules: &dyn SceneSource,
    id: &str,
    edit: ComponentEdit<'_>,
) -> Result<(usize, String)> {
    let index = SceneIndex::build(world);
    let owner = require_object(&index, id)?;
    if edit.properties.is_none() && edit.enabled.is_none() {
        bail!("Nothing to change: pass `properties` and/or `enabled`.");
    }
    let component = resolve_component(world, id, owner, edit.component_index, edit.class_name)?;
    let mut decoded = None;
    if let Some(patch) = edit.properties {
        if component.unresolved {
            bail!(
                "{} on '{id}' is an unresolved component (this build cannot decode it), so its fields cannot be edited.",
                component.class_name
            );
        }
        let class = pulsar_world_registry::instance_engine_class(world, component.instance)
            .ok_or_else(|| anyhow!("{} on '{id}' holds no value", component.class_name))?;
        let mut data = class_json(class);
        apply_patch(&component.class_name, &mut data, patch)?;
        decoded = Some(decode_component(&component.class_name, &data)?);
    }
    rec.note_existing(world, &index, id);
    if let Some(value) = decoded {
        pulsar_world_registry::set_instance_value(world, component.instance, InstanceValue::Value(value))
            .map_err(|e| anyhow!("Could not write {} on '{id}': {e}", component.class_name))?;
    }
    if let Some(enabled) = edit.enabled {
        attachments::set_enabled(world, component.instance, enabled);
    }
    rules
        .check_instance(world, component.instance)
        .map_err(|reason| anyhow!("{} edit refused on '{id}': {reason}", component.class_name))?;
    Ok((component.index, component.class_name))
}

pub fn remove_component(
    world: &mut World,
    rec: &mut Recorder,
    id: &str,
    component_index: Option<usize>,
    class_name: Option<&str>,
) -> Result<String> {
    let index = SceneIndex::build(world);
    let owner = require_object(&index, id)?;
    let component = resolve_component(world, id, owner, component_index, class_name)?;
    rec.note_existing(world, &index, id);
    if !attachments::detach(world, component.instance) {
        bail!("Could not remove {} from '{id}'.", component.class_name);
    }
    Ok(component.class_name)
}

/// Delete objects and everything below them. Returns the number of objects
/// removed (descendants included).
pub fn delete_objects(world: &mut World, rec: &mut Recorder, ids: &[String]) -> Result<usize> {
    if ids.is_empty() {
        bail!("Pass `id` or `ids`.");
    }
    let index = SceneIndex::build(world);
    let mut roots: Vec<Entity> = Vec::new();
    for id in ids {
        let entity = require_object(&index, id)?;
        if !roots.contains(&entity) {
            roots.push(entity);
        }
    }
    // An object inside another deleted subtree goes with it.
    let all = roots.clone();
    roots.retain(|&e| !all.iter().any(|&other| other != e && index.is_descendant(e, other)));
    let mut removed = 0;
    for &root in &roots {
        let id = index.id_of(root).unwrap_or_default().to_string();
        removed += index.subtree(root).len();
        rec.note_subtree(world, &index, &id);
    }
    for &root in &roots {
        world.despawn_tree(root);
    }
    Ok(removed)
}
