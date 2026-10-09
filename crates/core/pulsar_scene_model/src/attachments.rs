//! Component instances as SceneDB entities (Pulsar-Native#1035, decision D1).
//!
//! Every component attached to a scene object is its own entity, carrying:
//!
//! - the component's live value -- the registered Rust type itself, typed,
//!   inserted through SceneDB's normal write path, whether or not the
//!   instance is enabled -- or, for a class this build does not know or data
//!   that failed to decode, an [`UnresolvedComponent`] that keeps the
//!   payload verbatim and is explicitly not a live component;
//! - [`ComponentOwner`]: the owning object and the enabled flag. It is a
//!   GPU-mirrored row keyed by the instance entity, so render passes can join
//!   a component row to its owner's transform and visibility rows on the GPU;
//! - [`ComponentMeta`]: the stable [`ComponentInstanceId`], class name,
//!   presentation parent and class-slot provenance.
//!
//! The owning object carries [`ComponentAttachments`], the ordered list of
//! its instance entities. Order is presentation only, never identity:
//! references hold a [`ComponentInstanceId`] (persistent) or the instance
//! entity (one live world).
//!
//! Several instances of one class on one object are simply several
//! entities. Despawning an object despawns its instances with it
//! ([`crate::SceneWorldExt::despawn_tree`]), so an instance never outlives its
//! owner.
//!
//! This module is structural and class-agnostic. Producing the typed value
//! for a class name (factory, boundary decode, clone) is the registry's job
//! (`pulsar_world_registry::instances`).

use std::fmt;

use pulsar_scenedb::{Component, ComponentId, Entity, World};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Stable identity of one attached component instance: opaque, 128-bit,
/// unique within a scene, preserved by save/load and history. Never derived
/// from an `Entity` or a list position.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ComponentInstanceId(pub u128);

impl ComponentInstanceId {
    /// A fresh, random id.
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().as_u128())
    }
}

impl Default for ComponentInstanceId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for ComponentInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ComponentInstanceId({self})")
    }
}

impl fmt::Display for ComponentInstanceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl std::str::FromStr for ComponentInstanceId {
    type Err = std::num::ParseIntError;
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        u128::from_str_radix(text, 16).map(Self)
    }
}

// A hex string, not a JSON number: 128-bit integers do not survive tools
// that read JSON numbers as doubles.
impl Serialize for ComponentInstanceId {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ComponentInstanceId {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// On a component-instance entity: the owning object and whether the
/// instance is enabled. A packed GPU row keyed by the instance entity, so a
/// GPU consumer can join any component row to its owner's rows and skip
/// disabled instances.
#[derive(Clone, Copy, Debug, PartialEq, Eq, pulsar_scenedb::SceneStore)]
#[gpu(layout = packed, buffer = "component_owners")]
#[repr(C)]
pub struct ComponentOwner {
    #[gpu]
    pub owner_index: u32,
    #[gpu]
    pub owner_generation: u32,
    /// 1 when enabled, 0 when disabled.
    #[gpu]
    pub enabled: u32,
}

impl ComponentOwner {
    pub fn new(owner: Entity, enabled: bool) -> Self {
        Self {
            owner_index: owner.index(),
            owner_generation: owner.generation(),
            enabled: u32::from(enabled),
        }
    }

    /// The owning object.
    pub fn entity(&self) -> Entity {
        Entity::from_bits(((self.owner_generation as u64) << 32) | self.owner_index as u64)
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled != 0
    }
}

/// On a component-instance entity: its identity and presentation metadata.
#[derive(Clone, Debug, PartialEq)]
pub struct ComponentMeta {
    pub id: ComponentInstanceId,
    /// The registered class name (the schema id for now).
    pub class_name: String,
    /// Presentation-only nesting under another instance of the same owner.
    pub parent: Option<ComponentInstanceId>,
    /// The class-prefab slot this instance was placed from, if any.
    pub class_slot: Option<ClassSlot>,
}

/// Class-prefab provenance of a placed component instance.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassSlot {
    /// The slot's stable id in the class definition.
    pub slot_id: String,
    /// The slot's local transform, for a slot placed on its own generated
    /// child object.
    pub local_transform: Option<crate::Transform>,
}

/// On a component-instance entity whose class is not registered in this
/// build, or whose stored data does not decode as its class: the payload,
/// kept verbatim so saving loses nothing. It is not a live component and has
/// no typed value; `reason` says why.
#[derive(Clone, Debug, PartialEq)]
pub struct UnresolvedComponent {
    pub data: serde_json::Value,
    pub reason: String,
}

/// On a scene object: its component-instance entities, in order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ComponentAttachments(pub Vec<Entity>);

/// Why a structural instance operation was refused.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum InstanceError {
    #[error("object {0:?} is not alive")]
    DeadOwner(Entity),
    #[error("component instance {0} already exists in this world")]
    DuplicateId(ComponentInstanceId),
    #[error("parent instance {0} is not attached to the same object")]
    ForeignParent(ComponentInstanceId),
}

