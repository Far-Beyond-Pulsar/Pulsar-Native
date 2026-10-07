//! Component instances on scene objects: attach / remove / enable / reorder /
//! parent, live property edits, and the world subscriptions the properties
//! panel uses.
//!
//! Every attached component is its own entity (Pulsar-Native#1035, D1),
//! linked to its object and holding its typed value whether enabled or not;
//! the object keeps the ordered list (see
//! [`engine_backend::scene::attachments`]). The editor addresses an instance
//! by its position in that list. JSON appears only at boundaries: the
//! records [`get_components`] returns (files, history, tools) and the data
//! an attach or [`update_component`] decodes once. A class this build does
//! not register is attached as an explicit unresolved instance that keeps
//! its payload. Every function takes the `World` to work on.

use std::any::Any;

use engine_backend::scene::attachments as attach;
use engine_backend::scene::SceneWorldExt;
use pulsar_scenedb::{Entity, World};
use serde_json::Value;

use super::changes::{record_property_change, record_structural_change};
use super::ComponentInstance;

// ── Addressing ─────────────────────────────────────────────────────────────

/// The instance entity at `index` in `object_id`'s component list.
pub fn instance_at(world: &World, object_id: &str, index: usize) -> Option<Entity> {
    let owner = world.entity_for(object_id)?;
    attach::instances(world, owner).get(index).copied()
}

/// The instance at `index` when it holds a live value of `class_name`.
fn live_instance(world: &World, object_id: &str, class_name: &str, index: usize) -> Option<Entity> {
    let instance = instance_at(world, object_id, index)?;
    let meta = attach::meta(world, instance)?;
    (meta.class_name == class_name && world.get::<attach::UnresolvedComponent>(instance).is_none())
        .then_some(instance)
}

// ── Reads ──────────────────────────────────────────────────────────────────

/// `object_id`'s component list as records without the classes' data: class
/// names, order, enabled flags and record metadata (`__parent_index`, ...).
/// No value is encoded.
pub fn get_components_metadata(world: &World, object_id: &str) -> Vec<ComponentInstance> {
    world
        .entity_for(object_id)
        .map(|owner| pulsar_world_registry::component_metadata_records(world, owner))
        .unwrap_or_default()
}

/// Class names attached to `object_id`, in order.
pub fn get_component_class_names(world: &World, object_id: &str) -> Vec<String> {
    let Some(owner) = world.entity_for(object_id) else {
        return Vec::new();
    };
    attach::instances(world, owner)
        .into_iter()
        .filter_map(|instance| attach::meta(world, instance).map(|meta| meta.class_name.clone()))
        .collect()
}

/// Number of components attached to `object_id`.
pub fn component_count(world: &World, object_id: &str) -> usize {
    world
        .entity_for(object_id)
        .map_or(0, |owner| attach::instances(world, owner).len())
}

/// Every component instance attached to `object_id` as a record, each live
/// value encoded once here -- for saving, history and tools.
pub fn get_components(world: &World, object_id: &str) -> Vec<ComponentInstance> {
    world
        .entity_for(object_id)
        .map(|owner| pulsar_world_registry::component_records(world, owner))
        .unwrap_or_default()
}

/// Whether the instance at `index` holds a live typed `class_name` value
/// (it is not an unresolved payload). Its card reads and subscribes to the
/// world.
pub fn is_live_instance(world: &World, object_id: &str, class_name: &str, index: usize) -> bool {
    live_instance(world, object_id, class_name, index).is_some()
}

/// The payload of the instance at `index` when it is unresolved (a class
/// this build does not register, or data that did not decode).
pub fn unresolved_payload(world: &World, object_id: &str, index: usize) -> Option<Value> {
    let instance = instance_at(world, object_id, index)?;
    Some(
        world
            .get::<attach::UnresolvedComponent>(instance)?
            .data
            .clone(),
    )
}

