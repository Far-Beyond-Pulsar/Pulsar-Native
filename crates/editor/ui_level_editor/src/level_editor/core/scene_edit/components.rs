//! Component instances on scene objects: attach / remove / enable / reorder /
//! parent, live property edits, and the world subscriptions the properties
//! panel uses.
//!
//! Live registered components are typed values in the world and are the source
//! of truth for their fields. [`ComponentAttachments`] records which instances an
//! object has, their order and enabled state, plus the JSON of dormant or
//! unregistered ones. Every function takes the `World` to work on.

use std::any::Any;
use std::collections::{HashMap, HashSet};

use engine_backend::scene::attachments as attach;
use engine_backend::scene::{ComponentAttachments, RenderProps, SceneWorldExt};
use pulsar_reflection::{apply_scene_props_for_class, registered_scene_props_classes};
use pulsar_scenedb::{Entity, World};
use serde_json::Value;

use super::changes::{record_property_change, record_structural_change};
use super::{
    attachment_data, is_scenedb_authority_class, overlay_live_data, remap_component_parents,
    ComponentInstance, ObjectId,
};

// ── Reads ──────────────────────────────────────────────────────────────────

/// Metadata-only view of an object's attached components: class names, order,
/// enabled flags and stored JSON, with NO live-world overlay.
///
/// [`get_components`] serializes every world-registered component to overlay
/// fresh values — right for save-to-disk, but waste for callers that only need
/// structure or that read live values themselves (the property cards batch-read
/// straight from the world).
pub fn get_components_metadata(world: &World, object_id: &str) -> Vec<ComponentInstance> {
    world
        .entity_for(object_id)
        .map(|entity| attach::get_components(world, entity))
        .unwrap_or_default()
}

/// Class names attached to `object_id`, in order. Cheaper than
/// [`get_components`]: no JSON clone, no serialization.
pub fn get_component_class_names(world: &World, object_id: &str) -> Vec<String> {
    get_components_metadata(world, object_id)
        .into_iter()
        .map(|c| c.class_name)
        .collect()
}

/// Cheap component count for `object_id`.
pub fn component_count(world: &World, object_id: &str) -> usize {
    get_components_metadata(world, object_id).len()
}

/// Every component instance attached to `object_id`, with `data` resolved
/// *live* off the world for any class that has a live value there
/// (Pulsar-Native#561): the stored JSON is trusted only for which components are
/// attached, their order and `enabled` flag. This is the one choke point both
/// the properties panel and save-to-disk go through.
pub fn get_components(world: &World, object_id: &str) -> Vec<ComponentInstance> {
    let Some(entity) = world.entity_for(object_id) else {
        return Vec::new();
    };
    let mut components = attach::get_components(world, entity);
    // Overlay ONLY onto each class's one live-typed instance (Pulsar-Native#519):
    // the world holds a single typed value per `(entity, ComponentId)` -- the
    // first enabled instance -- so stamping it onto EVERY instance of the class
    // used to clobber the other duplicates' own stored field values on every
    // read. A duplicate's `data` is its own blob; if it becomes the live-typed
    // one later, re-hydration adopts exactly that blob.
    let mut live_index_of_class: HashMap<String, usize> = HashMap::new();
    for (idx, component) in components.iter().enumerate() {
        if !component.enabled {
            continue;
        }
        if pulsar_world_registry::component_id_for_class(&component.class_name).is_some() {
            live_index_of_class
                .entry(component.class_name.clone())
                .or_insert(idx);
        }
    }
    for (idx, component) in components.iter_mut().enumerate() {
        if live_index_of_class.get(component.class_name.as_str()) != Some(&idx) {
            continue;
        }
        if let Some(live) = pulsar_world_registry::get_world_component_as_engine_class(
            component.class_name.as_str(),
            world,
            entity,
        ) {
            match live.to_json() {
                Ok(json) => component.data = overlay_live_data(&component.data, json),
                Err(error) => tracing::warn!(
                    "[GET_COMPONENTS] '{}' on '{object_id}' has a live world value but \
                     failed to serialize it, keeping the last-known-good stored copy: {error}",
                    component.class_name
                ),
            }
        }
    }
    components
}

