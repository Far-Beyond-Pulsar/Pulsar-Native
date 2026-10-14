//! Typed object snapshots: the unit the transaction journal, the sandbox fork
//! and the diff are built from.
//!
//! A snapshot clones an object's own components (name, transform, visibility,
//! type, free-form props, sibling order, parent) and its attached component
//! instances through the world registry's typed snapshots, so nothing is
//! encoded to JSON to undo or fork. It is the same capture the level editor's
//! undo history uses, kept here so the agent crate does not depend on editor
//! code. Components that are not attached instances and are not listed above
//! (selection markers, runtime-only state) are not captured.

use std::collections::HashMap;

use pulsar_scene_model::attachments;
use pulsar_scene_model::{
    Name, ObjectType, Parent, RenderProps, SceneWorldExt, SiblingIndex, SpawnObject, StableId,
    Transform, Visibility,
};
use pulsar_scenedb::{Entity, World};
use pulsar_world_registry::{
    restore_instances, snapshot_instances, InstanceSnapshot, InstanceValue,
};
use serde_json::Value;

/// A restorable copy of one scene object.
#[derive(Clone, Debug)]
pub struct ObjectSnapshot {
    pub id: String,
    pub name: String,
    pub object_type: ObjectType,
    pub transform: Transform,
    pub visibility: Visibility,
    /// Stable id of the parent object.
    pub parent: Option<String>,
    pub sibling: SiblingIndex,
    pub props: RenderProps,
    pub components: Vec<InstanceSnapshot>,
}

/// Identity and hierarchy of a world, built once per operation. SceneDB's
/// `children_of` and `entity_for` scan every object, so loops that visit many
/// objects go through this instead.
pub struct SceneIndex {
    by_id: HashMap<String, Entity>,
    ids: HashMap<Entity, String>,
    parents: HashMap<Entity, Option<Entity>>,
    children: HashMap<Option<Entity>, Vec<Entity>>,
}

impl SceneIndex {
    pub fn build(world: &World) -> Self {
        let mut rows: Vec<(Entity, String)> = world
            .query::<&StableId>()
            .map(|(entity, id)| (entity, id.0.clone()))
            .collect();
        // Entity bits break ties so the order is total and stable.
        rows.sort_by_key(|(entity, _)| entity.bits());
        let by_id: HashMap<String, Entity> =
            rows.iter().map(|(e, id)| (id.clone(), *e)).collect();
        let ids: HashMap<Entity, String> = rows.iter().cloned().collect();
        let mut parents = HashMap::with_capacity(rows.len());
        let mut sorted: Vec<(Option<Entity>, SiblingIndex, Entity)> = Vec::with_capacity(rows.len());
        for (entity, _) in &rows {
            let parent = world
                .get::<Parent>(*entity)
                .map(|p| p.0)
                // A parent that is gone or not an object makes this a root.
                .filter(|p| ids.contains_key(p));
            let order = world.get::<SiblingIndex>(*entity).copied().unwrap_or_default();
            parents.insert(*entity, parent);
            sorted.push((parent, order, *entity));
        }
        sorted.sort_by_key(|(_, order, entity)| (*order, entity.bits()));
        let mut children: HashMap<Option<Entity>, Vec<Entity>> = HashMap::new();
        for (parent, _, entity) in sorted {
            children.entry(parent).or_default().push(entity);
        }
        Self {
            by_id,
            ids,
            parents,
            children,
        }
    }

    pub fn len(&self) -> usize {
        self.ids.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }

    pub fn entity(&self, id: &str) -> Option<Entity> {
        self.by_id.get(id).copied()
    }

    pub fn id_of(&self, entity: Entity) -> Option<&str> {
        self.ids.get(&entity).map(String::as_str)
    }

    pub fn parent_of(&self, entity: Entity) -> Option<Entity> {
        self.parents.get(&entity).copied().flatten()
    }

    pub fn parent_id(&self, entity: Entity) -> Option<&str> {
        self.parent_of(entity).and_then(|p| self.id_of(p))
    }

    pub fn children(&self, parent: Option<Entity>) -> &[Entity] {
        self.children.get(&parent).map(Vec::as_slice).unwrap_or(&[])
    }

