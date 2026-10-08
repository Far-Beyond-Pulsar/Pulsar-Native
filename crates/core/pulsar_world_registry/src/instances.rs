//! Attaching registered component classes as component-instance entities
//! (Pulsar-Native#1035, D1), and their JSON records at boundaries.
//!
//! [`pulsar_scene_model::attachments`] owns the structure (instance
//! entities, owner links, order, ids). This module adds what needs the class
//! registry: producing the instance's typed value -- the class factory, a
//! caller-supplied owned value, or a boundary decode -- and inserting it
//! through SceneDB's erased insert, so every write hook runs.
//!
//! Attaching validates everything before writing anything: a refused attach
//! leaves the world untouched, and never leaves an instance that looks
//! attached without its value. A payload this build cannot turn into a live
//! value is attached only when the caller asks for that explicitly
//! ([`attach_record_or_unresolved`]), as an [`UnresolvedComponent`].
//!
//! JSON appears only in [`ComponentRecord`] conversions, for files and
//! external tools. Undo history and play mode use typed
//! [`InstanceSnapshot`]s.

use std::any::Any;

use pulsar_reflection::EngineClass;
use pulsar_scene_model::attachments::{
    self, ComponentAttachments, ComponentInstanceId, ComponentMeta, InstanceError, NewInstance,
    UnresolvedComponent,
};
use pulsar_scene_model::{ClassSlot, ComponentInstance as ComponentRecord, Transform};
use pulsar_scenedb::{Entity, World};
use serde_json::{Map, Value};

use crate::values::{insert_world_component_value, ComponentValueError};

/// Record metadata key: the class-prefab slot an instance was placed from.
pub const SLOT_ID_KEY: &str = "__slot_id";
/// Record metadata key: a slot component's local transform.
pub const TRANSFORM_KEY: &str = "__transform";
/// Record metadata key: the parent instance's position in the record list.
pub const PARENT_INDEX_KEY: &str = "__parent_index";
/// Record metadata key: the instance's stable [`ComponentInstanceId`].
pub const INSTANCE_ID_KEY: &str = "__instance_id";

/// Where an attached instance's value comes from.
pub enum ComponentPayload {
    /// The class's default value (its factory).
    Default,
    /// An owned value of the class, e.g. a clone.
    Value(Box<dyn Any + Send + Sync>),
    /// The class's JSON representation, decoded once at this boundary.
    Json(Value),
}