/// Which instance of `class_name` on `object_id` is the **live-typed** one --
/// the single instance whose value actually lives in the world.
///
/// The world stores one value per `(entity, ComponentId)`, so of N instances of
/// the same class exactly ONE can be live-typed: the first ENABLED one. Every
/// other instance exists only as its own JSON blob. `None` when the class isn't
/// world-registered, or no enabled instance is attached.
///
/// This is Pulsar-Native#519's identity anchor: the properties panel uses it to
/// decide, per card, whether values come from the world (live card, subscribable)
/// or from that card's own JSON.
pub fn live_typed_component_index(
    world: &World,
    object_id: &str,
    class_name: &str,
) -> Option<usize> {
    pulsar_world_registry::component_id_for_class(class_name)?;
    get_components_metadata(world, object_id)
        .iter()
        .enumerate()
        .find(|(_, c)| c.class_name == class_name && c.enabled)
        .map(|(idx, _)| idx)
}

/// Read a single property straight off the **live world-resident component**,
/// correctly handling `#[sub_props]` nesting -- no JSON involved. `None` when the
/// class isn't world-registered, the object has no entity, or it isn't hydrated.
pub fn read_live_component_property(
    world: &World,
    object_id: &str,
    class_name: &str,
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
    let entity = world.entity_for(object_id)?;
    let instance =
        pulsar_world_registry::get_world_component_as_engine_class(class_name, world, entity)?;
    Some((getter)(instance))
}

/// Batch-read every property of a component with one entity lookup. Takes a
/// pre-built property metadata slice so the caller's cached metadata is reused.
pub fn read_component_properties_batch(
    world: &World,
    object_id: &str,
    class_name: &str,
    properties: &[pulsar_reflection::PropertyMetadata],
) -> Option<Vec<Box<dyn Any>>> {
    let entity = world.entity_for(object_id)?;
    let instance =
        pulsar_world_registry::get_world_component_as_engine_class(class_name, world, entity)?;
    Some(
        properties
            .iter()
            .map(|prop| (prop.getter)(instance))
            .collect(),
    )
}

/// Run a closure with the live world component reference. `None` if the class
/// isn't world-registered, the entity doesn't exist, or it isn't hydrated.
pub fn with_world_component<T>(
    world: &World,
    object_id: &str,
    class_name: &str,
    f: impl FnOnce(&dyn pulsar_reflection::EngineClass) -> T,
) -> Option<T> {
    let entity = world.entity_for(object_id)?;
    let instance =
        pulsar_world_registry::get_world_component_as_engine_class(class_name, world, entity)?;
    Some(f(instance))
}

// ── World subscriptions (Pulsar-Native#575, SceneDB#47) ────────────────────

