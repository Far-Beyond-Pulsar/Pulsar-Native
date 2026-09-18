//! Scene objects: queries, creation and removal, hierarchy, and name / visibility /
//! transform edits -- all directly on the world's components.
//!
//! An object is an entity carrying `StableId`, `Name`, `Transform`, `Visibility`,
//! `ObjectType`, `SiblingIndex` and `RenderProps`; parent links are a `Parent`
//! component and selection is a `Selected` marker. See
//! [`engine_backend::scene::SceneWorldExt`] for the helpers used here.

use engine_backend::scene::{
    Name, ObjectType, RenderProps, SceneWorldExt, SpawnObject, StableId,
    Transform as WorldTransform, Visibility as WorldVisibility,
};
use pulsar_scenedb::{Entity, World};

use super::components::{
    attach_component_instance, clear_components, descendant_ids, get_components,
    get_components_metadata, merge_component_props, sync_registered_component_props_to_scene_db,
};
use super::{
    find_script_path, static_mesh_component_json, ComponentInstance, ObjectId, SceneObjectData,
    Transform,
};

// ── Read model ─────────────────────────────────────────────────────────────

/// Build a [`SceneObjectData`] for `entity` straight off its components: transform,
/// name, visibility, type and render props plus the derived `parent` / `children`
/// / `scene_path`. Does NOT merge live component props (see
/// [`merge_component_props`]); callers that need that do it afterward.
pub fn entity_to_scene_object_data(world: &World, entity: Entity) -> SceneObjectData {
    let transform = world
        .get::<WorldTransform>(entity)
        .copied()
        .unwrap_or_default();
    let visibility = world
        .get::<WorldVisibility>(entity)
        .copied()
        .unwrap_or_default();
    let render_props = world
        .get::<RenderProps>(entity)
        .cloned()
        .unwrap_or_default();
    let id_of = |e: Entity| world.stable_id_of(e).map(str::to_string);

    SceneObjectData {
        id: id_of(entity).unwrap_or_default(),
        name: name_of(world, entity),
        object_type: world
            .get::<ObjectType>(entity)
            .copied()
            .unwrap_or(ObjectType::Empty),
        transform: transform.into(),
        visible: visibility.visible,
        locked: visibility.locked,
        parent: world.parent_of(entity).and_then(id_of),
        children: world
            .children_of(Some(entity))
            .into_iter()
            .filter_map(id_of)
            .collect(),
        scene_path: scene_path(world, entity),
        props: render_props.props,
        component_instances: render_props.component_instances,
    }
}

fn name_of(world: &World, entity: Entity) -> String {
    world
        .get::<Name>(entity)
        .map(|name| name.0.clone())
        .unwrap_or_default()
}

/// Name-joined path from the root to `entity` (`"Parent/Child"`), recomputed on
/// every read rather than stored.
fn scene_path(world: &World, entity: Entity) -> String {
    let mut parts = vec![name_of(world, entity)];
    let mut current = world.parent_of(entity);
    while let Some(parent) = current {
        parts.push(name_of(world, parent));
        current = world.parent_of(parent);
    }
    parts.reverse();
    parts.join("/")
}

pub(super) fn collect_dfs(world: &World, parent: Option<Entity>, out: &mut Vec<SceneObjectData>) {
    for entity in world.children_of(parent) {
        out.push(entity_to_scene_object_data(world, entity));
        collect_dfs(world, Some(entity), out);
    }
}

// ── Queries ────────────────────────────────────────────────────────────────

/// All objects in depth-first order.
pub fn get_all_objects(world: &World) -> Vec<SceneObjectData> {
    let mut out = Vec::new();
    collect_dfs(world, None, &mut out);
    for obj in &mut out {
        merge_component_props(world, &obj.id.clone(), &mut obj.props);
    }
    out
}

/// Root-level objects (no parent).
pub fn get_root_objects(world: &World) -> Vec<SceneObjectData> {
    world
        .children_of(None)
        .into_iter()
        .map(|e| entity_to_scene_object_data(world, e))
        .collect()
}

/// The hierarchy projection from one read: every object in DFS order plus the
/// root ids. Keeping them together prevents the UI from observing two different
/// revisions during a background mutation.
pub fn get_hierarchy_snapshot(world: &World) -> (Vec<SceneObjectData>, Vec<ObjectId>) {
    let objects = get_all_objects(world);
    let root_ids = world
        .children_of(None)
        .into_iter()
        .filter_map(|entity| world.stable_id_of(entity).map(str::to_string))
        .collect();
    (objects, root_ids)
}

/// Number of root-level objects, without building their data.
pub fn root_count(world: &World) -> usize {
    world.children_of(None).len()
}

