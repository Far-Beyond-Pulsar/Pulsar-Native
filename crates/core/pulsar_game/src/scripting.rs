//! Script classes on the engine script VM (`pulsar_script_runtime`).
//!
//! A class whose editor build produced `events/.build/module.json` runs on
//! the [`ScriptRuntime`]; a class with only the older PBGC
//! `bytecode.json` keeps running on the [`BlueprintDispatcher`](crate::blueprint_runtime::BlueprintDispatcher)
//! until it is rebuilt. Both are driven by the [`TickLoop`](crate::TickLoop).
//!
//! The runtime is language-neutral: any scripting plugin that writes a
//! module for a class is loaded the same way.

use std::path::{Path, PathBuf};

use engine_backend::scene::SceneWorldExt;
use pulsar_scene::format::BlueprintBindings;
pub use pulsar_script_runtime::{RuntimeError, ScriptRuntime};

use crate::blueprint_runtime::level_bindings::{
    instance_id_for, AppliedBinding, ApplyReport, BindingError, BindingFailure,
};

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

/// Apply a level's bindings for every class that has a compiled module:
/// one instance per (object, class), bound to the object's entity, with
/// the binding's overrides. Returns the report and the bindings left for
/// the bytecode dispatcher (classes without a module).
pub fn apply_script_bindings(
    runtime: &mut ScriptRuntime,
    store: &pulsar_scenedb::SceneDb,
    project_root: &Path,
    bindings: &BlueprintBindings,
) -> (ApplyReport, BlueprintBindings) {
    let mut report = ApplyReport::default();
    let mut remaining = BlueprintBindings::new();
    for (stable_id, class_bindings) in bindings {
        for binding in class_bindings {
            let class = &binding.class_name;
            let module = module_path_for_class(project_root, class);
            if !runtime.has_class(class) && !module.exists() {
                remaining.entry(stable_id.clone()).or_default().push(binding.clone());
                continue;
            }
            let result = (|| {
                let entity = store
                    .world
                    .entity_for(stable_id)
                    .ok_or_else(|| BindingError::UnknownObject { stable_id: stable_id.clone() })?;
                if !runtime.has_class(class) {
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
    (report, remaining)
}