/// Why an attach was refused. Nothing was written.
#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error(transparent)]
    Instance(#[from] InstanceError),
    #[error("`{0}` is not a registered component class")]
    UnknownClass(String),
    #[error("`{class}` data does not decode: {error}")]
    Decode { class: String, error: String },
    #[error(transparent)]
    Value(#[from] ComponentValueError),
}

/// Attach a new instance of the registered class `spec.class_name` to
/// `owner`, with its value from `payload`. Returns the instance entity.
pub fn attach_component(
    world: &mut World,
    owner: Entity,
    spec: NewInstance,
    payload: ComponentPayload,
) -> Result<Entity, AttachError> {
    let class = spec.class_name.clone();
    let Some(registration) = crate::find(&class) else {
        return Err(AttachError::UnknownClass(class));
    };
    let value = match payload {
        ComponentPayload::Default => (registration.default_value)(),
        ComponentPayload::Value(value) => value,
        ComponentPayload::Json(data) => {
            (registration.decode)(&data).map_err(|error| AttachError::Decode {
                class: class.clone(),
                error,
            })?
        }
    };
    let expected = (registration.register_erased)();
    if pulsar_scenedb::component::try_resolve_id((*value).type_id()) != Some(expected) {
        return Err(ComponentValueError::TypeMismatch { class, value }.into());
    }
    let instance = attachments::spawn_instance(world, owner, spec)?;
    // Cannot fail now: the instance is alive and the value's type was checked.
    insert_world_component_value(&class, world, instance, value)
        .expect("a freshly spawned instance takes a value of its own class");
    crate::unsupported::report_attach(&class);
    Ok(instance)
}

/// Attach typed `value` to `owner` as a new instance of its class `T`.
/// The typed convenience over [`attach_component`].
pub fn attach_value<T: EngineClass>(
    world: &mut World,
    owner: Entity,
    value: T,
) -> Result<Entity, AttachError> {
    attach_component(
        world,
        owner,
        NewInstance::new(T::class_name()),
        ComponentPayload::Value(Box::new(value)),
    )
}

/// A class's default value decoded once, for generated actor code that
/// attaches the same prefab component to every actor it spawns
/// ([`attach_cached_default`]).
pub type DefaultCache = std::sync::OnceLock<Result<Box<dyn Any + Send + Sync>, String>>;

/// Attach a new instance of `class_name` to `owner` holding a clone of the
/// default `json` describes, decoded once into `cache` (the first call) and
/// cloned for every later one. Refused like [`attach_component`] for an
/// unregistered class or data that does not decode.
pub fn attach_cached_default(
    world: &mut World,
    owner: Entity,
    class_name: &str,
    json: &str,
    cache: &DefaultCache,
) -> Result<Entity, AttachError> {
    let decoded = cache.get_or_init(|| {
        let data =
            serde_json::from_str::<Value>(json).unwrap_or_else(|_| Value::Object(Map::new()));
        match crate::find(class_name) {
            Some(registration) => (registration.decode)(&data),
            None => Err(format!(
                "`{class_name}` is not a registered component class"
            )),
        }
    });
    let default = decoded.as_ref().map_err(|error| AttachError::Decode {
        class: class_name.to_string(),
        error: error.clone(),
    })?;
    let value = crate::values::clone_value(class_name, default.as_ref())
        .ok_or_else(|| AttachError::UnknownClass(class_name.to_string()))?;
    attach_component(
        world,
        owner,
        NewInstance::new(class_name),
        ComponentPayload::Value(value),
    )
}

/// Attach a payload this build cannot use as a live component -- an
/// unregistered class, or data that does not decode -- as an explicit
/// [`UnresolvedComponent`] that keeps the payload for lossless saving.
pub fn attach_unresolved(
    world: &mut World,
    owner: Entity,
    spec: NewInstance,
    data: Value,
    reason: String,
) -> Result<Entity, InstanceError> {
    let instance = attachments::spawn_instance(world, owner, spec)?;
    world.insert(instance, UnresolvedComponent { data, reason });
    Ok(instance)
}

/// The instance's class value as `&dyn EngineClass` (reads), or `None` if it
/// is unresolved or not an instance.
pub fn instance_engine_class(world: &World, instance: Entity) -> Option<&dyn EngineClass> {
    let class = attachments::meta(world, instance)?.class_name.as_str();
    crate::get_world_component_as_engine_class(class, world, instance)
}

/// A clone of the instance's value, for duplication.
pub fn clone_instance_value(world: &World, instance: Entity) -> Option<Box<dyn Any + Send + Sync>> {
    let class = attachments::meta(world, instance)?.class_name.as_str();
    crate::clone_world_component_value(class, world, instance)
}

/// Duplicate `instance` onto `owner` (the same object or another): a clone
/// of its value (or of its unresolved payload) with a fresh id, inserted at
/// `index`. Class-slot provenance and parent links are not copied.
pub fn duplicate_instance(
    world: &mut World,
    instance: Entity,
    owner: Entity,
    index: Option<usize>,
) -> Result<Entity, AttachError> {
    let Some(meta) = attachments::meta(world, instance).cloned() else {
        return Err(AttachError::Instance(InstanceError::DeadOwner(instance)));
    };
    let mut spec = NewInstance::new(meta.class_name.clone());
    spec.enabled = attachments::is_enabled(world, instance);
    spec.index = index;
    if let Some(unresolved) = world.get::<UnresolvedComponent>(instance).cloned() {
        return Ok(attach_unresolved(
            world,
            owner,
            spec,
            unresolved.data,
            unresolved.reason,
        )?);
    }
    let value = clone_instance_value(world, instance)
        .ok_or_else(|| AttachError::UnknownClass(meta.class_name.clone()))?;
    attach_component(world, owner, spec, ComponentPayload::Value(value))
}

/// Duplicate the instances of `from` that `keep` selects onto `to`, in
/// order: clones of their values with fresh ids, the parent links among the
/// copies restored, and class-slot provenance copied only when `keep_slots`.
/// Returns the new instance entities.
pub fn duplicate_instances(
    world: &mut World,
    from: Entity,
    to: Entity,
    keep: impl Fn(&World, Entity) -> bool,
    keep_slots: bool,
) -> Result<Vec<Entity>, AttachError> {
    let sources: Vec<Entity> = attachments::instances(world, from)
        .into_iter()
        .filter(|instance| keep(world, *instance))
        .collect();
    let mut copies = Vec::with_capacity(sources.len());
    for &source in &sources {
        let copy = duplicate_instance(world, source, to, None)?;
        if keep_slots {
            let slot = attachments::meta(world, source).and_then(|meta| meta.class_slot.clone());
            if let Some(mut meta) = world.get_mut::<ComponentMeta>(copy) {
                meta.class_slot = slot;
            }
        }
        copies.push(copy);
    }
    let new_id = |world: &World, source: Entity| {
        let index = sources.iter().position(|s| *s == source)?;
        attachments::meta(world, copies[index]).map(|meta| meta.id)
    };
    for (source, copy) in sources.iter().zip(&copies) {
        let Some(parent) = attachments::meta(world, *source).and_then(|meta| meta.parent) else {
            continue;
        };
        let parent_copy = attachments::instance_by_id(world, parent)
            .and_then(|parent_source| new_id(world, parent_source));
        if let Some(parent_copy) = parent_copy {
            attachments::set_parent(world, *copy, Some(parent_copy));
        }
    }
    Ok(copies)
}

// ── Typed snapshots (undo history, play mode) ─────────────────────────────

/// A typed copy of one attached instance: its metadata, enabled flag and a
/// clone of its value (or its unresolved payload). Taken and restored
/// without encoding or decoding anything.
pub struct InstanceSnapshot {
    pub meta: ComponentMeta,
    pub enabled: bool,
    pub value: InstanceValue,
}

/// What an [`InstanceSnapshot`] holds for the instance's class.
pub enum InstanceValue {
    /// A clone of the live typed value.
    Value(Box<dyn Any + Send + Sync>),
    /// The payload of an unresolved instance, as kept.
    Unresolved(UnresolvedComponent),
}

impl InstanceValue {
    fn clone_for(&self, class_name: &str) -> Option<Self> {
        Some(match self {
            Self::Value(value) => {
                let registration = crate::find(class_name)?;
                Self::Value((registration.clone_value)(value.as_ref())?)
            }
            Self::Unresolved(unresolved) => Self::Unresolved(unresolved.clone()),
        })
    }
}

impl Clone for InstanceSnapshot {
    /// Clones the value through its class's registration. A value whose
    /// class cannot clone it (not possible for a value taken by
    /// [`snapshot_instance`]) is kept as an unresolved instance that says so.
    fn clone(&self) -> Self {
        let value = self
            .value
            .clone_for(&self.meta.class_name)
            .unwrap_or_else(|| {
                InstanceValue::Unresolved(UnresolvedComponent {
                    data: Value::Null,
                    reason: format!("`{}` value could not be cloned", self.meta.class_name),
                })
            });
        Self {
            meta: self.meta.clone(),
            enabled: self.enabled,
            value,
        }
    }
}

impl std::fmt::Debug for InstanceSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InstanceSnapshot")
            .field("meta", &self.meta)
            .field("enabled", &self.enabled)
            .field(
                "value",
                &match &self.value {
                    InstanceValue::Value(_) => "typed value",
                    InstanceValue::Unresolved(_) => "unresolved payload",
                },
            )
            .finish()
    }
}