// ── Targeted reads ─────────────────────────────────────────────────────────
//
// The bound-field editors refresh these on every scene revision bump under the
// current selection (gizmo drags, AI edits, typing). Each reads exactly one
// component and allocates almost nothing, unlike [`get_object`], whose cost is
// O(object data).

/// Just the object's transform.
pub fn get_object_transform(world: &World, id: &str) -> Option<Transform> {
    let entity = world.entity_for(id)?;
    world.get::<WorldTransform>(entity).copied().map(Into::into)
}

/// Just the object's name.
pub fn get_object_name(world: &World, id: &str) -> Option<String> {
    let entity = world.entity_for(id)?;
    Some(world.get::<Name>(entity)?.0.clone())
}

/// Just the object's `(visible, locked)` flags.
pub fn get_object_visibility(world: &World, id: &str) -> Option<(bool, bool)> {
    let entity = world.entity_for(id)?;
    let v = world.get::<WorldVisibility>(entity)?;
    Some((v.visible, v.locked))
}

/// Single object by ID, `None` if not found.
pub fn get_object(world: &World, id: &str) -> Option<SceneObjectData> {
    let entity = world.entity_for(id)?;
    let mut data = entity_to_scene_object_data(world, entity);
    merge_component_props(world, id, &mut data.props);
    Some(data)
}

/// Direct children of `id`.
pub fn get_children(world: &World, id: &str) -> Vec<ObjectId> {
    let Some(entity) = world.entity_for(id) else {
        return Vec::new();
    };
    world
        .children_of(Some(entity))
        .into_iter()
        .filter_map(|e| world.stable_id_of(e).map(str::to_string))
        .collect()
}

// ── Selection ──────────────────────────────────────────────────────────────

/// Select an object by id (`None` deselects). An unknown id is treated as a
/// deselect: selecting a since-removed id is a normal race between UI and render
/// threads, not an error.
pub fn select_object(world: &mut World, id: Option<&str>) {
    let entity = id.and_then(|id| world.entity_for(id));
    world.select(entity);
}

pub fn get_selected_object_id(world: &World) -> Option<ObjectId> {
    world.selected_id()
}

pub fn get_selected_object(world: &World) -> Option<SceneObjectData> {
    let entity = world.selected_entity()?;
    Some(entity_to_scene_object_data(world, entity))
}

// ── Creation / removal ─────────────────────────────────────────────────────

