//! Class instances in a SceneDB `World`.
//!
//! An instance is a root object carrying a [`ClassInstance`] component plus
//! the prefab components the [`plan`](crate::plan) put on it, and one
//! generated child object per component that needs its own entity. Every
//! component created from a slot carries `__slot_id` metadata in its
//! component record, which is how [`slot_map`] finds slot → entity/component
//! and how generated children are told apart from objects the user parented
//! under an instance.
//!
//! Component records are the object's `ComponentAttachments` (the editor's
//! per-object component list); objects without one (runtime hydration) are
//! read from `RenderProps::component_instances`.
//!
//! The editor normalizes records after these calls with its own
//! `sync_registered_component_props_to_scene_db`; the functions here keep
//! records and typed World components consistent on their own so the
//! runtime and tests need nothing else.

use std::collections::BTreeMap;

use glam::{EulerRot, Mat4, Quat, Vec3};
use pulsar_scene_model::{
    ComponentAttachments, ComponentInstance, ObjectType, RenderProps, SceneError, SceneWorldExt,
    SpawnObject, Transform, Visibility,
};
use pulsar_scenedb::{Entity, World};
use serde_json::Value;

use crate::component::ClassInstance;
use crate::overrides::{diff, prune_variable_overrides, split_meta};
use crate::plan::{
    is_removed_override, plan_instance, slot_default, LocalTransform, PlannedComponent,
};
use crate::registry::{ClassDefinition, ClassRegistry};
use crate::{child_stable_id, CLASS_INSTANCE, REMOVED_KEY, SLOT_ID_KEY, TRANSFORM_KEY};

// ── Component records ─────────────────────────────────────────────────────