/// Read a single property straight off the live instance at `index`,
/// correctly handling `#[sub_props]` nesting -- no JSON involved.
pub fn read_live_component_property(
    world: &World,
    object_id: &str,
    class_name: &str,
    index: usize,
    prop_name: &str,
) -> Option<Box<dyn Any>> {
    let getter = pulsar_reflection::REGISTRY
        .create_instance(class_name)
        .and_then(|instance| {
            instance
                .get_properties()
                .into_iter()
                .find(|p| p.name == prop_name)
                .map(|p| p.getter)
        })?;
    with_world_component(world, object_id, class_name, index, |instance| {
        (getter)(instance)
    })
}

/// Batch-read every property of the instance at `index`. Takes a pre-built
/// property metadata slice so the caller's cached metadata is reused.
pub fn read_component_properties_batch(
    world: &World,
    object_id: &str,
    class_name: &str,
    index: usize,
    properties: &[pulsar_reflection::PropertyMetadata],
) -> Option<Vec<Box<dyn Any>>> {
    with_world_component(world, object_id, class_name, index, |instance| {
        properties
            .iter()
            .map(|prop| (prop.getter)(instance))
            .collect()
    })
}

/// Run a closure with the live value of the instance at `index`. `None`
/// unless that instance holds a live `class_name` value.
pub fn with_world_component<T>(
    world: &World,
    object_id: &str,
    class_name: &str,
    index: usize,
    f: impl FnOnce(&dyn pulsar_reflection::EngineClass) -> T,
) -> Option<T> {
    let instance = live_instance(world, object_id, class_name, index)?;
    let value =
        pulsar_world_registry::get_world_component_as_engine_class(class_name, world, instance)?;
    Some(f(value))
}

// ── World subscriptions (Pulsar-Native#575, SceneDB#47) ────────────────────

/// Arm a world change subscription for the live instance at `index` -- the
/// properties panel's subscribe-once-per-card replacement for
/// poll-every-render. `None` when there is no such live instance.
pub fn subscribe_component(
    world: &mut World,
    object_id: &str,
    class_name: &str,
    index: usize,
) -> Option<pulsar_scenedb::SubscriptionId> {
    let cid = pulsar_world_registry::component_id_for_class(class_name)?;
    let instance = live_instance(world, object_id, class_name, index)?;
    world.subscribe_id(instance, cid)
}

/// Disarm a previously armed subscription. Idempotent.
pub fn unsubscribe_component(world: &mut World, sub: pulsar_scenedb::SubscriptionId) {
    world.unsubscribe(sub);
}

/// Drain every pending component-change event (SceneDB#47's batched delivery).
/// Call once per frame.
///
/// SINGLE-DRAINER CONTRACT: this empties a shared queue -- exactly one consumer
/// per frame does the draining (the properties panel's component-card host).
pub fn take_world_component_events(world: &mut World) -> Vec<pulsar_scenedb::ComponentChangeEvent> {
    world.take_component_change_events()
}

/// Follow-ups to a successful property edit of the instance at `index`.
/// Choosing a voxel terrain's generator attaches that generator's settings
/// component when the object has none, so its settings appear at once;
/// other settings components are kept.
pub fn after_property_edit(
    world: &mut World,
    object_id: &str,
    class_name: &str,
    index: usize,
    prop_name: &str,
) {
    if class_name != "VoxelTerrainComponent" || prop_name != "generator" {
        return;
    }
    let Some(terrain) = instance_at(world, object_id, index)
        .and_then(|instance| world.get::<helio_component::VoxelTerrainComponent>(instance))
    else {
        return;
    };
    let Some(class) = helio_component::voxel_world::generator_settings_component(
        &terrain.generator.id,
        terrain.generator.version,
    ) else {
        return;
    };
    if get_component_class_names(world, object_id).contains(&class) {
        return;
    }
    let Some(owner) = world.entity_for(object_id) else {
        return;
    };
    match pulsar_world_registry::attach_component(
        world,
        owner,
        attach::NewInstance::new(class.clone()),
        pulsar_world_registry::ComponentPayload::Default,
    ) {
        Ok(_) => record_structural_change(object_id, &class),
        Err(error) => tracing::warn!("Could not attach {class} to '{object_id}': {error}"),
    }
}

