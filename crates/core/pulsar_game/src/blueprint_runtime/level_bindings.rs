//! Level-driven Blueprint class bindings (#650): the data model that decides
//! which scene objects run which Blueprint classes at play time.
//!
//! A level file may carry a top-level `blueprint_bindings` section keyed by
//! object **StableId** (`pulsar_scene::format::BlueprintBindings`). At
//! play-mode bootstrap the host hydrates the level through
//! `engine_backend::scene::RuntimeLevel`, then hands the returned extras to
//! [`apply_blueprint_bindings`], which resolves every StableId against the
//! hydrated store and spawns one dispatcher instance per (object, class)
//! pair via [`BlueprintDispatcher::spawn_instance_for_entity`] — so each
//! instance's `begin_play`/`tick` component ops address its own entity from
//! the first dispatched event.
//!
//! # Schema (see `tests/fixtures/level_bindings_sample.level.json`)
//!
//! ```json
//! {
//!   "version": "2.1",
//!   "objects": [ { "id": "lever_a", "...": "..." } ],
//!   "blueprint_bindings": {
//!     "lever_a": [
//!       { "class_name": "Lever", "overrides": { "speed": 7.5 } },
//!       { "class_name": "Alarm" }
//!     ]
//!   }
//! }
//! ```
//!
//! Invariants: keys are StableIds, never names (renames never orphan a
//! binding); one class binds at most once per object ([`BindingError::
//! DuplicateClass`] otherwise); per-binding failures are collected, never
//! fatal — one stale or uncompiled binding cannot block play mode.
//!
//! # Runtime add/remove (gameplay-driven scripted objects)
//!
//! [`bind_object_class`] attaches a class to an already-live entity at any
//! time; [`unbind_object_class`] removes the binding and unregisters its
//! instance cleanly (the dispatcher drops the arena and pending `begin_play`,
//! so nothing keeps ticking). Spawning brand-new scene objects is the
//! store's job (`WorldSceneStore::spawn`) — combine both for full runtime
//! spawning of scripted objects.
//!
//! # Migration note (ScriptComponent → level bindings)
//!
//! The legacy path — helio's `ScriptComponent::sync_component` registering
//! string-keyed entries in `SCRIPT_REGISTRY` — stays functional but is
//! superseded by this format. It cannot be auto-converted engine-side:
//! `ScriptComponent` stores a blueprint *directory* (`graph_save.json`),
//! not a compiled class name, so directory→class mapping is editor policy.
//! Author new bindings here; deprecating the old component upstream is
//! tracked for the editor phase (F).

use super::{BlueprintDispatcher, ExecutorError};
use pulsar_scene::format::BlueprintBindings;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use pulsar_scenedb::Entity;
use serde_json::Value as JsonValue;

/// Where a class's compiled bytecode lives under a project root — the exact
/// layout `core_project_builder`'s generated discovery scans.
pub fn bytecode_path_for_class(project_root: &Path, class_name: &str) -> PathBuf {
    project_root
        .join("src")
        .join("classes")
        .join(class_name)
        .join("events")
        .join(".build")
        .join("bytecode.json")
}

/// Deterministic dispatcher instance id for one (object, class) binding.
///
/// Stable across loads so tooling can address an object's instance by its
/// scene identity; distinct per class so multiple bindings on one object
/// never collide.
pub fn instance_id_for(stable_id: &str, class_name: &str) -> String {
    format!("{stable_id}::{class_name}")
}

/// One applied binding: which object now runs which class on which entity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedBinding {
    pub stable_id: String,
    pub class_name: String,
    /// Dispatcher instance id (`instance_id_for`).
    pub instance_id: String,
    pub entity: Entity,
}

/// Why one binding could not be applied. Per-binding only — never aborts a
/// whole level load.
#[derive(Debug)]
pub enum BindingError {
    /// No live object with this StableId in the hydrated store (deleted
    /// after the binding was authored).
    UnknownObject {
        stable_id: String,
    },
    /// Two entries on one object name the same class.
    DuplicateClass {
        stable_id: String,
        class_name: String,
    },
    /// No compiled bytecode for the class under the project root.
    BytecodeMissing {
        class_name: String,
        path: PathBuf,
    },
    Io {
        path: String,
        message: String,
    },
    Serialization(String),
    Executor(ExecutorError),
}