/// The component records of `entity`: its `ComponentAttachments`, or the
/// `RenderProps::component_instances` projection when it has none.
pub fn component_records(world: &World, entity: Entity) -> Vec<ComponentInstance> {
    if let Some(attachments) = world.get::<ComponentAttachments>(entity) {
        return attachments.0.clone();
    }
    world
        .get::<RenderProps>(entity)
        .and_then(|props| props.component_instances.as_ref())
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    Some(ComponentInstance {
                        class_name: item.get("class_name")?.as_str()?.to_string(),
                        enabled: item.get("enabled").and_then(Value::as_bool).unwrap_or(true),
                        data: item.get("data").cloned().unwrap_or(Value::Null),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn write_records(world: &mut World, entity: Entity, records: Vec<ComponentInstance>) {
    let projection = Value::Array(
        records
            .iter()
            .enumerate()
            .map(|(index, c)| {
                serde_json::json!({
                    "index": index, "class_name": c.class_name, "data": c.data, "enabled": c.enabled
                })
            })
            .collect(),
    );
    if let Some(mut props) = world.get_mut::<RenderProps>(entity) {
        props.component_instances = Some(projection);
    }
    world.insert(entity, ComponentAttachments(records));
}

/// Slot id recorded on a component record.
pub fn record_slot_id(record: &ComponentInstance) -> Option<&str> {
    record.data.get(SLOT_ID_KEY).and_then(Value::as_str)
}

/// The data of record `index` on `entity`, read live from the World when it
/// is the class's live-typed instance (first enabled record of a registered
/// class), metadata keys included.
pub fn live_record_data(
    world: &World,
    entity: Entity,
    records: &[ComponentInstance],
    index: usize,
) -> Value {
    let Some(record) = records.get(index) else {
        return Value::Null;
    };
    let is_live = record.enabled
        && pulsar_world_registry::component_id_for_class(&record.class_name).is_some()
        && records
            .iter()
            .position(|r| r.enabled && r.class_name == record.class_name)
            == Some(index);
    if is_live {
        if let Some(live) = pulsar_world_registry::get_world_component_as_engine_class(
            &record.class_name,
            world,
            entity,
        ) {
            if let Ok(mut json) = live.to_json() {
                let (meta, _) = split_meta(&record.data);
                if let Some(map) = json.as_object_mut() {
                    map.extend(meta);
                }
                return json;
            }
        }
    }
    record.data.clone()
}

/// Append components to `entity`: the first enabled record of each
/// registered class is hydrated into its typed World component.
pub fn attach_components(world: &mut World, entity: Entity, components: Vec<ComponentInstance>) {
    let mut records = component_records(world, entity);
    for component in components {
        let registered =
            pulsar_world_registry::component_id_for_class(&component.class_name).is_some();
        let first_enabled = !records
            .iter()
            .any(|r| r.enabled && r.class_name == component.class_name);
        if component.enabled && registered && first_enabled {
            if let Err(error) = pulsar_world_registry::hydrate_world_component_for_class(
                &component.class_name,
                world,
                entity,
                &component.data,
            ) {
                tracing::warn!(class = %component.class_name, "Class component failed to hydrate: {error}");
            }
        }
        records.push(component);
    }
    write_records(world, entity, records);
}

/// Re-hydrate / remove typed components so each registered class's typed
/// value matches the first enabled record (records whose data is metadata
/// only are the editor's marker for "the typed value is authoritative").
fn resync_typed(world: &mut World, entity: Entity, records: &[ComponentInstance]) {
    let classes: Vec<&'static str> =
        pulsar_world_registry::registered_world_component_classes().collect();
    for class_name in classes {
        match records
            .iter()
            .find(|r| r.enabled && r.class_name == class_name)
        {
            Some(record) => {
                let (_, body) = split_meta(&record.data);
                if body.as_object().is_some_and(|m| !m.is_empty()) {
                    let _ = pulsar_world_registry::hydrate_world_component_for_class(
                        class_name,
                        world,
                        entity,
                        &record.data,
                    );
                }
            }
            None => {
                pulsar_world_registry::remove_world_component_for_class(class_name, world, entity);
            }
        }
    }
}

// ── ClassInstance access ──────────────────────────────────────────────────

/// The `ClassInstance` on `entity`: the typed World value, or the record's
/// JSON when it is not live (disabled, or not hydrated).
pub fn class_instance_of(world: &World, entity: Entity) -> Option<ClassInstance> {
    if let Some(instance) = world.get::<ClassInstance>(entity) {
        return Some(instance.clone());
    }
    component_records(world, entity)
        .into_iter()
        .find(|r| r.class_name == CLASS_INSTANCE)
        .and_then(|r| ClassInstance::from_json(&r.data))
}

/// Whether `entity` is a class instance root.
pub fn is_class_root(world: &World, entity: Entity) -> bool {
    world.get::<ClassInstance>(entity).is_some()
        || component_records(world, entity)
            .iter()
            .any(|r| r.class_name == CLASS_INSTANCE)
}

/// Write `instance` back onto its root (typed value and record).
pub fn store_class_instance(world: &mut World, root: Entity, instance: &ClassInstance) {
    let value = instance.to_value();
    let mut records = component_records(world, root);
    let mut found = false;
    for record in &mut records {
        if record.class_name == CLASS_INSTANCE {
            found = true;
            let (_, body) = split_meta(&record.data);
            // Keep "typed value is authoritative" markers as they are.
            if !body.as_object().is_some_and(|m| m.is_empty()) {
                record.data = value.clone();
            }
            break;
        }
    }
    if !found {
        records.insert(
            0,
            ComponentInstance {
                class_name: CLASS_INSTANCE.into(),
                enabled: true,
                data: value.clone(),
            },
        );
    }
    let _ = pulsar_world_registry::hydrate_world_component_for_class(
        CLASS_INSTANCE,
        world,
        root,
        &value,
    );
    write_records(world, root, records);
}

// ── Generated children and slots ──────────────────────────────────────────

/// Whether `entity` is a child object generated for a class slot: all its
/// component records come from slots and its parent chain reaches a class
/// root through generated objects only.
pub fn is_generated_child(world: &World, entity: Entity) -> bool {
    let records = component_records(world, entity);
    if records.is_empty() || !records.iter().all(|r| record_slot_id(r).is_some()) {
        return false;
    }
    match world.parent_of(entity) {
        Some(parent) => is_class_root(world, parent) || is_generated_child(world, parent),
        None => false,
    }
}

/// Generated descendants of `root`, parents before children.
pub fn generated_children(world: &World, root: Entity) -> Vec<Entity> {
    let mut out = Vec::new();
    fn go(world: &World, parent: Entity, out: &mut Vec<Entity>) {
        for child in world.children_of(Some(parent)) {
            if is_generated_child(world, child) {
                out.push(child);
                go(world, child, out);
            }
        }
    }
    go(world, root, &mut out);
    out
}

/// Where a slot's component lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotLocation {
    pub entity: Entity,
    /// Index into that entity's component records.
    pub index: usize,
    pub class_name: String,
}

/// Slot id → entity/component for the instance rooted at `root`.
pub fn slot_map(world: &World, root: Entity) -> BTreeMap<String, SlotLocation> {
    let mut map = BTreeMap::new();
    for entity in std::iter::once(root).chain(generated_children(world, root)) {
        for (index, record) in component_records(world, entity).iter().enumerate() {
            if let Some(slot) = record_slot_id(record) {
                map.entry(slot.to_string()).or_insert(SlotLocation {
                    entity,
                    index,
                    class_name: record.class_name.clone(),
                });
            }
        }
    }
    map
}

/// A component slot of a placed instance, resolved to the entity holding
/// the instance's real component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotHandle {
    pub slot_id: String,
    pub class_name: String,
    pub entity: Entity,
}

/// The result of placing a class: its root and a handle per component slot.
///
/// Slot ids exist only on disk and in the class's compiled script; this is
/// where they are resolved, once, into handles. Consumers (script binding)
/// take the handles and never look slot ids up again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClassPlacement {
    pub root: Option<Entity>,
    pub slots: Vec<SlotHandle>,
    /// Generated child objects, parents before children.
    pub children: Vec<Entity>,
}

