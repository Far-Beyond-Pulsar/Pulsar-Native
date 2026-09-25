//! Placed class instances in the editor (Pulsar-Native#921).
//!
//! A placed class is a root object with a `ClassInstance` component, the
//! class's prefab components, and generated child objects for components
//! that need their own entity (see `pulsar_class::plan`). This module wraps
//! `pulsar_class::world` with the editor's component bookkeeping (component
//! records are normalized through
//! [`sync_registered_component_props_to_scene_db`]) and provides the data
//! side of the details panel: which values an instance overrides, and
//! reverting them to the class default.

use std::path::Path;

use engine_backend::scene::{SceneWorldExt, SpawnObject, Transform as WorldTransform};
use pulsar_class::world as class_world;
use pulsar_class::{ClassDefinition, ClassInstance, ClassRegistry};
use pulsar_scenedb::{Entity, World};
use serde_json::Value;

use super::components::sync_registered_component_props_to_scene_db;
use super::objects::remove_object;
use super::{ObjectId, ObjectType, Transform};

/// The current project's classes (scanned from `src/classes`). Empty when
/// no project is open.
pub fn project_registry() -> ClassRegistry {
    match engine_state::get_project_path()
        .map(std::path::PathBuf::from)
        .or_else(|| {
            fallback_project_root()
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
        }) {
        Some(root) => ClassRegistry::scan(&root),
        None => ClassRegistry::default(),
    }
}

/// Project root used by [`project_registry`] when no engine context has
/// one (command-line tools, tests).
fn fallback_project_root() -> &'static std::sync::Mutex<Option<std::path::PathBuf>> {
    static ROOT: std::sync::OnceLock<std::sync::Mutex<Option<std::path::PathBuf>>> =
        std::sync::OnceLock::new();
    ROOT.get_or_init(Default::default)
}

/// Set the project root [`project_registry`] falls back to when the engine
/// context has none.
pub fn set_fallback_project_root(root: Option<std::path::PathBuf>) {
    *fallback_project_root()
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = root;
}

/// The registry for a class directory: the project it lives in
/// (`<project>/src/classes/<Name>`), else just its parent directory.
pub fn registry_for_class_dir(class_dir: &Path) -> ClassRegistry {
    match class_dir.parent() {
        Some(classes) => ClassRegistry::scan_classes_dir(classes),
        None => ClassRegistry::default(),
    }
}

/// The registry to resolve a legacy `script_asset` path with: the classes
/// next to it when that directory exists here, else the current project's.
pub fn registry_for_script_asset(script_asset: &str) -> ClassRegistry {
    let path = Path::new(script_asset.trim().trim_end_matches(['/', '\\']));
    if path.is_dir() {
        let registry = registry_for_class_dir(path);
        if registry.resolve_script_asset(script_asset).is_some() {
            return registry;
        }
    }
    project_registry()
}

fn entity(world: &World, id: &str) -> Option<Entity> {
    world.entity_for(id)
}

fn id_of(world: &World, entity: Entity) -> Option<ObjectId> {
    world.stable_id_of(entity).map(str::to_string)
}

/// Normalize the component records of `root` and its generated children.
fn sync_instance(world: &mut World, root: Entity) -> Vec<ObjectId> {
    let mut ids = Vec::new();
    for e in std::iter::once(root).chain(class_world::generated_children(world, root)) {
        if let Some(id) = id_of(world, e) {
            sync_registered_component_props_to_scene_db(world, &id);
            ids.push(id);
        }
    }
    ids
}

// ── Placement ──────────────────────────────────────────────────────────────