/// A typed snapshot of `instance`, or `None` if it is not an attached
/// instance (or its registered value is missing).
pub fn snapshot_instance(world: &World, instance: Entity) -> Option<InstanceSnapshot> {
    let meta = attachments::meta(world, instance)?.clone();
    let value = match world.get::<UnresolvedComponent>(instance) {
        Some(unresolved) => InstanceValue::Unresolved(unresolved.clone()),
        None => InstanceValue::Value(clone_instance_value(world, instance)?),
    };
    Some(InstanceSnapshot {
        enabled: attachments::is_enabled(world, instance),
        meta,
        value,
    })
}

/// Typed snapshots of all of `owner`'s instances, in order.
pub fn snapshot_instances(world: &World, owner: Entity) -> Vec<InstanceSnapshot> {
    attachments::instances(world, owner)
        .into_iter()
        .filter_map(|instance| snapshot_instance(world, instance))
        .collect()
}

/// Make `owner`'s instances equal `snapshots`, in place: an instance whose
/// id is in the snapshot keeps its entity and gets the snapshot's value (a
/// clone, through the class's insert, so every write hook runs), enabled
/// flag, slot and parent; instances not in the snapshot are detached;
/// missing ones are attached with their snapshot id; the list takes the
/// snapshot's order. Nothing is decoded. Returns the instance entities in
/// snapshot order.
///
/// An instance id held by another object is refused before anything is
/// written ([`InstanceError::DuplicateId`]).
pub fn restore_instances(
    world: &mut World,
    owner: Entity,
    snapshots: &[InstanceSnapshot],
) -> Result<Vec<Entity>, AttachError> {
    if !world.is_alive(owner) {
        return Err(InstanceError::DeadOwner(owner).into());
    }
    for snapshot in snapshots {
        let elsewhere = attachments::instance_by_id(world, snapshot.meta.id)
            .is_some_and(|entity| attachments::owner_of(world, entity) != Some(owner));
        if elsewhere {
            return Err(InstanceError::DuplicateId(snapshot.meta.id).into());
        }
    }

    // Detach what the snapshot does not hold, or holds as another class.
    for instance in attachments::instances(world, owner) {
        let keep = attachments::meta(world, instance).is_some_and(|meta| {
            snapshots
                .iter()
                .any(|s| s.meta.id == meta.id && s.meta.class_name == meta.class_name)
        });
        if !keep {
            attachments::detach(world, instance);
        }
    }

    let mut restored = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        let class = snapshot.meta.class_name.as_str();
        let Some(value) = snapshot.value.clone_for(class) else {
            return Err(AttachError::UnknownClass(class.to_string()));
        };
        let instance = match attachments::instance_by_id(world, snapshot.meta.id) {
            Some(instance) => {
                set_instance_value(world, instance, value)?;
                attachments::set_enabled(world, instance, snapshot.enabled);
                if let Some(mut meta) = world.get::<ComponentMeta>(instance).cloned() {
                    if meta.class_slot != snapshot.meta.class_slot {
                        meta.class_slot = snapshot.meta.class_slot.clone();
                        world.insert(instance, meta);
                    }
                }
                instance
            }
            None => {
                let spec = NewInstance {
                    id: snapshot.meta.id,
                    class_name: class.to_string(),
                    enabled: snapshot.enabled,
                    parent: None,
                    class_slot: snapshot.meta.class_slot.clone(),
                    index: None,
                };
                match value {
                    InstanceValue::Value(value) => {
                        attach_component(world, owner, spec, ComponentPayload::Value(value))?
                    }
                    InstanceValue::Unresolved(unresolved) => {
                        attach_unresolved(world, owner, spec, unresolved.data, unresolved.reason)?
                    }
                }
            }
        };
        restored.push(instance);
    }

    if attachments::instances(world, owner) != restored {
        world.insert(owner, ComponentAttachments(restored.clone()));
    }
    // Parents last: every instance they may name is attached now.
    for (snapshot, &instance) in snapshots.iter().zip(&restored) {
        let current = attachments::meta(world, instance).and_then(|meta| meta.parent);
        if current != snapshot.meta.parent {
            attachments::set_parent(world, instance, snapshot.meta.parent);
        }
    }
    Ok(restored)
}