// ── Attach / remove / enable / reorder ─────────────────────────────────────

/// Attach a new, enabled `class_name` instance holding `value` (a value of
/// that class), or the class default when `None`. Nothing is decoded.
/// Refused (nothing attached, an error logged) for an unregistered class or
/// a value of another class. Returns the new instance's index.
pub fn add_component_value(
    world: &mut World,
    object_id: &str,
    class_name: &str,
    value: Option<Box<dyn Any + Send + Sync>>,
) -> Option<usize> {
    profiling::profile_scope!("scene_edit::add_component_value");
    let owner = world.entity_for(object_id)?;
    let payload = match value {
        Some(value) => pulsar_world_registry::ComponentPayload::Value(value),
        None => pulsar_world_registry::ComponentPayload::Default,
    };
    match pulsar_world_registry::attach_component(
        world,
        owner,
        attach::NewInstance::new(class_name),
        payload,
    ) {
        Ok(instance) => {
            record_structural_change(object_id, class_name);
            attach::instances(world, owner)
                .iter()
                .position(|entity| *entity == instance)
        }
        Err(error) => {
            tracing::error!("Could not attach {class_name} to '{object_id}': {error}");
            None
        }
    }
}

/// Replace the value of the component at `component_index` with `value`,
/// in place (the instance keeps its entity and id). Nothing is decoded.
/// Returns whether it was written.
pub fn set_component_value(
    world: &mut World,
    object_id: &str,
    component_index: usize,
    value: pulsar_world_registry::InstanceValue,
) -> bool {
    profiling::profile_scope!("scene_edit::set_component_value");
    let Some(instance) = instance_at(world, object_id, component_index) else {
        return false;
    };
    let class_name = attach::meta(world, instance).map(|meta| meta.class_name.clone());
    match pulsar_world_registry::set_instance_value(world, instance, value) {
        Ok(()) => {
            if let Some(class_name) = class_name {
                record_structural_change(object_id, &class_name);
            }
            true
        }
        Err(error) => {
            tracing::warn!("Component {component_index} of '{object_id}' not updated: {error}");
            false
        }
    }
}

/// Attach a new, enabled `class_name` instance decoded from `data`. Refused
/// (nothing attached, an error logged) when the class is not registered or
/// the data does not decode. Returns the new instance's index.
pub fn add_component(
    world: &mut World,
    object_id: &str,
    class_name: String,
    data: Value,
) -> Option<usize> {
    profiling::profile_scope!("scene_edit::add_component");
    add_component_instance(
        world,
        object_id,
        ComponentInstance {
            class_name,
            enabled: true,
            data,
        },
    )
}

/// Attach a fully specified component record (see [`add_component`]).
pub fn add_component_instance(
    world: &mut World,
    object_id: &str,
    component: ComponentInstance,
) -> Option<usize> {
    let owner = world.entity_for(object_id)?;
    match pulsar_world_registry::attach_record(world, owner, &component, None) {
        Ok(instance) => {
            restore_parent(world, owner, instance, &component.data);
            record_structural_change(object_id, &component.class_name);
            attach::instances(world, owner)
                .iter()
                .position(|entity| *entity == instance)
        }
        Err(error) => {
            tracing::error!(
                "Could not attach {} to '{object_id}': {error}",
                component.class_name
            );
            None
        }
    }
}

/// Link `instance` to the instance at the record's `__parent_index`, if any.
fn restore_parent(world: &mut World, owner: Entity, instance: Entity, data: &Value) {
    let Some(parent_index) = data
        .get(pulsar_world_registry::instances::PARENT_INDEX_KEY)
        .and_then(Value::as_u64)
    else {
        return;
    };
    let parent = attach::instances(world, owner)
        .get(parent_index as usize)
        .and_then(|parent| attach::meta(world, *parent))
        .map(|meta| meta.id);
    if parent.is_some() {
        attach::set_parent(world, instance, parent);
    }
}