/// Place a new instance of `def`. Returns the root id followed by the ids of
/// its generated children, or `None` when the root could not be spawned.
pub fn instantiate_class(
    world: &mut World,
    def: &ClassDefinition,
    instance: ClassInstance,
    name: Option<String>,
    transform: &Transform,
    parent: Option<&str>,
) -> Option<Vec<ObjectId>> {
    profiling::profile_scope!("scene_edit::instantiate_class");
    let parent_entity = match parent {
        Some(parent_id) => Some(entity(world, parent_id)?),
        None => None,
    };
    let spawn = SpawnObject {
        stable_id: None,
        name: name.unwrap_or_else(|| def.name.clone()),
        parent: parent_entity,
        transform: WorldTransform::from(transform),
        visibility: Default::default(),
        object_type: ObjectType::Blueprint,
    };
    let root = match class_world::instantiate_class(world, def, instance, spawn) {
        Ok(placement) => {
            remember_built_definition(def);
            placement.root()
        }
        Err(error) => {
            tracing::error!(class = %def.name, "Could not place class: {error}");
            return None;
        }
    };
    Some(sync_instance(world, root))
}

/// Place an instance of the class in `class_dir` (a content-browser or
/// viewport drop).
pub fn instantiate_class_dir(
    world: &mut World,
    class_dir: &Path,
    transform: &Transform,
    parent: Option<&str>,
) -> Result<Vec<ObjectId>, String> {
    let registry = registry_for_class_dir(class_dir);
    let entry = registry
        .resolve_script_asset(&class_dir.display().to_string())
        .ok_or_else(|| format!("Not a class directory: {}", class_dir.display()))?;
    let def = entry.load_definition()?;
    instantiate_class(
        world,
        &def,
        ClassInstance::default(),
        None,
        transform,
        parent,
    )
    .ok_or_else(|| "Could not place the class".to_string())
}

/// Duplicate the class instance at `id`: a fresh instance of the same class
/// with the source's current overrides, at the source's transform and
/// parent, plus copies of any components the user added to the root.
/// Returns the new root id, or `None` when the class cannot be resolved (the
/// caller then falls back to a plain copy, which still keeps the class link).
pub fn duplicate_instance(world: &mut World, id: &str) -> Option<ObjectId> {
    duplicate_instance_with(world, id, &project_registry())
}

/// [`duplicate_instance`] with an explicit class registry.
pub fn duplicate_instance_with(
    world: &mut World,
    id: &str,
    registry: &ClassRegistry,
) -> Option<ObjectId> {
    let source = super::objects::get_object(world, id)?;
    let instance = current_overrides(world, id, registry)?;
    let def = registry.definition_for(&instance)?;
    let extra: Vec<super::ComponentInstance> = super::components::get_components(world, id)
        .into_iter()
        .filter(|c| {
            c.class_name != pulsar_class::CLASS_INSTANCE
                && c.data.get(pulsar_class::SLOT_ID_KEY).is_none()
        })
        .collect();
    let ids = instantiate_class(
        world,
        &def,
        instance,
        Some(format!("{} (Copy)", source.name)),
        &source.transform,
        source.parent.as_deref(),
    )?;
    let root_id = ids.first()?.clone();
    for component in extra {
        super::components::add_component_instance(world, &root_id, component);
    }
    Some(root_id)
}

/// Rebuild the instance at `root_id` from the current class definition,
/// keeping its overrides: generated children are removed and recreated.
/// Returns the ids touched, or `None` when the class cannot be resolved.
pub fn rebuild_instance(
    world: &mut World,
    root_id: &str,
    registry: &ClassRegistry,
) -> Option<Vec<ObjectId>> {
    let root = entity(world, root_id)?;
    let instance = class_world::class_instance_of(world, root)?;
    let def = registry.definition_for(&instance)?;
    for child in class_world::generated_children(world, root)
        .into_iter()
        .rev()
    {
        if let Some(id) = id_of(world, child) {
            remove_object(world, &id);
        }
    }
    if instance.class != def.id {
        let mut fixed = instance;
        fixed.class = def.id.clone();
        fixed.class_name = def.name.clone();
        class_world::store_class_instance(world, root, &fixed);
    }
    class_world::expand_class_instance(world, root, &def);
    remember_built_definition(&def);
    Some(sync_instance(world, root))
}