/// The metadata a new instance is created with.
#[derive(Clone, Debug)]
pub struct NewInstance {
    pub id: ComponentInstanceId,
    pub class_name: String,
    pub enabled: bool,
    pub parent: Option<ComponentInstanceId>,
    pub class_slot: Option<ClassSlot>,
    /// Position in the owner's list; appended when `None` or past the end.
    pub index: Option<usize>,
}

impl NewInstance {
    pub fn new(class_name: impl Into<String>) -> Self {
        Self {
            id: ComponentInstanceId::new(),
            class_name: class_name.into(),
            enabled: true,
            parent: None,
            class_slot: None,
            index: None,
        }
    }
}

/// Spawn an empty instance entity for `owner` (metadata and owner link
/// only; the caller inserts the value next) and place it in the owner's
/// list. Validates everything before writing anything.
pub fn spawn_instance(
    world: &mut World,
    owner: Entity,
    spec: NewInstance,
) -> Result<Entity, InstanceError> {
    if !world.is_alive(owner) {
        return Err(InstanceError::DeadOwner(owner));
    }
    if instance_by_id(world, spec.id).is_some() {
        return Err(InstanceError::DuplicateId(spec.id));
    }
    if let Some(parent) = spec.parent {
        let attached = instance_by_id(world, parent)
            .is_some_and(|entity| owner_of(world, entity) == Some(owner));
        if !attached {
            return Err(InstanceError::ForeignParent(parent));
        }
    }
    let instance = world.spawn_bundle((
        ComponentOwner::new(owner, spec.enabled),
        ComponentMeta {
            id: spec.id,
            class_name: spec.class_name,
            parent: spec.parent,
            class_slot: spec.class_slot,
        },
    ));
    let mut list = world
        .get::<ComponentAttachments>(owner)
        .cloned()
        .unwrap_or_default();
    let index = spec.index.unwrap_or(list.0.len()).min(list.0.len());
    list.0.insert(index, instance);
    world.insert(owner, list);
    Ok(instance)
}

/// `owner`'s instance entities, in order.
pub fn instances(world: &World, owner: Entity) -> Vec<Entity> {
    world
        .get::<ComponentAttachments>(owner)
        .map(|list| list.0.clone())
        .unwrap_or_default()
}

/// The object `instance` is attached to.
pub fn owner_of(world: &World, instance: Entity) -> Option<Entity> {
    world
        .get::<ComponentOwner>(instance)
        .map(ComponentOwner::entity)
}

/// `instance`'s metadata.
pub fn meta(world: &World, instance: Entity) -> Option<&ComponentMeta> {
    world.get::<ComponentMeta>(instance)
}

/// Whether `instance` is attached and enabled.
pub fn is_enabled(world: &World, instance: Entity) -> bool {
    world
        .get::<ComponentOwner>(instance)
        .is_some_and(ComponentOwner::is_enabled)
}

/// The instance with stable id `id`, if attached in this world.
pub fn instance_by_id(world: &World, id: ComponentInstanceId) -> Option<Entity> {
    world
        .query::<&ComponentMeta>()
        .find(|(_, meta)| meta.id == id)
        .map(|(entity, _)| entity)
}

/// Enable or disable `instance`. The value stays typed and in place either
/// way; the flag lives in its [`ComponentOwner`] row. Returns whether
/// `instance` is an attached instance.
pub fn set_enabled(world: &mut World, instance: Entity, enabled: bool) -> bool {
    let Some(current) = world.get::<ComponentOwner>(instance).copied() else {
        return false;
    };
    if current.is_enabled() != enabled {
        world.insert(instance, ComponentOwner::new(current.entity(), enabled));
    }
    true
}

