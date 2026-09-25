//! Script classes on the engine script VM (`pulsar_script_runtime`).
//!
//! Every class's editor build writes `events/.build/module.json`; the
//! [`ScriptRuntime`] runs it, driven by the [`TickLoop`](crate::tick::TickLoop).
//! The runtime is language-neutral: any scripting plugin that writes a
//! module for a class is loaded the same way.
//!
//! # Level bindings
//!
//! A level file may carry a `blueprint_bindings` section keyed by object
//! **StableId** (`pulsar_scene::format::BlueprintBindings`): which classes
//! each object runs, with per-instance variable overrides. At play-mode
//! bootstrap [`apply_script_bindings`] resolves every StableId against the
//! hydrated store and spawns one bound instance per (object, class) pair.
//! Keys are StableIds, never names, so renames never orphan a binding;
//! per-binding failures are collected, never fatal.

use std::path::{Path, PathBuf};

use engine_backend::scene::SceneWorldExt;
use pulsar_scene::format::BlueprintBindings;
pub use pulsar_script_runtime::{RuntimeError, ScriptRuntime};

use pulsar_scenedb::Entity;

// Scene-object lookup for scripts. The result is `entity::none()` when no
// object matches; component references built from it resolve to nothing.
inventory::submit! {
    pulsar_script_vm::NativeRegistration {
        build: || {
            pulsar_script_vm::NativeFn::builder("world::find_by_stable_id")
                .doc("The scene object with this stable id (the level file's object id).")
                .side_effect_free()
                .attr("category", "World")
                .params(["stable_id"])
                .build(|host: &mut pulsar_script_vm::Host<'_>, id: String| {
                    engine_backend::scene::entity_with_stable_id(host.world, &id)
                        .unwrap_or(pulsar_scenedb::Entity::DANGLING)
                })
        },
    }
}

inventory::submit! {
    pulsar_script_vm::NativeRegistration {
        build: || {
            pulsar_script_vm::NativeFn::builder("world::find_by_name")
                .doc("The first scene object with this display name.")
                .side_effect_free()
                .attr("category", "World")
                .params(["name"])
                .build(|host: &mut pulsar_script_vm::Host<'_>, name: String| {
                    engine_backend::scene::first_entity_named(host.world, &name)
                        .unwrap_or(pulsar_scenedb::Entity::DANGLING)
                })
        },
    }
}

/// Deterministic instance id for one (object, class) binding: stable
/// across loads, distinct per class.
pub fn instance_id_for(stable_id: &str, class_name: &str) -> String {
    format!("{stable_id}::{class_name}")
}

/// One applied binding: which object now runs which class on which entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedBinding {
    pub stable_id: String,
    pub class_name: String,
    /// Runtime instance id (`instance_id_for`).
    pub instance_id: String,
    pub entity: Entity,
}

/// Why one binding could not be applied. Per binding only: never aborts a
/// whole level load.
#[derive(Debug)]
pub enum BindingError {
    /// No live object with this StableId in the hydrated store.
    UnknownObject { stable_id: String },
    /// Two entries on one object name the same class.
    DuplicateClass { stable_id: String, class_name: String },
    /// No compiled module for the class under the project root.
    ModuleMissing { class_name: String, path: PathBuf },
    /// The script runtime refused (bad module, bad override, ..).
    Script(RuntimeError),
}

impl std::fmt::Display for BindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownObject { stable_id } => write!(f, "no live object with stable id '{stable_id}'"),
            Self::DuplicateClass { stable_id, class_name } => {
                write!(f, "class '{class_name}' bound twice to '{stable_id}'")
            }
            Self::ModuleMissing { class_name, path } => {
                write!(f, "no compiled script module for class '{class_name}' at {}", path.display())
            }
            Self::Script(error) => write!(f, "script runtime refused: {error}"),
        }
    }
}

/// One failed binding.
#[derive(Debug)]
pub struct BindingFailure {
    pub stable_id: String,
    pub class_name: String,
    pub error: BindingError,
}

/// Result of applying a level's bindings.
#[derive(Debug, Default)]
pub struct ApplyReport {
    pub applied: Vec<AppliedBinding>,
    pub failures: Vec<BindingFailure>,
}