/// Rebuild every class instance in the world (after a level load). Returns
/// the ids of instances whose class could not be found; they keep their
/// `ClassInstance` data untouched.
pub fn rebuild_all_instances(world: &mut World, registry: &ClassRegistry) -> Vec<ObjectId> {
    let roots: Vec<ObjectId> = world
        .query::<&engine_backend::scene::StableId>()
        .map(|(e, id)| (e, id.0.clone()))
        .filter(|(e, _)| class_world::is_class_root(world, *e))
        .map(|(_, id)| id)
        .collect();
    let mut unresolved = Vec::new();
    for id in roots {
        if rebuild_instance(world, &id, registry).is_none() {
            tracing::warn!(object = %id, "Placed class not found in this project; kept unresolved");
            unresolved.push(id);
        }
    }
    unresolved
}

/// Whether `id` is a generated child of a class instance (it is rebuilt from
/// the class and not saved on its own).
pub fn is_generated_child(world: &World, id: &str) -> bool {
    entity(world, id).is_some_and(|e| class_world::is_generated_child(world, e))
}

/// Whether `id` is a class instance root.
pub fn is_class_root(world: &World, id: &str) -> bool {
    entity(world, id).is_some_and(|e| class_world::is_class_root(world, e))
}

/// The `ClassInstance` of `id`, if it is a class root.
pub fn class_instance(world: &World, id: &str) -> Option<ClassInstance> {
    class_world::class_instance_of(world, entity(world, id)?)
}

/// Re-place the generated children of `root_id` after its transform
/// changed. Returns the ids moved.
pub fn relayout_children(world: &mut World, root_id: &str) -> Vec<ObjectId> {
    let Some(root) = entity(world, root_id) else {
        return Vec::new();
    };
    if !class_world::is_class_root(world, root) {
        return Vec::new();
    }
    class_world::relayout_generated_children(world, root)
        .into_iter()
        .filter_map(|e| id_of(world, e))
        .collect()
}

/// The instance's `ClassInstance` with overrides recomputed from its live
/// components (what saving writes). `None` when `id` is not a class root or
/// its class cannot be resolved.
pub fn current_overrides(
    world: &World,
    id: &str,
    registry: &ClassRegistry,
) -> Option<ClassInstance> {
    let root = entity(world, id)?;
    let instance = class_world::class_instance_of(world, root)?;
    let def = registry.definition_for(&instance)?;
    Some(class_world::collect_overrides(world, root, &def))
}

// ── Details panel data ─────────────────────────────────────────────────────

/// One script variable of a placed class.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassVariableView {
    pub name: String,
    /// How the details panel edits it.
    pub kind: pulsar_class::VariableKind,
    pub default: Value,
    /// The instance value (the override, or the default).
    pub value: Value,
    pub overridden: bool,
}

/// One overridden property of a prefab slot.
#[derive(Clone, Debug, PartialEq)]
pub struct OverriddenProperty {
    /// Dot-separated path into the component data (`intensity.intensity`).
    pub path: String,
    pub default: Value,
    pub value: Value,
}

/// One prefab component slot of a placed class.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassSlotView {
    pub slot_id: String,
    pub class_name: String,
    /// Object holding the component (the root or a generated child);
    /// `None` when the instance removed the slot.
    pub object_id: Option<ObjectId>,
    pub removed: bool,
    pub overridden: Vec<OverriddenProperty>,
}

/// What the details panel shows for a class instance.
#[derive(Clone, Debug, PartialEq)]
pub struct ClassInstanceView {
    pub class_name: String,
    pub class_id: String,
    /// `false` when the class is missing from the project.
    pub resolved: bool,
    pub variables: Vec<ClassVariableView>,
    pub slots: Vec<ClassSlotView>,
}