impl std::fmt::Display for BindingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BindingError::UnknownObject { stable_id } => {
                write!(f, "no live object with stable id '{stable_id}'")
            }
            BindingError::DuplicateClass {
                stable_id,
                class_name,
            } => {
                write!(f, "class '{class_name}' bound twice to '{stable_id}'")
            }
            BindingError::BytecodeMissing { class_name, path } => {
                write!(
                    f,
                    "no compiled bytecode for class '{class_name}' at {}",
                    path.display()
                )
            }
            BindingError::Io { path, message } => write!(f, "failed to read '{path}': {message}"),
            BindingError::Serialization(message) => write!(f, "invalid bytecode json: {message}"),
            BindingError::Executor(error) => write!(f, "dispatcher refused: {error}"),
        }
    }
}

impl From<ExecutorError> for BindingError {
    fn from(error: ExecutorError) -> Self {
        BindingError::Executor(error)
    }
}

/// Result of applying a whole level's bindings.
#[derive(Debug, Default)]
pub struct ApplyReport {
    pub applied: Vec<AppliedBinding>,
    pub failures: Vec<BindingFailure>,
}

/// One failed binding, carrying enough context to point at the file entry.
#[derive(Debug)]
pub struct BindingFailure {
    pub stable_id: String,
    pub class_name: String,
    pub error: BindingError,
}

/// Apply a whole level's bindings (play-mode bootstrap, #650).
///
/// Iterates deterministically (StableId order), resolving each object
/// through the hydrated store before spawning. Failures are collected into
/// the report and logged — they describe file-level staleness (deleted
/// objects, uncompiled classes), not transient conditions, so play mode
/// proceeds without them.
///
/// Re-applying the same bindings replaces prior instances of identical ids
/// (the dispatcher keys instances by id) — call once per level load.
pub fn apply_blueprint_bindings(
    dispatcher: &mut BlueprintDispatcher,
    store: &engine_backend::scene::WorldSceneStore,
    project_root: &Path,
    bindings: &BlueprintBindings,
) -> ApplyReport {
    let mut report = ApplyReport::default();
    for (stable_id, class_bindings) in bindings {
        for binding in class_bindings {
            match bind_object_class(
                dispatcher,
                store,
                project_root,
                stable_id,
                &binding.class_name,
                binding.overrides.clone(),
            ) {
                Ok(applied) => report.applied.push(applied),
                Err(error) => {
                    tracing::warn!(
                        stable_id = %stable_id,
                        class = %binding.class_name,
                        "Skipping blueprint binding: {error}"
                    );
                    report.failures.push(BindingFailure {
                        stable_id: stable_id.clone(),
                        class_name: binding.class_name.clone(),
                        error,
                    });
                }
            }
        }
    }
    report
}

/// Attach one Blueprint class to one live scene object at runtime (#650).
///
/// The gameplay-driven add half of the API: resolve `stable_id` → entity,
/// locate the class's compiled bytecode, and spawn a bound dispatcher
/// instance whose overrides seed its state arena. Returns the applied
/// binding (its instance id feeds [`unbind_object_class`]).
pub fn bind_object_class(
    dispatcher: &mut BlueprintDispatcher,
    store: &engine_backend::scene::WorldSceneStore,
    project_root: &Path,
    stable_id: &str,
    class_name: &str,
    overrides: HashMap<String, JsonValue>,
) -> Result<AppliedBinding, BindingError> {
    let Some(entity) = store.entity_for(stable_id) else {
        return Err(BindingError::UnknownObject {
            stable_id: stable_id.to_string(),
        });
    };

    let instance_id = instance_id_for(stable_id, class_name);
    if dispatcher.instance_ids().contains(&instance_id) {
        return Err(BindingError::DuplicateClass {
            stable_id: stable_id.to_string(),
            class_name: class_name.to_string(),
        });
    }

    let bytecode_path = bytecode_path_for_class(project_root, class_name);
    if !bytecode_path.exists() {
        return Err(BindingError::BytecodeMissing {
            class_name: class_name.to_string(),
            path: bytecode_path,
        });
    }

    let overrides = if overrides.is_empty() {
        None
    } else {
        Some(overrides)
    };
    dispatcher.spawn_instance_for_entity(instance_id.clone(), &bytecode_path, entity, overrides)?;

    tracing::info!(
        stable_id = %stable_id,
        class = %class_name,
        "Bound blueprint class to scene object"
    );
    Ok(AppliedBinding {
        stable_id: stable_id.to_string(),
        class_name: class_name.to_string(),
        instance_id,
        entity,
    })
}