impl ClassPlacement {
    pub fn root(&self) -> Entity {
        self.root.expect("placement of a spawned root")
    }

    /// The handle for `slot_id`, if the instance has that slot.
    pub fn handle(&self, slot_id: &str) -> Option<&SlotHandle> {
        self.slots.iter().find(|h| h.slot_id == slot_id)
    }
}

/// The placement of the instance rooted at `root`, as the world holds it
/// now: one handle per class slot the instance has (removed slots have
/// none).
pub fn placement(world: &World, root: Entity) -> ClassPlacement {
    ClassPlacement {
        root: Some(root),
        slots: slot_map(world, root)
            .into_iter()
            .map(|(slot_id, loc)| SlotHandle {
                slot_id,
                class_name: loc.class_name,
                entity: loc.entity,
            })
            .collect(),
        children: generated_children(world, root),
    }
}

// ── Transforms ────────────────────────────────────────────────────────────

fn to_matrix(position: [f32; 3], rotation: [f32; 3], scale: [f32; 3]) -> Mat4 {
    let q = Quat::from_euler(
        EulerRot::YXZ,
        rotation[1].to_radians(),
        rotation[0].to_radians(),
        rotation[2].to_radians(),
    );
    Mat4::from_scale_rotation_translation(Vec3::from_array(scale), q, Vec3::from_array(position))
}

/// World transform of a component placed at `local` under `parent`.
pub fn compose(parent: &Transform, local: &LocalTransform) -> Transform {
    let m = to_matrix(parent.position, parent.rotation, parent.scale)
        * to_matrix(local.position, local.rotation, local.scale);
    let (scale, rotation, translation) = m.to_scale_rotation_translation();
    let (y, x, z) = rotation.to_euler(EulerRot::YXZ);
    Transform {
        position: translation.to_array(),
        rotation: [x.to_degrees(), y.to_degrees(), z.to_degrees()],
        scale: scale.to_array(),
    }
}

fn local_transform_of(world: &World, entity: Entity) -> LocalTransform {
    component_records(world, entity)
        .iter()
        .find_map(|r| r.data.get(TRANSFORM_KEY))
        .map(LocalTransform::from_value)
        .unwrap_or_default()
}

/// Re-place every generated child from its root's current transform
/// (object transforms are flat world-space values, so they don't follow
/// their parent on their own). Returns the entities moved.
pub fn relayout_generated_children(world: &mut World, root: Entity) -> Vec<Entity> {
    let children = generated_children(world, root);
    for &child in &children {
        let Some(parent) = world.parent_of(child) else {
            continue;
        };
        let parent_tf = world.get::<Transform>(parent).copied().unwrap_or_default();
        let local = local_transform_of(world, child);
        world.insert(child, compose(&parent_tf, &local));
    }
    children
}

