//! Class instances in a SceneDB `World`.
//!
//! An instance is a root object carrying a [`ClassInstance`] component plus
//! the prefab components the [`plan`](crate::plan) put on it, and one
//! generated child object per component that needs its own entity. Every
//! component instance created from a slot records that slot as typed
//! provenance ([`pulsar_scene_model::ClassSlot`] on its `ComponentMeta`),
//! which is how [`slot_map`] finds slot → component instance and how
//! generated children are told apart from objects the user parented under
//! an instance.
//!
//! Components are component-instance entities (Pulsar-Native#1035, D1).
//! [`component_records`] is their JSON boundary view, used where class
//! overrides are still diffed as JSON (Phase 3 converts that).

use std::collections::BTreeMap;

use glam::{EulerRot, Mat4, Quat, Vec3};
use pulsar_scene_model::attachments::{self, NewInstance};
use pulsar_scene_model::{
    ComponentInstance, ObjectType, SceneError, SceneWorldExt, SpawnObject, Transform, Visibility,
};
use pulsar_scenedb::{Entity, World};
use serde_json::Value;

use crate::component::ClassInstance;
use crate::overrides::{diff, prune_variable_overrides, split_meta};
use crate::plan::{
    is_removed_override, migrate_slot_keys, plan_instance, slot_default, LocalTransform,
    PlannedComponent,
};
use crate::registry::{ClassDefinition, ClassRegistry};
use crate::template::{template, ClassTemplate};
use crate::{child_stable_id, CLASS_INSTANCE, REMOVED_KEY, SLOT_ID_KEY};
use pulsar_world_registry::InstanceValue;

// ── Component records ─────────────────────────────────────────────────────

/// The component records of `entity`: each attached instance's class,
/// enabled flag and data (its live value, or a kept unresolved payload),
/// with slot metadata.
pub fn component_records(world: &World, entity: Entity) -> Vec<ComponentInstance> {
    pulsar_world_registry::component_records(world, entity)
}

/// Slot id recorded on a component record.
pub fn record_slot_id(record: &ComponentInstance) -> Option<&str> {
    record.data.get(SLOT_ID_KEY).and_then(Value::as_str)
}

/// The class slot a component instance was placed from.
pub fn instance_slot_id(world: &World, instance: Entity) -> Option<&str> {
    attachments::meta(world, instance)?
        .class_slot
        .as_ref()
        .map(|slot| slot.slot_id.as_str())
}

/// Append components to `entity`, each as its own component instance. A
/// record this build cannot decode is kept as an explicit unresolved
/// payload (and reported), never silently dropped or half-attached.
pub fn attach_components(world: &mut World, entity: Entity, components: Vec<ComponentInstance>) {
    for component in &components {
        match pulsar_world_registry::attach_record_or_unresolved(world, entity, component, None) {
            Ok(instance) => {
                if let Some(unresolved) =
                    world.get::<pulsar_scene_model::UnresolvedComponent>(instance)
                {
                    tracing::warn!(class = %component.class_name, "Class component kept unresolved: {}", unresolved.reason);
                }
            }
            Err(error) => {
                tracing::warn!(class = %component.class_name, "Class component could not be attached: {error}");
            }
        }
    }
}

// ── ClassInstance access ──────────────────────────────────────────────────

/// The `ClassInstance` component instance on `entity`.
pub fn class_instance_entity(world: &World, entity: Entity) -> Option<Entity> {
    pulsar_world_registry::instances::resolve_instance(world, entity, CLASS_INSTANCE, 0)
}

/// The `ClassInstance` on `entity`.
pub fn class_instance_of(world: &World, entity: Entity) -> Option<ClassInstance> {
    world
        .get::<ClassInstance>(class_instance_entity(world, entity)?)
        .cloned()
}

/// Whether `entity` is a class instance root.
pub fn is_class_root(world: &World, entity: Entity) -> bool {
    class_instance_entity(world, entity).is_some()
}