    /// Every object, parents before their children, siblings in order.
    pub fn all_ordered(&self) -> Vec<Entity> {
        let mut out = Vec::with_capacity(self.len());
        for root in self.children(None) {
            self.collect_subtree(*root, &mut out);
        }
        out
    }

    /// `root` and all of its descendants, parents first.
    pub fn subtree(&self, root: Entity) -> Vec<Entity> {
        let mut out = Vec::new();
        self.collect_subtree(root, &mut out);
        out
    }

    fn collect_subtree(&self, entity: Entity, out: &mut Vec<Entity>) {
        out.push(entity);
        for child in self.children(Some(entity)) {
            self.collect_subtree(*child, out);
        }
    }

    pub fn is_descendant(&self, entity: Entity, ancestor: Entity) -> bool {
        let mut cursor = self.parent_of(entity);
        while let Some(current) = cursor {
            if current == ancestor {
                return true;
            }
            cursor = self.parent_of(current);
        }
        false
    }

    /// Ids that contain `needle` (lower case), for "did you mean" hints.
    pub fn similar_ids(&self, needle: &str, limit: usize) -> Vec<String> {
        let needle = needle.to_lowercase();
        let mut found: Vec<String> = self
            .by_id
            .keys()
            .filter(|id| {
                let lower = id.to_lowercase();
                !needle.is_empty() && (lower.contains(&needle) || needle.contains(&lower))
            })
            .cloned()
            .collect();
        found.sort();
        found.truncate(limit);
        found
    }
}

/// Snapshot of `entity`, or `None` if it is not a scene object.
pub fn snapshot_object(world: &World, index: &SceneIndex, entity: Entity) -> Option<ObjectSnapshot> {
    let id = index.id_of(entity)?.to_string();
    Some(ObjectSnapshot {
        name: world.get::<Name>(entity).map(|n| n.0.clone()).unwrap_or_default(),
        object_type: world.get::<ObjectType>(entity).copied().unwrap_or(ObjectType::Empty),
        transform: world.get::<Transform>(entity).copied().unwrap_or_default(),
        visibility: world.get::<Visibility>(entity).copied().unwrap_or_default(),
        parent: index.parent_id(entity).map(str::to_string),
        sibling: world.get::<SiblingIndex>(entity).copied().unwrap_or_default(),
        props: world.get::<RenderProps>(entity).cloned().unwrap_or_default(),
        components: snapshot_instances(world, entity),
        id,
    })
}

/// Snapshots of `root` and its descendants, parents first.
pub fn snapshot_subtree(world: &World, index: &SceneIndex, root: Entity) -> Vec<ObjectSnapshot> {
    index
        .subtree(root)
        .into_iter()
        .filter_map(|e| snapshot_object(world, index, e))
        .collect()
}

/// Make the world hold `snapshot`: the object is created when missing and
/// otherwise written in place (it keeps its entity), its components are made
/// equal to the snapshot's, and its parent is set. `ids` maps stable ids to
/// entities for this world and is updated for a created object. The parent
/// must already be in `ids`.
pub fn apply_snapshot(
    world: &mut World,
    ids: &mut HashMap<String, Entity>,
    snapshot: &ObjectSnapshot,
) -> Result<Entity, String> {
    let parent = match &snapshot.parent {
        Some(parent_id) => Some(*ids.get(parent_id).ok_or_else(|| {
            format!(
                "object '{}' needs its parent '{parent_id}', which does not exist",
                snapshot.id
            )
        })?),
        None => None,
    };
    let entity = match ids.get(&snapshot.id).copied().filter(|e| world.is_alive(*e)) {
        Some(entity) => {
            world.insert(entity, Name(snapshot.name.clone()));
            world.insert(entity, snapshot.object_type);
            world.insert(entity, snapshot.transform);
            world.insert(entity, snapshot.visibility);
            if world.parent_of(entity) != parent {
                world.reparent(entity, parent).map_err(|e| e.to_string())?;
            }
            entity
        }
        None => {
            let entity = world
                .spawn_object(SpawnObject {
                    stable_id: Some(snapshot.id.clone()),
                    name: snapshot.name.clone(),
                    parent,
                    transform: snapshot.transform,
                    visibility: snapshot.visibility,
                    object_type: snapshot.object_type,
                })
                .map_err(|e| e.to_string())?;
            ids.insert(snapshot.id.clone(), entity);
            entity
        }
    };
    world.insert(entity, snapshot.props.clone());
    world.insert(entity, snapshot.sibling);
    restore_instances(world, entity, &snapshot.components)
        .map_err(|e| format!("components of '{}': {e}", snapshot.id))?;
    Ok(entity)
}