// ── Instantiation ─────────────────────────────────────────────────────────

fn planned_record(component: PlannedComponent) -> ComponentInstance {
    ComponentInstance {
        class_name: component.class_name,
        enabled: component.enabled,
        data: component.data,
    }
}

/// Remove what a previous expansion created: slot records on the root and
/// generated child objects (despawned). The editor removes generated
/// children itself first (it also retires their GPU rows), so there are
/// none left here in that case.
pub fn clear_generated(world: &mut World, root: Entity) {
    for child in generated_children(world, root).into_iter().rev() {
        if world.is_alive(child) {
            world.despawn_tree(child);
        }
    }
    let mut records = component_records(world, root);
    let before = records.len();
    records.retain(|r| record_slot_id(r).is_none());
    if records.len() != before {
        resync_typed(world, root, &records);
        write_records(world, root, records);
    }
}

/// Build the class components of the instance at `root` from `def`, the
/// current class definition, applying the root's `ClassInstance` overrides.
/// Anything a previous expansion created is replaced. Returns the placement
/// (a handle per slot, and the generated children).
pub fn expand_class_instance(
    world: &mut World,
    root: Entity,
    def: &ClassDefinition,
) -> ClassPlacement {
    clear_generated(world, root);
    let instance = class_instance_of(world, root).unwrap_or_default();
    let plan = plan_instance(def, &instance);

    attach_components(
        world,
        root,
        plan.root.into_iter().map(planned_record).collect(),
    );

    let root_id = world.stable_id_of(root).unwrap_or_default().to_string();
    let mut spawned: Vec<(String, Entity)> = Vec::new();
    for child in plan.children {
        let parent = child
            .parent_slot
            .as_ref()
            .and_then(|slot| spawned.iter().find(|(s, _)| s == slot).map(|(_, e)| *e))
            .unwrap_or(root);
        let parent_tf = world.get::<Transform>(parent).copied().unwrap_or_default();
        let visible = world.get::<Visibility>(root).copied().unwrap_or_default();
        let slot_id = child.component.slot_id.clone();
        let wanted_id = child_stable_id(&root_id, &slot_id);
        let spec = SpawnObject {
            stable_id: (!root_id.is_empty() && world.entity_for(&wanted_id).is_none())
                .then_some(wanted_id),
            name: child.component.class_name.clone(),
            parent: Some(parent),
            transform: compose(&parent_tf, &child.local),
            visibility: visible,
            object_type: ObjectType::Empty,
        };
        let entity = match world.spawn_object(spec) {
            Ok(entity) => entity,
            Err(error) => {
                tracing::warn!(slot = %slot_id, "Could not spawn class child: {error}");
                continue;
            }
        };
        attach_components(world, entity, vec![planned_record(child.component)]);
        spawned.push((slot_id, entity));
    }
    placement(world, root)
}

/// Spawn a new instance of `def` as `spawn` describes: the root object with
/// its `ClassInstance` (carrying `instance`'s overrides) and every prefab
/// component. Returns the placement: the root and a handle per slot.
pub fn instantiate_class(
    world: &mut World,
    def: &ClassDefinition,
    mut instance: ClassInstance,
    spawn: SpawnObject,
) -> Result<ClassPlacement, SceneError> {
    let root = world.spawn_object(spawn)?;
    instance.class = def.id.clone();
    instance.class_name = def.name.clone();
    Ok(build_instance_root(world, def, instance, root))
}

/// [`instantiate_class`] onto `entity`, an already spawned but still bare
/// entity (no scene-object components yet): the id a script's
/// `world::spawn` handed out before the spawn was applied (#922).
pub fn instantiate_class_into(
    world: &mut World,
    def: &ClassDefinition,
    mut instance: ClassInstance,
    spawn: SpawnObject,
    entity: Entity,
) -> Result<ClassPlacement, SceneError> {
    world.spawn_object_into(entity, spawn)?;
    instance.class = def.id.clone();
    instance.class_name = def.name.clone();
    Ok(build_instance_root(world, def, instance, entity))
}