/// Build the details-panel view of the instance at `id`.
pub fn class_instance_view(
    world: &World,
    id: &str,
    registry: &ClassRegistry,
) -> Option<ClassInstanceView> {
    let root = entity(world, id)?;
    let stored = class_world::class_instance_of(world, root)?;
    let Some(def) = registry.definition_for(&stored) else {
        return Some(ClassInstanceView {
            class_name: stored.class_name.clone(),
            class_id: stored.class.to_string(),
            resolved: false,
            variables: stored
                .variable_overrides
                .iter()
                .map(|(name, value)| ClassVariableView {
                    name: name.clone(),
                    kind: pulsar_class::VariableKind::Other("unresolved".into()),
                    default: Value::Null,
                    value: value.clone(),
                    overridden: true,
                })
                .collect(),
            slots: Vec::new(),
        });
    };
    let current = class_world::collect_overrides(world, root, &def);
    let slots_at = class_world::slot_map(world, root);

    let mut variables: Vec<ClassVariableView> = def
        .variables()
        .into_iter()
        .map(|var| {
            let over = current
                .variable_overrides
                .get(&var.name)
                .map(|v| var.kind.coerce(Some(v)));
            ClassVariableView {
                value: over.clone().unwrap_or_else(|| var.default.clone()),
                overridden: over.is_some(),
                name: var.name,
                kind: var.kind,
                default: var.default,
            }
        })
        .collect();
    for (name, value) in &current.variable_overrides {
        if !variables.iter().any(|v| &v.name == name) {
            // An override for a variable the class no longer has: kept, shown.
            variables.push(ClassVariableView {
                name: name.clone(),
                kind: pulsar_class::VariableKind::Other("removed".into()),
                default: Value::Null,
                value: value.clone(),
                overridden: true,
            });
        }
    }

    let slots = def
        .prefab
        .components
        .iter()
        .map(|component| {
            let slot = &component.slot_id;
            let removed = class_world::slot_removed(&current, slot);
            let default = pulsar_class::plan::slot_default(&def, slot).unwrap_or(Value::Null);
            let overridden = match current.component_overrides.get(slot) {
                Some(diff) if !removed => pulsar_class::overrides::leaf_paths(diff)
                    .into_iter()
                    .map(|(path, value)| OverriddenProperty {
                        default: pulsar_class::overrides::get_path(&default, &path)
                            .cloned()
                            .unwrap_or(Value::Null),
                        path,
                        value,
                    })
                    .collect(),
                _ => Vec::new(),
            };
            ClassSlotView {
                slot_id: slot.clone(),
                class_name: component.class_name.clone(),
                object_id: slots_at.get(slot).and_then(|loc| id_of(world, loc.entity)),
                removed,
                overridden,
            }
        })
        .collect();

    Some(ClassInstanceView {
        class_name: def.name.clone(),
        class_id: def.id.to_string(),
        resolved: true,
        variables,
        slots,
    })
}

/// Set a script variable on the instance at `id` (stored as an override
/// only when it differs from the class default). Returns whether `id` is a
/// class instance.
pub fn set_variable(
    world: &mut World,
    id: &str,
    name: &str,
    value: Value,
    registry: &ClassRegistry,
) -> bool {
    let Some(root) = entity(world, id) else {
        return false;
    };
    let Some(mut instance) = class_world::class_instance_of(world, root) else {
        return false;
    };
    instance.variable_overrides.insert(name.to_string(), value);
    if let Some(def) = registry.definition_for(&instance) {
        instance.variable_overrides = pulsar_class::overrides::prune_variable_overrides(
            &def.prefab.variable_defaults(),
            &instance.variable_overrides,
        );
    }
    class_world::store_class_instance(world, root, &instance);
    sync_registered_component_props_to_scene_db(world, id);
    super::changes::record_property_change(id, pulsar_class::CLASS_INSTANCE, name);
    true
}

/// Revert a script variable of the instance at `id` to the class default.
pub fn revert_variable(world: &mut World, id: &str, name: &str) -> bool {
    let Some(root) = entity(world, id) else {
        return false;
    };
    let Some(mut instance) = class_world::class_instance_of(world, root) else {
        return false;
    };
    if instance.variable_overrides.remove(name).is_none() {
        return false;
    }
    class_world::store_class_instance(world, root, &instance);
    sync_registered_component_props_to_scene_db(world, id);
    super::changes::record_property_change(id, pulsar_class::CLASS_INSTANCE, name);
    true
}