/// Add an object. Returns the assigned `ObjectId`, or an empty string when the
/// caller-supplied id or parent is invalid (an add must never turn a duplicate or
/// stale reference into a different object while the caller keeps using the
/// original ID).
///
/// Blueprint objects always receive a `ScriptComponent` pointing at their
/// blueprint directory: the projection rebuilds `__component_instances` from the
/// attachments, so it must live there.
pub fn add_object(world: &mut World, obj: SceneObjectData, parent: Option<ObjectId>) -> ObjectId {
    if !obj.id.is_empty() && world.entity_for(&obj.id).is_some() {
        tracing::error!(id = %obj.id, "rejected duplicate object ID");
        return String::new();
    }
    if let Some(parent_id) = parent.as_deref() {
        if world.entity_for(parent_id).is_none() {
            tracing::error!(parent = %parent_id, "rejected missing parent");
            return String::new();
        }
    }

    // v2 scene objects may carry component instances inline. Preserve those
    // instances before the normal hydration pass; otherwise World-registered
    // components (notably StaticMeshComponent) never reach the live world.
    let mut inline_components = obj
        .component_instances
        .as_ref()
        .and_then(serde_json::Value::as_array)
        .map(|instances| {
            instances
                .iter()
                .filter_map(|instance| {
                    let object = instance.as_object()?;
                    Some(ComponentInstance {
                        class_name: object.get("class_name")?.as_str()?.to_string(),
                        enabled: object
                            .get("enabled")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(true),
                        data: object
                            .get("data")
                            .cloned()
                            .unwrap_or(serde_json::Value::Null),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    // A few older scene files projected StaticMeshComponent's asset path into
    // `props` without a component-instances entry. Treat that as a load-time
    // compatibility form so those scenes also get a typed component.
    if inline_components.is_empty() {
        if let Some(mesh_asset) = obj
            .props
            .get("mesh_asset")
            .and_then(serde_json::Value::as_str)
            .filter(|path| !path.trim().is_empty())
        {
            inline_components.push(ComponentInstance {
                class_name: "StaticMeshComponent".to_string(),
                enabled: true,
                data: static_mesh_component_json(mesh_asset),
            });
        }
    }
    let blueprint_script_path = (obj.object_type == ObjectType::Blueprint)
        .then(|| find_script_path(&obj.props, obj.component_instances.as_ref()));

    let parent_entity = parent.as_deref().and_then(|p| world.entity_for(p));
    let spec = SpawnObject {
        stable_id: (!obj.id.is_empty()).then(|| obj.id.clone()),
        name: obj.name.clone(),
        parent: parent_entity,
        transform: (&obj.transform).into(),
        visibility: WorldVisibility {
            visible: obj.visible,
            locked: obj.locked,
        },
        object_type: obj.object_type,
    };
    let entity = match world.spawn_object(spec.clone()) {
        Ok(entity) => entity,
        Err(err) => {
            tracing::warn!("add_object: {err}, auto-assigning a new id");
            let mut spec = spec;
            spec.stable_id = None;
            world
                .spawn_object(spec)
                .expect("auto-assigned stable id cannot collide")
        }
    };
    let object_id = world.stable_id_of(entity).unwrap_or_default().to_string();
    if let Some(mut render_props) = world.get_mut::<RenderProps>(entity) {
        render_props.props = obj.props;
        render_props.component_instances = obj.component_instances;
    }

    for component in inline_components {
        attach_component_instance(world, &object_id, component, false);
    }

    if let Some(script_path) = blueprint_script_path {
        let already_has = get_components_metadata(world, &object_id)
            .iter()
            .any(|c| c.class_name == "ScriptComponent");
        if !already_has {
            attach_component_instance(
                world,
                &object_id,
                ComponentInstance {
                    class_name: "ScriptComponent".to_string(),
                    enabled: true,
                    data: serde_json::json!({ "script_asset": script_path }),
                },
                false,
            );
        }
    }

    sync_registered_component_props_to_scene_db(world, &object_id);
    object_id
}

/// Add a folder object. Returns its id.
pub fn add_folder(world: &mut World, name: &str, parent: Option<ObjectId>) -> ObjectId {
    let obj = SceneObjectData {
        id: String::new(),
        name: name.to_string(),
        object_type: ObjectType::Folder,
        transform: Transform::default(),
        visible: true,
        locked: false,
        parent: parent.clone(),
        children: vec![],
        scene_path: String::new(),
        props: Default::default(),
        component_instances: None,
    };
    add_object(world, obj, parent)
}

/// Remove an object and all of its descendants. Returns `true` if found.
pub fn remove_object(world: &mut World, id: &str) -> bool {
    let Some(entity) = world.entity_for(id) else {
        return false;
    };
    // Registered typed components are torn down with the entity; clear the
    // attachment records first so no dangling component state outlives it.
    let mut ids = vec![id.to_string()];
    descendant_ids(world, entity, &mut ids);
    for object_id in &ids {
        clear_components(world, object_id);
    }
    world.despawn_tree(entity);
    true
}

/// Remove every object. Despawn is recursive, so one pass over all ids is enough.
pub fn clear(world: &mut World) {
    let ids: Vec<ObjectId> = world
        .query::<&StableId>()
        .map(|(_, id)| id.0.clone())
        .collect();
    for id in ids {
        if let Some(entity) = world.entity_for(&id) {
            world.despawn_tree(entity);
        }
    }
    tracing::info!("Scene cleared – ready for new level");
}

/// Write updated transform, name, visibility and props back to an existing object.
pub fn update_object(world: &mut World, obj: SceneObjectData) -> bool {
    let id = obj.id.clone();
    let Some(entity) = world.entity_for(&id) else {
        return false;
    };
    world.insert(entity, WorldTransform::from(&obj.transform));
    world.insert(entity, Name(obj.name));
    world.insert(
        entity,
        WorldVisibility {
            visible: obj.visible,
            locked: obj.locked,
        },
    );
    if let Some(mut render_props) = world.get_mut::<RenderProps>(entity) {
        render_props.props = obj.props;
    }
    sync_registered_component_props_to_scene_db(world, &id);
    true
}

// ── Property setters ───────────────────────────────────────────────────────

pub fn set_name(world: &mut World, id: &str, name: String) -> bool {
    match world.entity_for(id) {
        Some(entity) if world.get::<Name>(entity).is_some() => {
            world.insert(entity, Name(name));
            true
        }
        _ => false,
    }
}

fn edit_visibility(world: &mut World, id: &str, edit: impl FnOnce(&mut WorldVisibility)) -> bool {
    let Some(entity) = world.entity_for(id) else {
        return false;
    };
    let mut visibility = world
        .get::<WorldVisibility>(entity)
        .copied()
        .unwrap_or_default();
    edit(&mut visibility);
    world.insert(entity, visibility);
    true
}

pub fn set_visible(world: &mut World, id: &str, visible: bool) -> bool {
    edit_visibility(world, id, |v| v.visible = visible)
}

pub fn set_locked(world: &mut World, id: &str, locked: bool) -> bool {
    edit_visibility(world, id, |v| v.locked = locked)
}

/// Narrow transform update -- writes only the transform component, with no
/// whole-object round trip and no component re-hydration. `None` fields are left
/// unchanged; returns `false` if nothing actually changed or the object doesn't
/// exist.
pub fn set_transform(
    world: &mut World,
    id: &str,
    position: Option<[f32; 3]>,
    rotation: Option<[f32; 3]>,
    scale: Option<[f32; 3]>,
) -> bool {
    let Some(entity) = world.entity_for(id) else {
        return false;
    };
    let Some(mut transform) = world.get_mut::<WorldTransform>(entity) else {
        return false;
    };
    let mut changed = false;
    if let Some(p) = position.filter(|p| transform.position != *p) {
        transform.position = p;
        changed = true;
    }
    if let Some(r) = rotation.filter(|r| transform.rotation != *r) {
        transform.rotation = r;
        changed = true;
    }
    if let Some(s) = scale.filter(|s| transform.scale != *s) {
        transform.scale = s;
        changed = true;
    }
    changed
}

/// Re-parent an object (cycle-safe).
pub fn reparent_object(world: &mut World, id: &str, new_parent: Option<ObjectId>) -> bool {
    let Some(entity) = world.entity_for(id) else {
        return false;
    };
    let new_parent_entity = match new_parent {
        Some(ref parent_id) => match world.entity_for(parent_id) {
            Some(e) => Some(e),
            None => return false,
        },
        None => None,
    };
    world.reparent(entity, new_parent_entity).is_ok()
}

/// Reorder two sibling objects by swapping their positions. Both must share a
/// parent.
pub fn reorder_object_siblings(world: &mut World, object_id: &str, target_id: &str) -> bool {
    let (Some(entity), Some(target)) = (world.entity_for(object_id), world.entity_for(target_id))
    else {
        return false;
    };
    world.reorder_sibling(entity, target)
}

/// Move an object one step earlier among its siblings. No-op if already first.
pub fn move_object_up(world: &mut World, id: &str) {
    if let Some(entity) = world.entity_for(id) {
        world.move_sibling_up(entity);
    }
}

/// Move an object one step later among its siblings. No-op if already last.
pub fn move_object_down(world: &mut World, id: &str) {
    if let Some(entity) = world.entity_for(id) {
        world.move_sibling_down(entity);
    }
}

/// Shallow-duplicate an object (children are not copied). Returns the new ID.
pub fn duplicate_object(world: &mut World, id: &str) -> Option<ObjectId> {
    let source_components = get_components(world, id);
    let mut obj = get_object(world, id)?;
    obj.id = String::new(); // force auto-assign
    obj.name = format!("{} (Copy)", obj.name);
    obj.children = vec![];
    let parent = obj.parent.clone();
    let new_id = add_object(world, obj, parent);
    if new_id.is_empty() {
        return None;
    }

    clear_components(world, &new_id);
    for component in source_components {
        attach_component_instance(world, &new_id, component, false);
    }
    sync_registered_component_props_to_scene_db(world, &new_id);

    Some(new_id)
}

/// Whether `parent` may be reparented under `new_parent` without a cycle. Kept
/// public for callers that want to validate before issuing a command.
pub fn can_reparent(world: &World, id: &str, new_parent: Option<&str>) -> bool {
    let Some(entity) = world.entity_for(id) else {
        return false;
    };
    let mut cursor = new_parent.and_then(|p| world.entity_for(p));
    if new_parent.is_some() && cursor.is_none() {
        return false;
    }
    while let Some(current) = cursor {
        if current == entity {
            return false;
        }
        cursor = world.parent_of(current);
    }
    true
}

/// Spawn `obj` exactly as stored -- id, parent, transform, visibility, type and
/// the raw render props -- without running component hydration. The caller
/// attaches components afterwards. Used to rebuild a scene from a snapshot.
pub(super) fn spawn_raw(world: &mut World, obj: &SceneObjectData) -> Result<Entity, String> {
    let parent = match &obj.parent {
        Some(parent_id) => Some(
            world
                .entity_for(parent_id)
                .ok_or_else(|| format!("object '{}' references unknown parent '{parent_id}'", obj.id))?,
        ),
        None => None,
    };
    let entity = world
        .spawn_object(SpawnObject {
            stable_id: Some(obj.id.clone()),
            name: obj.name.clone(),
            parent,
            transform: (&obj.transform).into(),
            visibility: WorldVisibility {
                visible: obj.visible,
                locked: obj.locked,
            },
            object_type: obj.object_type,
        })
        .map_err(|e| e.to_string())?;
    if let Some(mut render_props) = world.get_mut::<RenderProps>(entity) {
        render_props.props = obj.props.clone();
        render_props.component_instances = obj.component_instances.clone();
    }
    Ok(entity)
}