fn build_instance_root(
    world: &mut World,
    def: &ClassDefinition,
    instance: ClassInstance,
    root: Entity,
) -> ClassPlacement {
    attach_components(
        world,
        root,
        vec![ComponentInstance {
            class_name: CLASS_INSTANCE.into(),
            enabled: true,
            data: instance.to_value(),
        }],
    );
    expand_class_instance(world, root, def)
}

/// Expand every class instance root in `world` from `registry`. Instances
/// whose class cannot be resolved keep their `ClassInstance` (and its
/// overrides) untouched and are reported by stable id.
pub fn expand_all(world: &mut World, registry: &ClassRegistry) -> ExpandReport {
    let roots: Vec<Entity> = world
        .query::<&pulsar_scene_model::StableId>()
        .map(|(entity, _)| entity)
        .collect();
    expand_roots(world, registry, &roots)
}

/// [`expand_all`] restricted to `candidates` (entities that are not class
/// roots are skipped).
pub fn expand_roots(
    world: &mut World,
    registry: &ClassRegistry,
    candidates: &[Entity],
) -> ExpandReport {
    let roots: Vec<Entity> = candidates
        .iter()
        .copied()
        .filter(|&entity| world.is_alive(entity) && is_class_root(world, entity))
        .collect();
    let mut report = ExpandReport::default();
    for root in roots {
        let Some(instance) = class_instance_of(world, root) else {
            continue;
        };
        let id = world.stable_id_of(root).unwrap_or_default().to_string();
        match registry.definition_for(&instance) {
            Some(def) => {
                // Refresh the GUID when the instance was matched by name.
                if instance.class != def.id {
                    let mut fixed = instance.clone();
                    fixed.class = def.id.clone();
                    fixed.class_name = def.name.clone();
                    store_class_instance(world, root, &fixed);
                }
                let placement = expand_class_instance(world, root, &def);
                report.expanded.push((id, placement));
            }
            None => {
                tracing::warn!(
                    object = %id,
                    class = %instance.class,
                    class_name = %instance.class_name,
                    "Class of placed instance not found; keeping it unresolved"
                );
                report.unresolved.push(id);
            }
        }
    }
    report
}

/// What [`expand_all`] did.
#[derive(Debug, Default)]
pub struct ExpandReport {
    /// Root stable id → its placement.
    pub expanded: Vec<(String, ClassPlacement)>,
    /// Roots whose class could not be resolved.
    pub unresolved: Vec<String>,
}

// ── Overrides from the live instance ──────────────────────────────────────

/// The instance's `ClassInstance` with overrides recomputed from its live
/// components: per slot, the diff of the live data against the class
/// default; variable overrides pruned of values equal to their default.
/// Slots the class no longer has keep their stored overrides; a class slot
/// with no component anymore is recorded as removed.
pub fn collect_overrides(world: &World, root: Entity, def: &ClassDefinition) -> ClassInstance {
    let mut instance = class_instance_of(world, root).unwrap_or_default();
    let slots = slot_map(world, root);
    for component in &def.prefab.components {
        let slot = &component.slot_id;
        let Some(location) = slots.get(slot) else {
            instance
                .component_overrides
                .insert(slot.clone(), serde_json::json!({ REMOVED_KEY: true }));
            continue;
        };
        let records = component_records(world, location.entity);
        let live = live_record_data(world, location.entity, &records, location.index);
        let (_, live_body) = split_meta(&live);
        let default = slot_default(def, slot).unwrap_or(Value::Null);
        match diff(&default, &live_body) {
            Some(d) => {
                instance.component_overrides.insert(slot.clone(), d);
            }
            None => {
                instance.component_overrides.remove(slot);
            }
        }
    }
    instance.variable_overrides = prune_variable_overrides(
        &def.prefab.variable_defaults(),
        &instance.variable_overrides,
    );
    instance
}

/// Whether a stored override marks the slot as removed.
pub fn slot_removed(instance: &ClassInstance, slot_id: &str) -> bool {
    instance
        .component_overrides
        .get(slot_id)
        .is_some_and(is_removed_override)
}
