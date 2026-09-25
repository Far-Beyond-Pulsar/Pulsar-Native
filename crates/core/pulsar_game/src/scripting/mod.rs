//! Script classes on the engine script VM (`pulsar_script_runtime`).
//!
//! Every class's editor build writes `events/.build/module.json`; the
//! [`ScriptRuntime`] runs it. The runtime is language-neutral: any
//! scripting plugin that writes a module for a class is loaded the same way.
//!
//! What runs is decided by the world, not by side tables (#922): the
//! [`ScriptDriver`] gives every placed class instance (an entity carrying a
//! `pulsar_class::ClassInstance`) one script instance bound to that entity,
//! and follows the world as instances are placed, spawned and destroyed. The
//! standalone game, Play-in-Editor and runtime spawns all go through it; the
//! [`TickLoop`](crate::tick::TickLoop) runs it as its script phase. Classes
//! listed as global scripts in `Pulsar/scripting.json` get one unbound
//! instance each. See [`driver`] for lifecycle order and identity, and
//! [`commands`] for the `world::spawn` / `world::destroy` natives.

use std::path::{Path, PathBuf};

pub use pulsar_script_runtime::{RuntimeError, ScriptRuntime};

use pulsar_scenedb::Entity;

pub mod commands;
pub mod driver;
#[cfg(test)]
mod tests;

pub use commands::WorldCommand;
pub use driver::{
    global_instance_id, instance_id_for, scripting_config_path, DriverReport, ReloadRequests,
    ScriptDriver, ScriptingConfig, SCRIPTING_CONFIG_FILE,
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

/// Where a class's compiled script module lives, by class directory name.
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
/// reflected methods, the `world::*` natives). Native library shadow copies
/// go in the system temp directory.
pub fn new_runtime() -> ScriptRuntime {
    // Reference pulsar_std so its natives are linked into this binary.
    let _ = pulsar_std::get_all_nodes().len();
    ScriptRuntime::new(std::env::temp_dir().join("pulsar_script_libraries"))
}

/// A driver for the project at `project_root` on a fresh [`new_runtime`].
pub fn new_driver(project_root: impl Into<PathBuf>) -> ScriptDriver {
    ScriptDriver::new(new_runtime(), project_root)
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