/// Where a class's compiled script module lives.
pub fn module_path_for_class(project_root: &Path, class_name: &str) -> PathBuf {
    project_root
        .join("src")
        .join("classes")
        .join(class_name)
        .join("events")
        .join(".build")
        .join("module.json")
}

/// A runtime with every engine native (pulsar_std, world components,
/// reflected methods). Native library shadow copies go in the system temp
/// directory.
pub fn new_runtime() -> ScriptRuntime {
    // Reference pulsar_std so its natives are linked into this binary.
    let _ = pulsar_std::get_all_nodes().len();
    ScriptRuntime::new(std::env::temp_dir().join("pulsar_script_libraries"))
}

/// Load every class under `classes_dir` that has a compiled module and
/// give each one default, unbound instance (`<class>__vm_default`), the
/// same policy as the bytecode discovery. Returns the loaded class names.
pub fn load_project_classes(runtime: &mut ScriptRuntime, classes_dir: &Path) -> Vec<String> {
    let mut loaded = Vec::new();
    let Ok(entries) = std::fs::read_dir(classes_dir) else { return loaded };
    let mut dirs: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    for dir in dirs {
        let module = dir.join("events").join(".build").join("module.json");
        if !module.exists() {
            continue;
        }
        match runtime.load_class_file(&module) {
            Ok(class) => {
                let instance = format!("{class}__vm_default");
                match runtime.spawn(instance.clone(), &class, None, &[]) {
                    Ok(()) => tracing::info!("Script class loaded: {class} → {instance}"),
                    Err(e) => tracing::warn!("Script class '{class}' loaded but not spawned: {e}"),
                }
                loaded.push(class);
            }
            Err(e) => tracing::warn!("Failed to load script module {}: {e}", module.display()),
        }
    }
    loaded
}

/// Apply a level's bindings: one instance per (object, class), bound to
/// the object's entity, with the binding's overrides, loading each class's
/// module on first use.
pub fn apply_script_bindings(
    runtime: &mut ScriptRuntime,
    store: &pulsar_scenedb::SceneDb,
    project_root: &Path,
    bindings: &BlueprintBindings,
) -> ApplyReport {
    let mut report = ApplyReport::default();
    for (stable_id, class_bindings) in bindings {
        for binding in class_bindings {
            let class = &binding.class_name;
            let module = module_path_for_class(project_root, class);
            let result = (|| {
                let entity = store
                    .world
                    .entity_for(stable_id)
                    .ok_or_else(|| BindingError::UnknownObject { stable_id: stable_id.clone() })?;
                if !runtime.has_class(class) {
                    if !module.exists() {
                        return Err(BindingError::ModuleMissing { class_name: class.clone(), path: module.clone() });
                    }
                    runtime.load_class_file(&module).map_err(BindingError::Script)?;
                }
                let instance_id = instance_id_for(stable_id, class);
                if runtime.instance_ids().contains(&instance_id) {
                    return Err(BindingError::DuplicateClass {
                        stable_id: stable_id.clone(),
                        class_name: class.clone(),
                    });
                }
                let overrides = binding.overrides.clone().into_iter().collect();
                runtime
                    .spawn_with_json(instance_id.clone(), class, Some(entity), &overrides)
                    .map_err(BindingError::Script)?;
                bind_class_slots(runtime, &instance_id, &store.world, entity);
                Ok(AppliedBinding {
                    stable_id: stable_id.clone(),
                    class_name: class.clone(),
                    instance_id,
                    entity,
                })
            })();
            match result {
                Ok(applied) => report.applied.push(applied),
                Err(error) => {
                    tracing::warn!(stable_id = %stable_id, class = %class, "Skipping script binding: {error}");
                    report.failures.push(BindingFailure {
                        stable_id: stable_id.clone(),
                        class_name: class.clone(),
                        error,
                    });
                }
            }
        }
    }
    report
}

// ---- class component slots (#921) ------------------------------------------