/// Replace `object_id`'s components with `components` -- records from a
/// file or a history snapshot, kept losslessly (a payload this build cannot
/// use stays attached as unresolved).
pub(super) fn replace_components(
    world: &mut World,
    object_id: &str,
    components: &[ComponentInstance],
) {
    let Some(owner) = world.entity_for(object_id) else {
        return;
    };
    if let Err(error) = pulsar_world_registry::replace_records(world, owner, components) {
        tracing::error!("Could not restore the components of '{object_id}': {error}");
    }
}

/// Detach every component of `object_id`.
pub(super) fn clear_components(world: &mut World, object_id: &str) {
    if let Some(owner) = world.entity_for(object_id) {
        attach::detach_all(world, owner);
    }
}

/// Detach the component at `component_index`. Returns whether there was one.
pub fn remove_component(world: &mut World, object_id: &str, component_index: usize) -> bool {
    profiling::profile_scope!("scene_edit::remove_component");
    let Some(instance) = instance_at(world, object_id, component_index) else {
        return false;
    };
    let class_name = attach::meta(world, instance).map(|meta| meta.class_name.clone());
    attach::detach(world, instance);
    if let Some(class_name) = class_name {
        record_structural_change(object_id, &class_name);
    }
    true
}

/// Enable or disable a component by index. Its value stays in place.
/// Returns whether there is a component at `component_index`.
pub fn set_component_enabled(
    world: &mut World,
    object_id: &str,
    component_index: usize,
    enabled: bool,
) -> bool {
    profiling::profile_scope!("scene_edit::set_component_enabled");
    let Some(instance) = instance_at(world, object_id, component_index) else {
        return false;
    };
    if attach::is_enabled(world, instance) == enabled {
        return true;
    }
    attach::set_enabled(world, instance, enabled);
    if let Some(meta) = attach::meta(world, instance) {
        let class_name = meta.class_name.clone();
        record_structural_change(object_id, &class_name);
    }
    true
}

/// Duplicate a component on the same object, inserting the copy directly after
/// the source.
pub fn duplicate_component(
    world: &mut World,
    object_id: &str,
    component_index: usize,
) -> Option<usize> {
    let owner = world.entity_for(object_id)?;
    let instance = instance_at(world, object_id, component_index)?;
    let insert_index = component_index + 1;
    match pulsar_world_registry::duplicate_instance(world, instance, owner, Some(insert_index)) {
        Ok(copy) => {
            if let Some(meta) = attach::meta(world, copy) {
                let class_name = meta.class_name.clone();
                record_structural_change(object_id, &class_name);
            }
            Some(insert_index)
        }
        Err(error) => {
            tracing::error!("Could not duplicate a component of '{object_id}': {error}");
            None
        }
    }
}

/// Move the component at `from_index` to `to_index`. Returns whether the
/// order changed.
pub fn reorder_component(
    world: &mut World,
    object_id: &str,
    from_index: usize,
    to_index: usize,
) -> bool {
    let Some(owner) = world.entity_for(object_id) else {
        return false;
    };
    let class_name = instance_at(world, object_id, from_index)
        .and_then(|instance| attach::meta(world, instance))
        .map(|meta| meta.class_name.clone());
    if from_index == to_index || !attach::move_instance(world, owner, from_index, to_index) {
        return false;
    }
    if let Some(class_name) = class_name {
        record_structural_change(object_id, &class_name);
    }
    true
}