/// Replace the attached `instance`'s value: a typed value of its class
/// (written through the class's insert, so every write hook runs; an
/// unresolved instance becomes live) or an unresolved payload (the typed
/// value, if any, is removed). Nothing is decoded; a value of another class
/// is refused before anything is written.
pub fn set_instance_value(
    world: &mut World,
    instance: Entity,
    value: InstanceValue,
) -> Result<(), AttachError> {
    let Some(class) = attachments::meta(world, instance).map(|meta| meta.class_name.clone()) else {
        return Err(AttachError::Instance(InstanceError::DeadOwner(instance)));
    };
    match value {
        InstanceValue::Value(value) => {
            insert_world_component_value(&class, world, instance, value)?;
            if world.get::<UnresolvedComponent>(instance).is_some() {
                world.remove::<UnresolvedComponent>(instance);
            }
        }
        InstanceValue::Unresolved(unresolved) => {
            if let Some(registration) = crate::find(&class) {
                (registration.remove)(world, instance);
            }
            if world.get::<UnresolvedComponent>(instance) != Some(&unresolved) {
                world.insert(instance, unresolved);
            }
        }
    }
    Ok(())
}

// ── Records (file and tool boundaries) ───────────────────────────

/// The record of `instance`: its class, id, enabled flag and data -- the
/// live value encoded once here, or the unresolved payload as kept -- with
/// slot metadata. `parent_index` is the position of its parent in the
/// record list being built, which only the caller knows.
pub fn instance_record(
    world: &World,
    instance: Entity,
    parent_index: Option<usize>,
) -> Option<ComponentRecord> {
    let data = match world.get::<UnresolvedComponent>(instance) {
        Some(unresolved) => unresolved.data.clone(),
        None => instance_engine_class(world, instance)?.to_json().ok()?,
    };
    record_with_metadata(world, instance, data, parent_index)
}