/// Arm a world change subscription for `(object_id, class_name)`'s live
/// component -- the properties panel's subscribe-once-per-card replacement for
/// poll-every-render. `None` when the class has no world-registered component id,
/// the object has no live entity, or it is dead: "nothing to subscribe to".
pub fn subscribe_component(
    world: &mut World,
    object_id: &str,
    class_name: &str,
) -> Option<pulsar_scenedb::SubscriptionId> {
    let cid = pulsar_world_registry::component_id_for_class(class_name)?;
    let entity = world.entity_for(object_id)?;
    world.subscribe_id(entity, cid)
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

// ── Attach / remove / enable / reorder ─────────────────────────────────────

pub fn add_component(world: &mut World, object_id: &str, class_name: String, data: Value) {
    profiling::profile_scope!("scene_edit::add_component");
    attach_component_instance(
        world,
        object_id,
        ComponentInstance {
            class_name,
            enabled: true,
            data,
        },
        true,
    );
}

/// Add a fully specified component instance.
pub fn add_component_instance(world: &mut World, object_id: &str, component: ComponentInstance) {
    attach_component_instance(world, object_id, component, true);
}

/// Attach a component. For registered classes the first enabled instance is
/// hydrated into the world and the attachment record stores only attachment
/// state; disabled/failed entries retain JSON for re-enable compatibility.
pub(super) fn attach_component_instance(
    world: &mut World,
    object_id: &str,
    mut component: ComponentInstance,
    record_change: bool,
) {
    profiling::profile_scope!("scene_edit::attach_component_instance");
    let class_name = component.class_name.clone();
    if component.enabled
        && is_scenedb_authority_class(&class_name)
        && !get_components_metadata(world, object_id)
            .iter()
            .any(|existing| existing.enabled && existing.class_name == class_name)
        && hydrate_canonical_component(world, object_id, &class_name, &component.data)
    {
        component.data = attachment_data(&component.data);
    }
    if let Some(entity) = world.entity_for(object_id) {
        attach::add_component_instance(world, entity, component);
    }
    sync_registered_component_props_to_scene_db(world, object_id);
    if record_change {
        record_structural_change(object_id, &class_name);
    }
}

fn replace_components(world: &mut World, object_id: &str, components: Vec<ComponentInstance>) {
    if let Some(entity) = world.entity_for(object_id) {
        attach::replace_components(world, entity, components);
    }
}

/// Remove all attachment records of `object_id` (the objects are being despawned
/// or replaced).
pub(super) fn clear_components(world: &mut World, object_id: &str) {
    replace_components(world, object_id, Vec::new());
}

pub fn remove_component(world: &mut World, object_id: &str, component_index: usize) {
    profiling::profile_scope!("scene_edit::remove_component");
    // Preserve the old representative before changing instance order.
    let mut components = get_components(world, object_id);
    if component_index >= components.len() {
        return;
    }
    let class_name = components.remove(component_index).class_name;
    remap_component_parents(&mut components, |parent| {
        if parent == component_index {
            None
        } else {
            Some(if parent > component_index {
                parent - 1
            } else {
                parent
            })
        }
    });
    replace_components(world, object_id, components);
    sync_registered_component_props_to_scene_db(world, object_id);
    record_structural_change(object_id, &class_name);
}

/// Enable or disable a component by index.
pub fn set_component_enabled(
    world: &mut World,
    object_id: &str,
    component_index: usize,
    enabled: bool,
) -> bool {
    profiling::profile_scope!("scene_edit::set_component_enabled");
    let class_name = get_components_metadata(world, object_id)
        .get(component_index)
        .map(|c| c.class_name.clone());
    let mut components = get_components(world, object_id);
    let Some(component) = components.get_mut(component_index) else {
        return false;
    };
    if component.enabled == enabled {
        return true;
    }
    if is_scenedb_authority_class(&component.class_name) && component.enabled {
        if let Some(live) = get_components(world, object_id).get(component_index) {
            component.data = live.data.clone();
        }
    }
    component.enabled = enabled;
    replace_components(world, object_id, components);
    sync_registered_component_props_to_scene_db(world, object_id);
    if let Some(name) = class_name {
        record_structural_change(object_id, &name);
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
    let mut components = get_components(world, object_id);
    if component_index >= components.len() {
        return None;
    }

    let insert_index = component_index.saturating_add(1);
    let component = components.get(component_index)?.clone();
    let class_name = component.class_name.clone();
    components.insert(insert_index, component);
    remap_component_parents(&mut components, |parent| {
        Some(if parent >= insert_index {
            parent + 1
        } else {
            parent
        })
    });
    replace_components(world, object_id, components);
    sync_registered_component_props_to_scene_db(world, object_id);
    record_structural_change(object_id, &class_name);
    Some(insert_index)
}

pub fn reorder_component(world: &mut World, object_id: &str, from_index: usize, to_index: usize) {
    let mut components = get_components(world, object_id);
    if from_index >= components.len() || to_index >= components.len() || from_index == to_index {
        return;
    }

    let component = components.remove(from_index);
    let class_name = component.class_name.clone();
    components.insert(to_index, component);
    remap_component_parents(&mut components, |parent| {
        Some(if parent == from_index {
            to_index
        } else if from_index < to_index && parent > from_index && parent <= to_index {
            parent - 1
        } else if to_index < from_index && parent >= to_index && parent < from_index {
            parent + 1
        } else {
            parent
        })
    });
    replace_components(world, object_id, components);
    sync_registered_component_props_to_scene_db(world, object_id);
    record_structural_change(object_id, &class_name);
}

/// Whether component `potential_descendant` sits below `potential_ancestor` in
/// the component tree.
fn is_component_descendant(
    components: &[ComponentInstance],
    potential_descendant: usize,
    potential_ancestor: usize,
) -> bool {
    let mut current = potential_descendant;
    let mut visited = HashSet::new();
    loop {
        if !visited.insert(current) {
            return true;
        }
        if current == potential_ancestor {
            return true;
        }
        let parent = components[current]
            .data
            .get("__parent_index")
            .and_then(|v| v.as_u64())
            .map(|v| v as usize);

        match parent {
            Some(parent_idx) if parent_idx < components.len() => current = parent_idx,
            _ => return false, // Reached root or invalid parent
        }
    }
}

/// Set the parent of a component (for hierarchical organization).
pub fn set_component_parent(
    world: &mut World,
    object_id: &str,
    component_index: usize,
    parent_index: Option<usize>,
) {
    let mut components = get_components(world, object_id);
    if component_index >= components.len() {
        return;
    }

    // Prevent cycles: a component cannot be a parent of itself or its descendants.
    if let Some(parent_idx) = parent_index {
        if parent_idx == component_index || parent_idx >= components.len() {
            return;
        }
        if is_component_descendant(&components, parent_idx, component_index) {
            return;
        }
    }

    let component = &mut components[component_index];
    let mut data = component.data.as_object().cloned().unwrap_or_default();
    if let Some(parent_idx) = parent_index {
        data.insert("__parent_index".to_string(), serde_json::json!(parent_idx));
    } else {
        data.remove("__parent_index");
    }
    component.data = Value::Object(data);
    replace_components(world, object_id, components);
    sync_registered_component_props_to_scene_db(world, object_id);
}

// ── Edits ──────────────────────────────────────────────────────────────────

/// Hydrate one canonical component directly into the entity world -- an explicit
/// edit into the authoritative typed component. The caller decides whether the
/// attachment record retains its input JSON as a dormant compatibility value.
pub(super) fn hydrate_canonical_component(
    world: &mut World,
    object_id: &str,
    class_name: &str,
    data: &Value,
) -> bool {
    let Some(entity) = world.entity_for(object_id) else {
        return false;
    };
    match pulsar_world_registry::hydrate_world_component_for_class(class_name, world, entity, data)
    {
        Ok(hydrated) => hydrated,
        Err(error) => {
            tracing::error!("World hydration failed for {class_name} on '{object_id}': {error}");
            false
        }
    }
}

/// Update a single component's data by index. World-authoritative classes are
/// hydrated directly; legacy classes keep the attachment-JSON behavior.
pub fn update_component(
    world: &mut World,
    object_id: &str,
    component_index: usize,
    data: Value,
) {
    profiling::profile_scope!("scene_edit::update_component");
    let component = get_components_metadata(world, object_id)
        .get(component_index)
        .cloned();
    if let Some(component) = component.as_ref() {
        if component.enabled
            && is_scenedb_authority_class(&component.class_name)
            && live_typed_component_index(world, object_id, &component.class_name)
                == Some(component_index)
            && hydrate_canonical_component(world, object_id, &component.class_name, &data)
        {
            if let Some(entity) = world.entity_for(object_id) {
                attach::update_component(
                    world,
                    entity,
                    component_index,
                    attachment_data(&component.data),
                );
            }
            sync_registered_component_props_to_scene_db(world, object_id);
            record_structural_change(object_id, &component.class_name);
            return;
        }
    }

    let ok = world
        .entity_for(object_id)
        .is_some_and(|entity| attach::update_component(world, entity, component_index, data));
    if !ok {
        tracing::warn!(
            "[UPDATE_COMPONENT] update_component returned false for {object_id} idx={component_index}"
        );
    }
    sync_registered_component_props_to_scene_db(world, object_id);
}

/// Update a single property inside a reflection-based component by class name and
/// property name.
///
/// Legacy flat-JSON path -- only for classes never migrated to
/// `pulsar_world_registry`, where JSON in the attachment genuinely is the only
/// representation. **Do not call this for anything that supports
/// [`update_live_component_property`]**: it writes `new_value` at the top level of
/// the component's JSON, which is wrong for any `#[sub_props]`-nested field.
/// See Pulsar-Native#561.
pub fn update_component_property(
    world: &mut World,
    object_id: &str,
    class_name: &str,
    prop_name: &str,
    new_value: Value,
) {
    let components = get_components(world, object_id);
    if let Some((idx, comp)) = components
        .iter()
        .enumerate()
        .find(|(_, c)| c.class_name == class_name)
    {
        let mut data = comp.data.clone();
        if let Some(obj) = data.as_object_mut() {
            obj.insert(prop_name.to_string(), new_value);
        }
        update_component(world, object_id, idx, data);
        record_property_change(object_id, class_name, prop_name);
    }
}

/// Edit a single property on ONE specific component instance, correctly handling
/// `#[sub_props]` nesting (Pulsar-Native#561) and per-instance field values
/// (Pulsar-Native#519).
///
/// `component_index` addresses the exact instance in the object's component list:
/// an object can carry several instances of the same class, each with independent
/// field values.
///
/// - **The live-typed instance**: the setter runs straight against the
///   world-resident typed value -- no JSON on this path.
/// - **Every other instance**: the edit is applied through the real typed machinery
///   against a throwaway world seeded from that instance's JSON, then the full
///   result is written back to that same blob.
///
/// `Err(new_value)` -- handing the value straight back, since nothing was
/// written -- when the index/class pair doesn't match the object's component list,
/// or the class has no reflection metadata here (plugin-only classes; the command
/// layer's flat-JSON fallback covers those).
pub fn update_live_component_property(
    world: &mut World,
    object_id: &str,
    class_name: &str,
    component_index: usize,
    prop_name: &str,
    new_value: Box<dyn Any + Send>,
) -> Result<(), Box<dyn Any + Send>> {
    profiling::profile_scope!("scene_edit::update_live_component_property");
    // The index IS the identity: a stale or mismatched one must never land an
    // edit into some OTHER instance's storage.
    let components = get_components(world, object_id);
    let Some(target) = components.get(component_index) else {
        return Err(new_value);
    };
    if target.class_name != class_name {
        tracing::warn!(
            "[LIVE_PROPERTY_EDIT] index {component_index} holds '{}' not '{class_name}' -- edit refused",
            target.class_name
        );
        return Err(new_value);
    }

    let Some(prop_meta) = pulsar_reflection::REGISTRY
        .create_instance(class_name)
        .and_then(|instance| {
            instance
                .get_properties()
                .into_iter()
                .find(|p| p.name == prop_name)
        })
    else {
        tracing::warn!("[LIVE_PROPERTY_EDIT] no reflected property '{prop_name}' on '{class_name}'");
        return Err(new_value);
    };

    let is_live =
        live_typed_component_index(world, object_id, class_name) == Some(component_index);
    if !is_live {
        let mut scratch = World::new();
        let scratch_entity = scratch.spawn();
        let hydrated = pulsar_world_registry::hydrate_world_component_for_class(
            class_name,
            &mut scratch,
            scratch_entity,
            &target.data,
        );
        if hydrated.is_err() {
            // This instance's stored JSON doesn't deserialize for its own class --
            // refuse the edit rather than guess.
            return Err(new_value);
        }
        let Some(instance) = pulsar_world_registry::get_world_component_as_engine_class_mut(
            class_name,
            &mut scratch,
            scratch_entity,
        ) else {
            // Hydrate was a no-op: this class has no world bridge at all
            // (plugin-only). Hand the value back untouched.
            return Err(new_value);
        };
        (prop_meta.setter)(instance, new_value);
        let Ok(value_json) = instance.to_json() else {
            return Err(Box::new(()));
        };
        if let Some(entity) = world.entity_for(object_id) {
            attach::update_component(world, entity, component_index, value_json);
        }
        record_property_change(object_id, class_name, prop_name);
        return Ok(());
    }

    let setter = prop_meta.setter;
    let Some(entity) = world.entity_for(object_id) else {
        return Err(new_value);
    };
    let Some(instance) = pulsar_world_registry::get_world_component_as_engine_class_mut(
        class_name, world, entity,
    ) else {
        return Err(new_value);
    };
    (setter)(instance, new_value);
    // Capture the component's full current shape while `instance` is still borrowed.
    let persisted_json = instance.to_json().ok();
    record_property_change(object_id, class_name, prop_name);

    // A live migrated class is deliberately not written back: the world is its
    // authority, the attachment keeps only the order/enabled record.
    if is_live && is_scenedb_authority_class(class_name) {
        // The setter above only touched the one reflected field. Some classes
        // derive other, non-reflected state from their fields (StaticMeshComponent
        // reloading `vertices`/`indices` from `mesh_asset`; LightComponent's GPU
        // mirror) that a raw field write never re-derives -- generically re-run
        // whatever this class registered for exactly that (a no-op for classes
        // with nothing to refresh).
        pulsar_world_registry::refresh_world_component_gpu_mirror_for_class(
            class_name, world, entity,
        );
    } else if let Some(json) = persisted_json {
        attach::update_component(world, entity, component_index, json);
    }
    Ok(())
}

// ── Projection ─────────────────────────────────────────────────────────────

/// Bring the world in line with `object_id`'s attachment list: hydrate the first
/// enabled instance of each registered class, remove typed components with no
/// enabled instance, and refresh the JSON projection ([`RenderProps`]) from the
/// live values.
pub(super) fn sync_registered_component_props_to_scene_db(world: &mut World, object_id: &str) {
    let Some(entity) = world.entity_for(object_id) else {
        return;
    };
    let mut components = attach::get_components(world, entity);
    for class_name in pulsar_world_registry::registered_world_component_classes() {
        let component = components
            .iter_mut()
            .find(|component| component.enabled && component.class_name == class_name);
        if let Some(component) = component {
            // Null is the attachment marker for a live typed value. Only
            // explicit edits or newly promoted instances carry input JSON.
            if component.data != attachment_data(&component.data) {
                if let Err(error) = pulsar_world_registry::hydrate_world_component_for_class(
                    class_name,
                    world,
                    entity,
                    &component.data,
                ) {
                    tracing::error!(
                        "World hydration failed for {class_name} on '{object_id}': {error}"
                    );
                    continue;
                }
            }
            if pulsar_world_registry::get_world_component_as_engine_class(
                class_name, world, entity,
            )
            .is_some()
            {
                component.data = attachment_data(&component.data);
            }
        } else {
            pulsar_world_registry::remove_world_component_for_class(class_name, world, entity);
        }
    }
    world.insert(entity, ComponentAttachments(components.clone()));
    // Legacy props are a disposable serialization projection, never the input to
    // a typed component during an unrelated object edit.
    let mut projected_classes = HashSet::new();
    for component in &mut components {
        if component.enabled && projected_classes.insert(component.class_name.clone()) {
            if let Some(live) = pulsar_world_registry::get_world_component_as_engine_class(
                &component.class_name,
                world,
                entity,
            ) {
                if let Ok(data) = live.to_json() {
                    component.data = overlay_live_data(&component.data, data);
                }
            }
        }
    }
    if let Some(mut render_props) = world.get_mut::<RenderProps>(entity) {
        for class_name in registered_scene_props_classes() {
            let data = components
                .iter()
                .find(|c| c.class_name == class_name && c.enabled)
                .map(|c| &c.data);
            apply_scene_props_for_class(class_name, &mut render_props.props, data);
        }
        render_props.component_instances = Some(Value::Array(
            components
                .iter()
                .enumerate()
                .filter(|(_, component)| component.enabled)
                .map(|(index, component)| {
                    serde_json::json!({
                        "index": index, "class_name": component.class_name, "data": component.data
                    })
                })
                .collect(),
        ));
    }
}

/// Fold each enabled component's scene props into `props`.
pub(super) fn merge_component_props(
    world: &World,
    object_id: &str,
    props: &mut HashMap<String, Value>,
) {
    for component in get_components(world, object_id)
        .into_iter()
        .filter(|component| component.enabled)
    {
        if apply_scene_props_for_class(&component.class_name, props, Some(&component.data)) {
            continue;
        }
        if let Value::Object(map) = component.data {
            for (k, v) in map {
                props.insert(k, v);
            }
        }
    }
}

/// Entities of `object_id` and all its descendants' ids, for bulk cleanup.
pub(super) fn descendant_ids(world: &World, entity: Entity, out: &mut Vec<ObjectId>) {
    for child in world.children_of(Some(entity)) {
        if let Some(id) = world.stable_id_of(child) {
            out.push(id.to_string());
        }
        descendant_ids(world, child, out);
    }
}