/// Set the parent of a component (for hierarchical organization). Refused
/// when it would make a cycle. Returns whether the parent changed.
pub fn set_component_parent(
    world: &mut World,
    object_id: &str,
    component_index: usize,
    parent_index: Option<usize>,
) -> bool {
    let Some(instance) = instance_at(world, object_id, component_index) else {
        return false;
    };
    let parent = match parent_index {
        Some(index) => {
            let Some(parent) = instance_at(world, object_id, index)
                .and_then(|parent| attach::meta(world, parent))
                .map(|meta| meta.id)
            else {
                return false;
            };
            Some(parent)
        }
        None => None,
    };
    let current = attach::meta(world, instance).and_then(|meta| meta.parent);
    current != parent && attach::set_parent(world, instance, parent)
}

// ── Edits ──────────────────────────────────────────────────────────────────

/// Replace the value of the component at `component_index` with `data`,
/// decoded once. Nothing is written when it does not decode.
pub fn update_component(world: &mut World, object_id: &str, component_index: usize, data: Value) {
    profiling::profile_scope!("scene_edit::update_component");
    let Some(instance) = instance_at(world, object_id, component_index) else {
        return;
    };
    let class_name = attach::meta(world, instance).map(|meta| meta.class_name.clone());
    match pulsar_world_registry::set_instance_data(world, instance, &data) {
        Ok(()) => {
            if let Some(class_name) = class_name {
                record_structural_change(object_id, &class_name);
            }
        }
        Err(error) => tracing::warn!(
            "[UPDATE_COMPONENT] {object_id} idx={component_index} not updated: {error}"
        ),
    }
}

/// Set one top-level field of the data of the `class_name` instance at
/// `component_index`.
///
/// For classes with no reflected setter here (plugin-only classes, whose
/// value is an unresolved payload): a typed class is edited through
/// [`update_live_component_property`], which handles `#[sub_props]`
/// nesting. See Pulsar-Native#561.
pub fn update_component_property(
    world: &mut World,
    object_id: &str,
    class_name: &str,
    component_index: usize,
    prop_name: &str,
    new_value: Value,
) {
    let Some(record) = instance_at(world, object_id, component_index)
        .and_then(|instance| pulsar_world_registry::instance_record(world, instance, None))
        .filter(|record| record.class_name == class_name)
    else {
        return;
    };
    let mut data = record.data;
    if let Some(obj) = data.as_object_mut() {
        obj.insert(prop_name.to_string(), new_value);
    }
    update_component(world, object_id, component_index, data);
    record_property_change(object_id, class_name, prop_name);
}

/// Edit a single property on ONE specific component instance, correctly handling
/// `#[sub_props]` nesting (Pulsar-Native#561) and per-instance field values
/// (Pulsar-Native#519).
///
/// `component_index` addresses the exact instance in the object's component
/// list; every instance holds its own typed value, so the setter (and the
/// class's derived-field normalization) runs straight against it under one
/// SceneDB guard. GPU rows follow through SceneDB's own mirror dispatch.
///
/// `Err(new_value)` -- handing the value straight back, since nothing was
/// written -- when the index/class pair doesn't match the object's component
/// list, or the instance holds no live value (an unresolved payload; the
/// command layer's flat-JSON fallback covers those).
pub fn update_live_component_property(
    world: &mut World,
    object_id: &str,
    class_name: &str,
    component_index: usize,
    prop_name: &str,
    new_value: Box<dyn Any>,
) -> Result<(), Box<dyn Any>> {
    profiling::profile_scope!("scene_edit::update_live_component_property");
    // The index IS the identity: a stale or mismatched one must never land an
    // edit into some OTHER instance.
    let Some(instance) = live_instance(world, object_id, class_name, component_index) else {
        tracing::warn!(
            "[LIVE_PROPERTY_EDIT] index {component_index} of '{object_id}' holds no live \
             '{class_name}' -- edit refused"
        );
        return Err(new_value);
    };
    pulsar_world_registry::set_world_component_property(
        class_name, world, instance, prop_name, new_value,
    )?;
    record_property_change(object_id, class_name, prop_name);
    Ok(())
}