/// Fill the hidden component-slot handles (`__slot:<uuid>` variables, see
/// `pulsar_class::SLOT_VARIABLE_PREFIX`) of instance `instance_id` from the
/// placement of the class instance rooted at `root`.
///
/// This is the one place slot UUIDs are resolved: each becomes a handle to
/// the instance's real component, and the script only ever uses the
/// handle. A slot the placed instance does not have (removed from the
/// class, or by the instance) keeps a `none` handle, with a warning naming
/// the class and the slot. Returns the slots left unresolved.
pub fn bind_class_slots(
    runtime: &mut ScriptRuntime,
    instance_id: &str,
    world: &pulsar_scenedb::World,
    root: Entity,
) -> Vec<String> {
    let Some(class) = runtime.class_of(instance_id).map(str::to_owned) else {
        return Vec::new();
    };
    let slot_vars: Vec<(String, String)> = runtime
        .class_variables(&class)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, ty)| {
            pulsar_class::slot_of_variable(&name)?;
            match ty {
                pulsar_script_vm::Type::Component(component) => Some((name, component)),
                _ => None,
            }
        })
        .collect();
    if slot_vars.is_empty() {
        return Vec::new();
    }
    let placement = pulsar_class::world::placement(world, root);
    let mut unresolved = Vec::new();
    for (variable, component_class) in slot_vars {
        let slot = pulsar_class::slot_of_variable(&variable).unwrap_or_default().to_owned();
        let handle = placement
            .handle(&slot)
            .filter(|h| h.class_name == component_class)
            .and_then(|h| {
                let id = pulsar_world_registry::component_id_for_class(&component_class)?;
                Some(pulsar_scenedb::ComponentRef::new(h.entity, id))
            });
        match handle {
            Some(handle) => {
                if let Err(error) = runtime.set_variable(
                    instance_id,
                    &variable,
                    pulsar_script_vm::Value::Component(handle),
                ) {
                    tracing::warn!(class = %class, slot = %slot, "Could not bind component slot: {error}");
                    unresolved.push(slot);
                }
            }
            None => {
                tracing::warn!(
                    class = %class,
                    slot = %slot,
                    component = %component_class,
                    "Component slot not found on the placed instance; its handle stays none"
                );
                unresolved.push(slot);
            }
        }
    }
    unresolved
}

/// Reload the script module of the class an [`AssetUpdated`] names (by the
/// class directory's name) if the runtime has it loaded, then re-bind the
/// component-slot handles of every instance of it. Returns the class name
/// when it reloaded.
///
/// [`AssetUpdated`]: pulsar_events::AssetUpdated
pub fn reload_class_for_asset(
    runtime: &mut ScriptRuntime,
    world: &pulsar_scenedb::World,
    project_root: &Path,
    event: &pulsar_events::AssetUpdated,
) -> Option<String> {
    if event.kind != pulsar_events::AssetKind::Blueprint {
        return None;
    }
    let class = match &event.path {
        Some(path) => path.file_name()?.to_str()?.to_owned(),
        None => {
            let id = event.id.as_deref()?;
            pulsar_class::ClassRegistry::scan(project_root)
                .by_id(&pulsar_class::ClassId::from(id))?
                .name
                .clone()
        }
    };
    if !runtime.has_class(&class) {
        return None;
    }
    let module = module_path_for_class(project_root, &class);
    if let Err(error) = runtime.load_class_file(&module) {
        tracing::warn!(class = %class, "Class updated but its script module did not reload: {error}");
        return None;
    }
    let instances: Vec<(String, Entity)> = runtime
        .instance_ids()
        .iter()
        .filter(|id| runtime.class_of(id) == Some(class.as_str()))
        .filter_map(|id| Some((id.clone(), runtime.entity_of(id)?)))
        .collect();
    for (id, entity) in instances {
        bind_class_slots(runtime, &id, world, entity);
    }
    Some(class)
}

/// Keep `runtime` in step with class edits: every published class
/// [`AssetUpdated`](pulsar_events::AssetUpdated) reloads that class's
/// module (see [`reload_class_for_asset`]). Live instance handling (spawn,
/// despawn, re-placing instances) is the script runtime's own phase (#922).
pub fn subscribe_class_reloads(
    runtime: std::sync::Arc<std::sync::Mutex<ScriptRuntime>>,
    scene: engine_backend::scene::SharedScene,
    project_root: PathBuf,
) -> pulsar_events::AssetSubscription {
    pulsar_events::subscribe_asset_updates(Some(pulsar_events::AssetKind::Blueprint), move |event| {
        let world_guard = scene.read();
        let mut runtime = runtime.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(class) = reload_class_for_asset(&mut runtime, &world_guard.world, &project_root, event) {
            tracing::info!(class = %class, "Reloaded script class after an asset update");
        }
    })
}