/// Set `instance`'s presentation parent. Refuses a parent on another object
/// and a cycle. Returns whether it was applied.
pub fn set_parent(
    world: &mut World,
    instance: Entity,
    parent: Option<ComponentInstanceId>,
) -> bool {
    let Some(owner) = owner_of(world, instance) else {
        return false;
    };
    if let Some(parent_id) = parent {
        let Some(parent_entity) = instance_by_id(world, parent_id) else {
            return false;
        };
        if owner_of(world, parent_entity) != Some(owner) {
            return false;
        }
        // Walk up from the new parent; reaching `instance` would be a cycle.
        let mut cursor = Some(parent_entity);
        while let Some(current) = cursor {
            if current == instance {
                return false;
            }
            cursor = meta(world, current)
                .and_then(|meta| meta.parent)
                .and_then(|id| instance_by_id(world, id));
        }
    }
    let Some(mut updated) = meta(world, instance).cloned() else {
        return false;
    };
    if updated.parent != parent {
        updated.parent = parent;
        world.insert(instance, updated);
    }
    true
}

/// Move the instance at `from` to `to` in `owner`'s list. Presentation only.
pub fn move_instance(world: &mut World, owner: Entity, from: usize, to: usize) -> bool {
    let mut list = world
        .get::<ComponentAttachments>(owner)
        .cloned()
        .unwrap_or_default();
    if from >= list.0.len() || to >= list.0.len() {
        return false;
    }
    if from != to {
        let instance = list.0.remove(from);
        list.0.insert(to, instance);
        world.insert(owner, list);
    }
    true
}

/// Detach and despawn `instance`. Instances nested under it lose their
/// parent link (nesting is presentation only). Returns whether it was
/// attached.
pub fn detach(world: &mut World, instance: Entity) -> bool {
    let Some(owner) = owner_of(world, instance) else {
        return false;
    };
    let id = meta(world, instance).map(|meta| meta.id);
    if let Some(mut list) = world.get::<ComponentAttachments>(owner).cloned() {
        list.0.retain(|entity| *entity != instance);
        world.insert(owner, list);
    }
    if let Some(id) = id {
        for sibling in instances(world, owner) {
            if meta(world, sibling).is_some_and(|meta| meta.parent == Some(id)) {
                set_parent(world, sibling, None);
            }
        }
    }
    world.despawn(instance)
}

/// Despawn every instance attached to `owner` (the object itself stays).
pub fn detach_all(world: &mut World, owner: Entity) {
    for instance in instances(world, owner) {
        world.despawn(instance);
    }
    if world.get::<ComponentAttachments>(owner).is_some() {
        world.insert(owner, ComponentAttachments::default());
    }
}

/// A component of `instance`'s owner object, e.g. its `Transform` or
/// `Visibility`: the join from a component instance to its object.
pub fn owner_component<T: Component>(world: &World, instance: Entity) -> Option<&T> {
    world.get::<T>(owner_of(world, instance)?)
}

/// The entity holding component `id` that a reference to `entity` names:
/// `entity` itself when it holds one (a component instance, or a value put
/// on the entity directly), else the first of `entity`'s instances, in
/// list order, that holds one.
pub fn holder_of(world: &World, entity: Entity, id: ComponentId) -> Option<Entity> {
    if world.has_component(entity, id) {
        return Some(entity);
    }
    instances(world, entity)
        .into_iter()
        .find(|instance| world.has_component(*instance, id))
}

/// The object `entity` belongs to: its owner when it is a component
/// instance, else `entity` itself.
pub fn object_of(world: &World, entity: Entity) -> Entity {
    owner_of(world, entity).unwrap_or(entity)
}

/// `owner`'s enabled instances that hold a `T`, in list order.
pub fn enabled_components_of<T: Component>(world: &World, owner: Entity) -> Vec<(Entity, &T)> {
    instances(world, owner)
        .into_iter()
        .filter(|instance| is_enabled(world, *instance))
        .filter_map(|instance| world.get::<T>(instance).map(|value| (instance, value)))
        .collect()
}

/// `owner`'s one enabled `T`: `Ok(None)` if it has none, an error naming the
/// count if it has several. The convenience for callers that need exactly
/// one; it never silently picks the first of several.
pub fn single_enabled_component_of<T: Component>(
    world: &World,
    owner: Entity,
) -> Result<Option<(Entity, &T)>, AmbiguousComponent> {
    let mut found = enabled_components_of::<T>(world, owner);
    match found.len() {
        0 => Ok(None),
        1 => Ok(found.pop()),
        count => Err(AmbiguousComponent {
            type_name: std::any::type_name::<T>(),
            count,
        }),
    }
}