/// Revert one property (dot path) of a slot's component to the class
/// default, or the whole slot when `path` is `None` (which also restores a
/// removed slot). Returns whether anything changed.
pub fn revert_slot(
    world: &mut World,
    id: &str,
    slot_id: &str,
    path: Option<&str>,
    registry: &ClassRegistry,
) -> bool {
    let Some(root) = entity(world, id) else {
        return false;
    };
    let Some(stored) = class_world::class_instance_of(world, root) else {
        return false;
    };
    let Some(def) = registry.definition_for(&stored) else {
        return false;
    };
    // Start from the live overrides so unsaved edits are not lost.
    let mut instance = class_world::collect_overrides(world, root, &def);
    let changed = match (path, instance.component_overrides.get_mut(slot_id)) {
        (_, None) => false,
        (None, Some(_)) => instance.component_overrides.remove(slot_id).is_some(),
        (Some(path), Some(diff)) => {
            let removed = pulsar_class::overrides::remove_path(diff, path);
            if diff.as_object().is_some_and(|m| m.is_empty()) {
                instance.component_overrides.remove(slot_id);
            }
            removed
        }
    };
    if !changed {
        return false;
    }
    class_world::store_class_instance(world, root, &instance);
    rebuild_instance(world, id, registry).is_some()
}

// ── Per-property class defaults (details panel) ─────────────────────────────

static CLASS_DEFS_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Bumped whenever class definitions were reloaded (an asset update); panels
/// caching class defaults compare it to know when to re-read them.
pub fn class_defs_generation() -> u64 {
    CLASS_DEFS_GENERATION.load(std::sync::atomic::Ordering::Relaxed)
}

fn bump_class_defs_generation() {
    CLASS_DEFS_GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
}

/// The class instance root `id` belongs to: itself when it is a root, its
/// root when it is a generated child.
pub fn class_root_of(world: &World, id: &str) -> Option<Entity> {
    let mut current = entity(world, id)?;
    loop {
        if class_world::is_class_root(world, current) {
            return Some(current);
        }
        if !class_world::is_generated_child(world, current) {
            return None;
        }
        current = world.parent_of(current)?;
    }
}

/// Class default of one component built from a class slot.
pub struct SlotDefault {
    pub slot_id: String,
    pub class_name: String,
    /// The slot's default data, normalized to the component's shape.
    pub data: Value,
    /// The default as a reflected instance, for per-property reads.
    pub instance: Option<Box<dyn pulsar_reflection::EngineClass>>,
}

impl SlotDefault {
    /// The class default of property `prop_name` (typed, as the property's
    /// getter returns it).
    pub fn property(&self, prop_name: &str) -> Option<Box<dyn std::any::Any>> {
        let instance = self.instance.as_deref()?;
        instance
            .get_properties()
            .into_iter()
            .find(|p| p.name == prop_name)
            .map(|p| (p.getter)(instance))
    }
}

/// For object `id`, the class defaults of its components that were built
/// from a class slot, keyed by component index. Empty for objects that are
/// not part of a resolved class instance.
pub fn slot_defaults(
    world: &World,
    id: &str,
    registry: &ClassRegistry,
) -> std::collections::HashMap<usize, SlotDefault> {
    let mut out = std::collections::HashMap::new();
    let Some(root) = class_root_of(world, id) else {
        return out;
    };
    let Some(instance) = class_world::class_instance_of(world, root) else {
        return out;
    };
    let Some(def) = registry.definition_for(&instance) else {
        return out;
    };
    let Some(object) = entity(world, id) else {
        return out;
    };
    for (index, record) in class_world::component_records(world, object)
        .iter()
        .enumerate()
    {
        let Some(slot) = class_world::record_slot_id(record) else {
            continue;
        };
        let Some(data) = pulsar_class::plan::slot_default(&def, slot) else {
            continue;
        };
        let instance = pulsar_reflection::REGISTRY
            .create_instance_from_json(&record.class_name, &data)
            .and_then(Result::ok);
        out.insert(
            index,
            SlotDefault {
                slot_id: slot.to_string(),
                class_name: record.class_name.clone(),
                data,
                instance,
            },
        );
    }
    out
}