/// [`instance_record`] without the class's data: only the record metadata
/// keys, and no encoding. For callers that need the structure of an
/// object's component list (class names, order, enabled flags, parents).
pub fn instance_metadata_record(
    world: &World,
    instance: Entity,
    parent_index: Option<usize>,
) -> Option<ComponentRecord> {
    record_with_metadata(world, instance, Value::Object(Map::new()), parent_index)
}

fn record_with_metadata(
    world: &World,
    instance: Entity,
    mut data: Value,
    parent_index: Option<usize>,
) -> Option<ComponentRecord> {
    let meta = attachments::meta(world, instance)?;
    if let Some(map) = data.as_object_mut() {
        map.insert(INSTANCE_ID_KEY.into(), Value::String(meta.id.to_string()));
        if let Some(slot) = &meta.class_slot {
            map.insert(SLOT_ID_KEY.into(), Value::String(slot.slot_id.clone()));
            if let Some(local) = &slot.local_transform {
                map.insert(
                    TRANSFORM_KEY.into(),
                    serde_json::json!({
                        "position": local.position,
                        "rotation": local.rotation,
                        "scale": local.scale,
                    }),
                );
            }
        }
        if let Some(parent) = parent_index {
            map.insert(PARENT_INDEX_KEY.into(), serde_json::json!(parent));
        }
    }
    Some(ComponentRecord {
        class_name: meta.class_name.clone(),
        enabled: attachments::is_enabled(world, instance),
        data,
    })
}

/// The records of all of `owner`'s instances, in order.
pub fn component_records(world: &World, owner: Entity) -> Vec<ComponentRecord> {
    records_with(world, owner, instance_record)
}

/// [`component_records`] without the classes' data (see
/// [`instance_metadata_record`]).
pub fn component_metadata_records(world: &World, owner: Entity) -> Vec<ComponentRecord> {
    records_with(world, owner, instance_metadata_record)
}

fn records_with(
    world: &World,
    owner: Entity,
    record: fn(&World, Entity, Option<usize>) -> Option<ComponentRecord>,
) -> Vec<ComponentRecord> {
    let list = attachments::instances(world, owner);
    let ids: Vec<Option<ComponentInstanceId>> = list
        .iter()
        .map(|i| attachments::meta(world, *i).map(|m| m.id))
        .collect();
    list.iter()
        .filter_map(|instance| {
            let parent = attachments::meta(world, *instance)?.parent;
            let parent_index = parent.and_then(|p| ids.iter().position(|id| *id == Some(p)));
            record(world, *instance, parent_index)
        })
        .collect()
}

/// Split a record's data into the class's own data and its metadata.
struct SplitRecord {
    body: Value,
    id: Option<ComponentInstanceId>,
    class_slot: Option<ClassSlot>,
    parent_index: Option<usize>,
}

fn split_record(data: &Value) -> SplitRecord {
    let Some(map) = data.as_object() else {
        return SplitRecord {
            body: data.clone(),
            id: None,
            class_slot: None,
            parent_index: None,
        };
    };
    let mut body = Map::new();
    let mut slot_id = None;
    let mut local_transform = None;
    let mut parent_index = None;
    let mut id = None;
    for (key, value) in map {
        match key.as_str() {
            INSTANCE_ID_KEY => id = value.as_str().and_then(|text| text.parse().ok()),
            SLOT_ID_KEY => slot_id = value.as_str().map(str::to_string),
            TRANSFORM_KEY => local_transform = Some(transform_of(value)),
            PARENT_INDEX_KEY => parent_index = value.as_u64().map(|i| i as usize),
            _ if key.starts_with("__") => {
                tracing::warn!("dropping unknown component record metadata `{key}`");
            }
            _ => {
                body.insert(key.clone(), value.clone());
            }
        }
    }
    SplitRecord {
        body: Value::Object(body),
        id,
        class_slot: slot_id.map(|slot_id| ClassSlot {
            slot_id,
            local_transform,
        }),
        parent_index,
    }
}