/// More than one enabled instance matched a lookup that needs exactly one.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("{count} enabled `{type_name}` instances where exactly one was expected")]
pub struct AmbiguousComponent {
    pub type_name: &'static str,
    pub count: usize,
}

/// Every enabled `T` instance in the world whose owner is alive, with its
/// owner: `(instance, owner, value)`.
pub fn enabled_components<T: Component>(
    world: &World,
) -> impl Iterator<Item = (Entity, Entity, &T)> {
    world
        .query::<(&T, &ComponentOwner)>()
        .filter(|(_, (_, link))| link.is_enabled())
        .map(|(instance, (value, link))| (instance, link.entity(), value))
        .filter(move |(_, owner, _)| world.is_alive(*owner))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Health(u32);

    fn attach(world: &mut World, owner: Entity, value: u32, enabled: bool) -> Entity {
        let mut spec = NewInstance::new("Health");
        spec.enabled = enabled;
        let instance = spawn_instance(world, owner, spec).unwrap();
        world.insert(instance, Health(value));
        instance
    }

    #[test]
    fn several_instances_of_one_class_are_separate_entities() {
        let mut world = World::new();
        let owner = world.spawn();
        let a = attach(&mut world, owner, 1, true);
        let b = attach(&mut world, owner, 2, true);
        let disabled = attach(&mut world, owner, 3, false);

        assert_eq!(instances(&world, owner), vec![a, b, disabled]);
        assert_eq!(owner_of(&world, b), Some(owner));
        // A disabled instance keeps its typed value.
        assert_eq!(world.get::<Health>(disabled), Some(&Health(3)));
        let enabled: Vec<u32> = enabled_components_of::<Health>(&world, owner)
            .into_iter()
            .map(|(_, h)| h.0)
            .collect();
        assert_eq!(enabled, vec![1, 2]);
        assert!(matches!(
            single_enabled_component_of::<Health>(&world, owner),
            Err(AmbiguousComponent { count: 2, .. })
        ));

        let all: Vec<_> = enabled_components::<Health>(&world)
            .map(|(i, o, h)| (i, o, h.0))
            .collect();
        assert_eq!(all.len(), 2);
        assert!(all.iter().all(|(_, o, _)| *o == owner));
    }

    #[test]
    fn enable_reorder_parent_and_detach() {
        let mut world = World::new();
        let owner = world.spawn();
        let a = attach(&mut world, owner, 1, true);
        let b = attach(&mut world, owner, 2, true);
        let a_id = meta(&world, a).unwrap().id;

        assert!(set_enabled(&mut world, a, false));
        assert!(!is_enabled(&world, a));
        assert!(move_instance(&mut world, owner, 1, 0));
        assert_eq!(instances(&world, owner), vec![b, a]);

        assert!(set_parent(&mut world, b, Some(a_id)));
        let b_id = meta(&world, b).unwrap().id;
        assert!(!set_parent(&mut world, a, Some(b_id)), "a cycle is refused");

        assert!(detach(&mut world, a));
        assert!(!world.is_alive(a));
        assert_eq!(instances(&world, owner), vec![b]);
        assert_eq!(
            meta(&world, b).unwrap().parent,
            None,
            "the orphaned child loses its parent link"
        );
    }

    #[test]
    fn ids_are_unique_and_parents_must_share_the_owner() {
        let mut world = World::new();
        let owner = world.spawn();
        let other = world.spawn();
        let a = attach(&mut world, owner, 1, true);
        let a_id = meta(&world, a).unwrap().id;

        let mut duplicate = NewInstance::new("Health");
        duplicate.id = a_id;
        assert_eq!(
            spawn_instance(&mut world, owner, duplicate),
            Err(InstanceError::DuplicateId(a_id))
        );

        let mut foreign = NewInstance::new("Health");
        foreign.parent = Some(a_id);
        assert_eq!(
            spawn_instance(&mut world, other, foreign),
            Err(InstanceError::ForeignParent(a_id))
        );
        assert!(
            instances(&world, other).is_empty(),
            "a refused spawn writes nothing"
        );
    }

    #[test]
    fn instance_ids_round_trip_as_hex_strings() {
        let id = ComponentInstanceId(0x0123_4567_89ab_cdef_0011_2233_4455_6677);
        let json = serde_json::to_value(id).unwrap();
        assert_eq!(json, serde_json::json!("0123456789abcdef0011223344556677"));
        assert_eq!(
            serde_json::from_value::<ComponentInstanceId>(json).unwrap(),
            id
        );
    }
}