/// Remove one object's Blueprint class binding at runtime (#650).
///
/// Unregisters the dispatcher instance cleanly — its arena is dropped and a
/// still-pending `begin_play` is cancelled, so nothing ticks afterwards.
/// Returns `false` when no such binding exists (unknown object or class).
pub fn unbind_object_class(
    dispatcher: &mut BlueprintDispatcher,
    stable_id: &str,
    class_name: &str,
) -> bool {
    dispatcher
        .unregister_instance(&instance_id_for(stable_id, class_name))
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint_runtime::CompiledBytecode;

    fn empty_dispatcher() -> BlueprintDispatcher {
        BlueprintDispatcher::new().expect("blueprint executor loads")
    }

    #[test]
    fn instance_ids_pair_stable_id_with_class() {
        assert_eq!(instance_id_for("lever_a", "Lever"), "lever_a::Lever");
        assert_ne!(
            instance_id_for("lever_a", "Alarm"),
            instance_id_for("lever_a", "Lever"),
            "multiple classes on one object never collide"
        );
    }

    /// The discovered path must stay identical to what the generated
    /// engine_main scans (`src/classes/<C>/events/.build/bytecode.json`).
    #[test]
    fn bytecode_path_matches_generated_discovery_layout() {
        let path = bytecode_path_for_class(Path::new("/proj"), "Enemy");
        assert_eq!(
            path,
            Path::new("/proj/src/classes/Enemy/events/.build/bytecode.json")
        );
    }

    #[test]
    fn unknown_stable_ids_fail_per_binding() {
        let mut dispatcher = empty_dispatcher();
        let store = engine_backend::scene::WorldSceneStore::new();
        let error = bind_object_class(
            &mut dispatcher,
            &store,
            Path::new("."),
            "ghost",
            "Any",
            HashMap::new(),
        )
        .unwrap_err();
        assert!(
            matches!(error, BindingError::UnknownObject { .. }),
            "{error}"
        );
    }

    #[test]
    fn missing_bytecode_is_typed_not_an_io_surprise() {
        let mut dispatcher = empty_dispatcher();
        let mut store = engine_backend::scene::WorldSceneStore::new();
        store.spawn(Some("obj".into()), "Obj", None).expect("spawn");
        let error = bind_object_class(
            &mut dispatcher,
            &store,
            Path::new("."),
            "obj",
            "NeverCompiled",
            HashMap::new(),
        )
        .unwrap_err();
        assert!(
            matches!(error, BindingError::BytecodeMissing { .. }),
            "{error}"
        );
    }

    /// Removing a binding unregisters cleanly even while `begin_play` is
    /// still queued — the cancelled instance must never dispatch.
    #[test]
    fn unbinding_cancels_pending_begin_play() {
        let mut dispatcher = empty_dispatcher();
        // Register directly in memory so this test needs no disk fixture;
        // one variable keeps the arena non-empty (real bytecode always is).
        let mut bytecode = CompiledBytecode::new("TickProbe");
        bytecode.add_variable(super::super::VariableDescriptor::f32("speed", 0, 1.0));
        bytecode.calculate_arena_size();
        dispatcher
            .register_bytecode(instance_id_for("lever", "TickProbe"), bytecode, None, None)
            .expect("register");
        assert!(unbind_object_class(&mut dispatcher, "lever", "TickProbe"));
        assert!(!dispatcher
            .instance_ids()
            .contains(&instance_id_for("lever", "TickProbe")));
        assert!(
            !unbind_object_class(&mut dispatcher, "lever", "TickProbe"),
            "already gone"
        );

        let mut world = pulsar_scenedb::World::new();
        dispatcher.dispatch_pending_begin_play(&mut world);
        assert_eq!(
            dispatcher.instance_ids().len(),
            0,
            "cancelled instance must not reappear via begin_play"
        );
    }
}