fn transform_of(value: &Value) -> Transform {
    let vec3 = |key: &str, default: [f32; 3]| {
        value
            .get(key)
            .and_then(|v| serde_json::from_value::<[f32; 3]>(v.clone()).ok())
            .unwrap_or(default)
    };
    Transform {
        position: vec3("position", [0.0; 3]),
        rotation: vec3("rotation", [0.0; 3]),
        scale: vec3("scale", [1.0; 3]),
    }
}

fn spec_of(record: &ComponentRecord, split: &SplitRecord, index: Option<usize>) -> NewInstance {
    NewInstance {
        id: split.id.unwrap_or_default(),
        class_name: record.class_name.clone(),
        enabled: record.enabled,
        parent: None,
        class_slot: split.class_slot.clone(),
        index,
    }
}

/// Attach one record as a live instance: the registered class decodes its
/// data. Refused (nothing written) for an unregistered class or data that
/// does not decode. The record's parent index is ignored here; see
/// [`attach_records`].
pub fn attach_record(
    world: &mut World,
    owner: Entity,
    record: &ComponentRecord,
    index: Option<usize>,
) -> Result<Entity, AttachError> {
    let split = split_record(&record.data);
    let spec = spec_of(record, &split, index);
    attach_component(world, owner, spec, ComponentPayload::Json(split.body))
}

/// Attach one record, keeping a payload this build cannot use as an
/// explicit [`UnresolvedComponent`] instead of refusing it -- for loaders,
/// which must not lose data. `Err` only for a structural problem (dead
/// owner, duplicate id).
pub fn attach_record_or_unresolved(
    world: &mut World,
    owner: Entity,
    record: &ComponentRecord,
    index: Option<usize>,
) -> Result<Entity, InstanceError> {
    match attach_record(world, owner, record, index) {
        Ok(instance) => Ok(instance),
        Err(AttachError::Instance(error)) => Err(error),
        Err(error) => {
            let split = split_record(&record.data);
            let spec = spec_of(record, &split, index);
            attach_unresolved(world, owner, spec, split.body, error.to_string())
        }
    }
}

/// Attach `records` to `owner` in order (lossless: see
/// [`attach_record_or_unresolved`]), then restore their parent links from
/// the records' parent indices. Returns the instance entities.
pub fn attach_records(
    world: &mut World,
    owner: Entity,
    records: &[ComponentRecord],
) -> Result<Vec<Entity>, InstanceError> {
    let mut attached = Vec::with_capacity(records.len());
    for record in records {
        attached.push(attach_record_or_unresolved(world, owner, record, None)?);
    }
    for (record, instance) in records.iter().zip(&attached) {
        let Some(parent_index) = split_record(&record.data).parent_index else {
            continue;
        };
        let Some(parent) = attached.get(parent_index).copied() else {
            continue;
        };
        if let Some(parent_id) = attachments::meta(world, parent).map(|meta| meta.id) {
            attachments::set_parent(world, *instance, Some(parent_id));
        }
    }
    Ok(attached)
}

/// Replace all of `owner`'s instances with `records` (history restore,
/// class rebuild). Ids in the records are kept.
pub fn replace_records(
    world: &mut World,
    owner: Entity,
    records: &[ComponentRecord],
) -> Result<Vec<Entity>, InstanceError> {
    attachments::detach_all(world, owner);
    attach_records(world, owner, records)
}

/// `owner`'s instance holding stable id `id`.
pub fn instance_of(world: &World, owner: Entity, id: ComponentInstanceId) -> Option<Entity> {
    attachments::instances(world, owner)
        .into_iter()
        .find(|instance| {
            attachments::meta(world, *instance).is_some_and(|m: &ComponentMeta| m.id == id)
        })
}

/// The entity holding `class_name`'s value for an address that is either
/// that entity itself (a component instance, or any entity a caller put the
/// value on directly) or an owner object. An owner resolves to its
/// `ordinal`-th instance of the class, in list order (enabled or not);
/// `None` if there is no such instance.
pub fn resolve_instance(
    world: &World,
    entity: Entity,
    class_name: &str,
    ordinal: u32,
) -> Option<Entity> {
    if let Some(meta) = attachments::meta(world, entity) {
        return (meta.class_name == class_name && ordinal == 0).then_some(entity);
    }
    if ordinal == 0
        && crate::component_id_for_class(class_name)
            .is_some_and(|id| world.has_component(entity, id))
    {
        return Some(entity);
    }
    attachments::instances(world, entity)
        .into_iter()
        .filter(|instance| {
            attachments::meta(world, *instance).is_some_and(|m| m.class_name == class_name)
        })
        .nth(ordinal as usize)
}