/// Apply several snapshots, parents before children regardless of the order
/// given (a parent that is in the list but later is applied first).
pub fn apply_snapshots(
    world: &mut World,
    ids: &mut HashMap<String, Entity>,
    snapshots: &[ObjectSnapshot],
) -> Result<(), String> {
    let mut pending: Vec<&ObjectSnapshot> = snapshots.iter().collect();
    while !pending.is_empty() {
        let before = pending.len();
        let mut deferred = Vec::new();
        for snapshot in pending {
            let ready = snapshot.parent.as_ref().is_none_or(|p| ids.contains_key(p));
            if ready {
                apply_snapshot(world, ids, snapshot)?;
            } else {
                deferred.push(snapshot);
            }
        }
        if deferred.len() == before {
            let names: Vec<&str> = deferred.iter().map(|s| s.id.as_str()).collect();
            return Err(format!("parents are missing for: {}", names.join(", ")));
        }
        pending = deferred;
    }
    Ok(())
}

// ── Equality and JSON views ──────────────────────────────────────────────────

/// A component class's data as JSON: its serialized form, or the reflected
/// property map for classes that do not serialize.
pub fn class_json(value: &dyn pulsar_reflection::EngineClass) -> Value {
    value.to_json().unwrap_or_else(|_| {
        Value::Object(
            value
                .get_properties()
                .iter()
                .map(|prop| {
                    let json = pulsar_reflection::RUNTIME_TYPE_REGISTRY
                        .serialize_json_for_any((prop.getter)(value).as_ref())
                        .unwrap_or(Value::Null);
                    (prop.name.to_string(), json)
                })
                .collect(),
        )
    })
}

/// The data of a snapshotted instance as JSON.
pub fn instance_json(snapshot: &InstanceSnapshot) -> Value {
    match &snapshot.value {
        InstanceValue::Value(value) => {
            pulsar_world_registry::value_engine_class(&snapshot.meta.class_name, value.as_ref())
                .map(class_json)
                .unwrap_or(Value::Null)
        }
        InstanceValue::Unresolved(unresolved) => unresolved.data.clone(),
    }
}

fn instances_equal(a: &InstanceSnapshot, b: &InstanceSnapshot) -> bool {
    if a.meta != b.meta || a.enabled != b.enabled {
        return false;
    }
    match (&a.value, &b.value) {
        (InstanceValue::Unresolved(x), InstanceValue::Unresolved(y)) => x == y,
        (InstanceValue::Value(_), InstanceValue::Value(_)) => instance_json(a) == instance_json(b),
        _ => false,
    }
}

/// Do the two snapshots hold the same object state?
pub fn snapshots_equal(a: &ObjectSnapshot, b: &ObjectSnapshot) -> bool {
    a.id == b.id
        && a.name == b.name
        && a.object_type == b.object_type
        && a.transform == b.transform
        && a.visibility == b.visibility
        && a.parent == b.parent
        && a.props == b.props
        && a.components.len() == b.components.len()
        && a
            .components
            .iter()
            .zip(&b.components)
            .all(|(x, y)| instances_equal(x, y))
}

/// Every object in the world, parents first.
pub fn capture_all(world: &World) -> Vec<ObjectSnapshot> {
    let index = SceneIndex::build(world);
    index
        .all_ordered()
        .into_iter()
        .filter_map(|e| snapshot_object(world, &index, e))
        .collect()
}

/// Component instance entities of an object (owner order).
pub fn instance_entities(world: &World, owner: Entity) -> Vec<Entity> {
    attachments::instances(world, owner)
}

pub(crate) fn instance_equal(a: &InstanceSnapshot, b: &InstanceSnapshot) -> bool {
    instances_equal(a, b)
}