/// Write `instance` onto its root: a typed write of the root's
/// `ClassInstance`, attached first when the root has none.
pub fn store_class_instance(world: &mut World, root: Entity, instance: &ClassInstance) {
    match class_instance_entity(world, root) {
        Some(holder) => {
            world.insert(holder, instance.clone());
        }
        None => {
            let mut spec = NewInstance::new(CLASS_INSTANCE);
            spec.index = Some(0);
            if let Err(error) = pulsar_world_registry::attach_component(
                world,
                root,
                spec,
                pulsar_world_registry::ComponentPayload::Value(Box::new(instance.clone())),
            ) {
                tracing::warn!("ClassInstance could not be attached: {error}");
            }
        }
    }
}

// ── Generated children and slots ──────────────────────────────────────────

/// Whether `entity` is a child object generated for a class slot: all its
/// component records come from slots and its parent chain reaches a class
/// root through generated objects only.
pub fn is_generated_child(world: &World, entity: Entity) -> bool {
    let instances = attachments::instances(world, entity);
    if instances.is_empty()
        || !instances
            .iter()
            .all(|i| instance_slot_id(world, *i).is_some())
    {
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
    /// The object the slot's component is attached to.
    pub entity: Entity,
    /// Index into that object's component list.
    pub index: usize,
    /// The component-instance entity holding the slot's value.
    pub instance: Entity,
    pub class_name: String,
}

/// Slot id → entity/component for the instance rooted at `root`.
pub fn slot_map(world: &World, root: Entity) -> BTreeMap<String, SlotLocation> {
    let mut map = BTreeMap::new();
    for entity in std::iter::once(root).chain(generated_children(world, root)) {
        for (index, instance) in attachments::instances(world, entity)
            .into_iter()
            .enumerate()
        {
            let Some(meta) = attachments::meta(world, instance) else {
                continue;
            };
            if let Some(slot) = &meta.class_slot {
                map.entry(slot.slot_id.clone()).or_insert(SlotLocation {
                    entity,
                    index,
                    instance,
                    class_name: meta.class_name.clone(),
                });
            }
        }
    }
    map
}

/// A component slot of a placed instance, resolved to the
/// component-instance entity holding its real value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotHandle {
    pub slot_id: String,
    pub class_name: String,
    /// The component-instance entity.
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
                entity: loc.instance,
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
    attachments::instances(world, entity)
        .into_iter()
        .find_map(|instance| {
            attachments::meta(world, instance)?
                .class_slot
                .as_ref()?
                .local_transform
        })
        .map(|t| LocalTransform {
            position: t.position,
            rotation: t.rotation,
            scale: t.scale,
        })
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

/// Attach planned `component` to `owner`: its slot's template value with
/// the instance's overrides applied (nothing decoded), slot provenance
/// recorded. A slot with no typed default is kept as an unresolved payload
/// (and reported), never dropped.
fn attach_planned(
    world: &mut World,
    owner: Entity,
    template: &ClassTemplate,
    component: &PlannedComponent,
    local: Option<&LocalTransform>,
) {
    let Some(value) = template.slot_value(&component.slot_id, component.overrides.as_ref()) else {
        tracing::warn!(slot = %component.slot_id, "Planned class slot missing from its template");
        return;
    };
    let spec = NewInstance {
        enabled: component.enabled,
        class_slot: Some(pulsar_scene_model::ClassSlot {
            slot_id: component.slot_id.clone(),
            local_transform: local.map(|local| Transform {
                position: local.position,
                rotation: local.rotation,
                scale: local.scale,
            }),
        }),
        ..NewInstance::new(component.class_name.clone())
    };
    let attached = match value {
        InstanceValue::Value(value) => pulsar_world_registry::attach_component(
            world,
            owner,
            spec,
            pulsar_world_registry::ComponentPayload::Value(value),
        ),
        InstanceValue::Unresolved(unresolved) => {
            tracing::warn!(class = %component.class_name, "Class component kept unresolved: {}", unresolved.reason);
            pulsar_world_registry::attach_unresolved(
                world,
                owner,
                spec,
                unresolved.data,
                unresolved.reason,
            )
            .map_err(Into::into)
        }
    };
    if let Err(error) = attached {
        tracing::warn!(class = %component.class_name, "Class component could not be attached: {error}");
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
    for instance in attachments::instances(world, root) {
        if instance_slot_id(world, instance).is_some() {
            attachments::detach(world, instance);
        }
    }
}

/// Build the class components of the instance at `root` from `def`, the
/// current class definition, applying the root's `ClassInstance` overrides.
/// Anything a previous expansion created is replaced. Returns the placement
/// (a handle per slot, and the generated children). Values come from the
/// class's cached [`template`](crate::template::template).
pub fn expand_class_instance(
    world: &mut World,
    root: Entity,
    def: &ClassDefinition,
) -> ClassPlacement {
    expand_from_template(world, root, &template(def))
}

/// [`expand_class_instance`] from an already built template.
pub fn expand_from_template(
    world: &mut World,
    root: Entity,
    template: &ClassTemplate,
) -> ClassPlacement {
    let def = &template.def;
    clear_generated(world, root);
    let mut instance = class_instance_of(world, root).unwrap_or_default();
    // Overrides saved under slot ids the class has since replaced move to the
    // replacements, and are stored so the next save writes them that way.
    if migrate_slot_keys(def, &mut instance) {
        store_class_instance(world, root, &instance);
    }
    let plan = plan_instance(def, &instance);

    for component in &plan.root {
        attach_planned(world, root, template, component, None);
    }

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
        attach_planned(
            world,
            entity,
            template,
            &child.component,
            Some(&child.local),
        );
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
    instance: ClassInstance,
    spawn: SpawnObject,
) -> Result<ClassPlacement, SceneError> {
    instantiate_template(world, &template(def), instance, spawn)
}

/// [`instantiate_class`] from an already built template.
pub fn instantiate_template(
    world: &mut World,
    template: &ClassTemplate,
    mut instance: ClassInstance,
    spawn: SpawnObject,
) -> Result<ClassPlacement, SceneError> {
    let root = world.spawn_object(spawn)?;
    instance.class = template.def.id.clone();
    instance.class_name = template.def.name.clone();
    Ok(build_instance_root(world, template, instance, root))
}

/// [`instantiate_class`] onto `entity`, an already spawned but still bare
/// entity (no scene-object components yet): the id a script's
/// `world::spawn` handed out before the spawn was applied (#922).
pub fn instantiate_class_into(
    world: &mut World,
    def: &ClassDefinition,
    instance: ClassInstance,
    spawn: SpawnObject,
    entity: Entity,
) -> Result<ClassPlacement, SceneError> {
    instantiate_template_into(world, &template(def), instance, spawn, entity)
}

/// [`instantiate_class_into`] from an already built template.
pub fn instantiate_template_into(
    world: &mut World,
    template: &ClassTemplate,
    mut instance: ClassInstance,
    spawn: SpawnObject,
    entity: Entity,
) -> Result<ClassPlacement, SceneError> {
    world.spawn_object_into(entity, spawn)?;
    instance.class = template.def.id.clone();
    instance.class_name = template.def.name.clone();
    Ok(build_instance_root(world, template, instance, entity))
}

fn build_instance_root(
    world: &mut World,
    template: &ClassTemplate,
    instance: ClassInstance,
    root: Entity,
) -> ClassPlacement {
    store_class_instance(world, root, &instance);
    expand_from_template(world, root, template)
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
    // Each class is read and its template built once for the whole call,
    // however many instances it has.
    let mut templates: std::collections::HashMap<
        std::path::PathBuf,
        std::sync::Arc<ClassTemplate>,
    > = std::collections::HashMap::new();
    for root in roots {
        let Some(instance) = class_instance_of(world, root) else {
            continue;
        };
        let id = world.stable_id_of(root).unwrap_or_default().to_string();
        let resolved = registry.resolve(&instance).and_then(|entry| {
            if let Some(template) = templates.get(&entry.dir) {
                return Some(std::sync::Arc::clone(template));
            }
            let def = registry.definition_for(&instance)?;
            let template = template(&def);
            templates.insert(entry.dir.clone(), std::sync::Arc::clone(&template));
            Some(template)
        });
        match resolved {
            Some(template) => {
                let def = &template.def;
                // Refresh the GUID when the instance was matched by name.
                if instance.class != def.id {
                    let mut fixed = instance.clone();
                    fixed.class = def.id.clone();
                    fixed.class_name = def.name.clone();
                    store_class_instance(world, root, &fixed);
                }
                let placement = expand_from_template(world, root, &template);
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
        let live = pulsar_world_registry::instance_record(world, location.instance, None)
            .map(|record| record.data)
            .unwrap_or(Value::Null);
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