/// Whether a property value equals its class default (both serialized
/// through the reflection registry; float noise tolerated).
pub fn property_equals_default(current: &dyn std::any::Any, default: &dyn std::any::Any) -> bool {
    let registry = &pulsar_reflection::RUNTIME_TYPE_REGISTRY;
    match (
        registry.serialize_json_for_any(current),
        registry.serialize_json_for_any(default),
    ) {
        (Ok(a), Ok(b)) => pulsar_class::overrides::values_equal(&a, &b),
        _ => true,
    }
}

// ── Asset updates: class edits reach placed instances ───────────────────────

/// Whether `event` names the class `instance` refers to (by GUID, else by
/// class directory name).
fn event_names_class(
    event: &plugin_editor_api::AssetUpdated,
    instance: &ClassInstance,
    registry: &ClassRegistry,
) -> bool {
    if let Some(id) = &event.id {
        if instance.class.as_str() == id {
            return true;
        }
    }
    let Some(dir_name) = event
        .path
        .as_ref()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
    else {
        return false;
    };
    registry
        .resolve(instance)
        .map(|entry| entry.name == dir_name)
        .unwrap_or(instance.class_name == dir_name)
}

/// Rebuild every placed instance of the class an asset update names from
/// the class's new definition, keeping each instance's overrides (current
/// edits included). Returns the ids of the rebuilt objects (roots and their
/// generated children). Events for other asset kinds are ignored.
pub fn apply_class_asset_update(
    world: &mut World,
    event: &plugin_editor_api::AssetUpdated,
) -> Vec<ObjectId> {
    if event.kind != plugin_editor_api::AssetKind::Blueprint {
        return Vec::new();
    }
    let registry = match &event.path {
        Some(dir) if dir.is_dir() => registry_for_class_dir(dir),
        _ => project_registry(),
    };
    let roots: Vec<(ObjectId, Entity)> = world
        .query::<&engine_backend::scene::StableId>()
        .map(|(e, id)| (id.0.clone(), e))
        .filter(|(_, e)| class_world::is_class_root(world, *e))
        .collect();
    // First fold every matching instance's live values into its overrides,
    // diffed against the definition it was built from (the new one would
    // turn every value it did not override into one); only then rebuild,
    // which records the new definition as the one built from. Slot ids are
    // stable across class edits, so the overrides apply to the new one.
    let mut matching = Vec::new();
    for (id, root) in roots {
        let Some(stored) = class_world::class_instance_of(world, root) else {
            continue;
        };
        if !event_names_class(event, &stored, &registry) {
            continue;
        }
        let Some(entry) = registry.resolve(&stored) else {
            continue;
        };
        if let Some(built_from) = built_definition(&entry.id) {
            let current = class_world::collect_overrides(world, root, &built_from);
            class_world::store_class_instance(world, root, &current);
        }
        matching.push(id);
    }
    let mut touched = Vec::new();
    for id in matching {
        if let Some(ids) = rebuild_instance(world, &id, &registry) {
            touched.extend(ids);
        }
    }
    if !touched.is_empty() {
        bump_class_defs_generation();
    }
    touched
}

/// Definitions instances were last built from, per class, so an asset
/// update can tell the instance's own edits from old class defaults.
fn built_definitions() -> &'static std::sync::Mutex<
    std::collections::HashMap<pulsar_class::ClassId, pulsar_class::ClassDefinition>,
> {
    static BUILT: std::sync::OnceLock<
        std::sync::Mutex<
            std::collections::HashMap<pulsar_class::ClassId, pulsar_class::ClassDefinition>,
        >,
    > = std::sync::OnceLock::new();
    BUILT.get_or_init(Default::default)
}

fn remember_built_definition(def: &pulsar_class::ClassDefinition) {
    built_definitions()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .insert(def.id.clone(), def.clone());
}

fn built_definition(id: &pulsar_class::ClassId) -> Option<pulsar_class::ClassDefinition> {
    built_definitions()
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(id)
        .cloned()
}
