//! Blueprint executor for running bytecode at runtime.
//!
//! Manages the virtual machine, loaded blueprints, and bytecode execution.

use super::byte_arena::ByteArena;
use super::component_ops::{component_op_handlers, run_with_component_context};
use super::compiled_bytecode::CompiledBytecode;
use pbgc::{vm, BpProgram};
use pulsar_bp_executor::{BpExecutor as NativeExecutor, ExecutorError as NativeExecutorError};
use pulsar_scenedb::{Entity, World};
use pulsar_std_bundle::{expected_sha256, extract_to_tempfile};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Errors that can occur during blueprint execution.
#[derive(Debug)]
pub enum ExecutorError {
    /// Error loading native library
    NativeLibrary(NativeExecutorError),

    /// Error preparing bytecode (function pointer patching)
    Prepare(NativeExecutorError),

    /// Error during bytecode execution
    Execution(String),

    /// Blueprint not loaded
    BlueprintNotLoaded(String),

    /// No instance registered under the requested object id (#648).
    InstanceNotRegistered(String),

    /// IO error
    Io(std::io::Error),

    /// Serialization error
    Serialization(serde_json::Error),
}

impl std::fmt::Display for ExecutorError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ExecutorError::NativeLibrary(e) => write!(f, "Native library error: {}", e),
            ExecutorError::Prepare(e) => write!(f, "Prepare error: {}", e),
            ExecutorError::Execution(e) => write!(f, "Execution error: {}", e),
            ExecutorError::BlueprintNotLoaded(name) => write!(f, "Blueprint not loaded: {}", name),
            ExecutorError::InstanceNotRegistered(id) => {
                write!(f, "Blueprint instance not registered: {}", id)
            }
            ExecutorError::Io(e) => write!(f, "IO error: {}", e),
            ExecutorError::Serialization(e) => write!(f, "Serialization error: {}", e),
        }
    }
}

impl std::error::Error for ExecutorError {}

impl From<NativeExecutorError> for ExecutorError {
    fn from(e: NativeExecutorError) -> Self {
        ExecutorError::NativeLibrary(e)
    }
}

impl From<std::io::Error> for ExecutorError {
    fn from(e: std::io::Error) -> Self {
        ExecutorError::Io(e)
    }
}

impl From<serde_json::Error> for ExecutorError {
    fn from(e: serde_json::Error) -> Self {
        ExecutorError::Serialization(e)
    }
}

/// A loaded blueprint with patched bytecode ready for execution.
pub struct LoadedBlueprint {
    /// Blueprint class name
    pub class_name: String,

    /// Original compiled bytecode
    pub bytecode: CompiledBytecode,

    /// Event programs with patched function pointers
    pub programs: HashMap<String, BpProgram>,
}

/// Blueprint executor manages the virtual machine and loaded blueprints.
pub struct BlueprintExecutor {
    /// Native function library (pulsar_std)
    native_executor: NativeExecutor,

    /// Loaded blueprints (class_name -> loaded blueprint)
    loaded_blueprints: HashMap<String, Arc<LoadedBlueprint>>,

    /// Temp file handle (keep alive for library lifetime)
    _temp_lib: pulsar_std_bundle::TempLib,
}

/// The world slice one blueprint event executes against (#648).
///
/// `comp_*` nodes read and mutate authoritative SceneDB state through this:
/// `world` is the exclusive borrow held for the duration of the event (the
/// tick loop's phase-3 write lock), and `entity` is the scene object the
/// running instance is bound to. An instance that is registered but not yet
/// bound executes with `entity: None` — graphs run, component ops refuse
/// with a logged error rather than misaddressing writes.
pub struct EventWorld<'w> {
    /// Exclusive world access for the event's duration.
    pub world: &'w mut World,
    /// Scene entity the executing instance is bound to, if any.
    pub entity: Option<Entity>,
}

impl<'w> EventWorld<'w> {
    /// World access for a bound instance: component ops address `entity`.
    pub fn bound(world: &'w mut World, entity: Entity) -> Self {
        Self {
            world,
            entity: Some(entity),
        }
    }

    /// World access for an unbound instance: component ops refuse.
    pub fn unbound(world: &'w mut World) -> Self {
        Self { world, entity: None }
    }
}

impl BlueprintExecutor {
    /// Create a new blueprint executor.
    ///
    /// This extracts the pulsar_std native library and loads it.
    pub fn new() -> Result<Self, ExecutorError> {
        // Extract pulsar_std to temp file
        let temp_lib = extract_to_tempfile()
            .map_err(|e| ExecutorError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;

        // Load native library
        let native_executor = NativeExecutor::load(&temp_lib.path, Some(expected_sha256()))?;

        Ok(Self {
            native_executor,
            loaded_blueprints: HashMap::new(),
            _temp_lib: temp_lib,
        })
    }

    /// Load a compiled blueprint from bytecode.json file.
    ///
    /// This reads the bytecode, patches function pointers, and caches it for execution.
    pub fn load_blueprint_from_file(&mut self, bytecode_path: &Path) -> Result<(), ExecutorError> {
        let json = std::fs::read_to_string(bytecode_path)?;
        let bytecode: CompiledBytecode = serde_json::from_str(&json)?;

        self.load_blueprint(bytecode)
    }

    /// Load a compiled blueprint from bytecode structure.
    pub fn load_blueprint(&mut self, bytecode: CompiledBytecode) -> Result<(), ExecutorError> {
        let class_name = bytecode.source_class.clone();

        // Patch function pointers for each event program. Component ops are
        // bound to the live trampolines; std nodes resolve from pulsar_std.
        let mut programs = HashMap::new();

        for (event_name, mut program) in bytecode.event_programs.clone() {
            self.native_executor
                .prepare_with_component_ops(&mut program, component_op_handlers())
                .map_err(ExecutorError::Prepare)?;

            programs.insert(event_name, program);
        }

        // Create loaded blueprint
        let loaded = Arc::new(LoadedBlueprint {
            class_name: class_name.clone(),
            bytecode,
            programs,
        });

        self.loaded_blueprints.insert(class_name.clone(), loaded);

        tracing::debug!("Loaded blueprint: {}", class_name);

        Ok(())
    }

    /// Reload a blueprint with new bytecode (for hot-reload).
    ///
    /// This replaces the existing loaded blueprint with new bytecode,
    /// preserving the class name. Existing instances keep running against
    /// the swapped programs on their next event — identities and bindings
    /// are untouched, which is what makes PIE recompile hot-swap logic
    /// without respawning entities.
    ///
    /// Prefer [`BlueprintDispatcher::reload_blueprint`] (same module tree)
    /// as the entry point: it additionally rebuilds each affected instance's
    /// state arena so a changed variable layout can never alias old bytes.
    pub fn reload_blueprint(&mut self, bytecode: CompiledBytecode) -> Result<(), ExecutorError> {
        let class_name = bytecode.source_class.clone();

        if !self.loaded_blueprints.contains_key(&class_name) {
            return Err(ExecutorError::BlueprintNotLoaded(class_name));
        }

        // Remove old version
        self.loaded_blueprints.remove(&class_name);

        // Load new version
        self.load_blueprint(bytecode)
    }

    /// Check if a blueprint is loaded.
    pub fn is_loaded(&self, class_name: &str) -> bool {
        self.loaded_blueprints.contains_key(class_name)
    }

    /// Get a loaded blueprint by class name.
    pub fn get_loaded_blueprint(&self, class_name: &str) -> Option<Arc<LoadedBlueprint>> {
        self.loaded_blueprints.get(class_name).cloned()
    }

    /// Execute an event on a blueprint instance.
    ///
    /// # Arguments
    /// * `class_name` - The blueprint class name
    /// * `event_name` - The event to execute (e.g., "begin_play", "tick")
    /// * `arena` - The instance state arena (persistent across events)
    ///
    /// # Returns
    /// Ok(()) if execution succeeded, Err otherwise.
    pub fn execute_event(
        &self,
        class_name: &str,
        event_name: &str,
        arena: &mut ByteArena,
    ) -> Result<(), ExecutorError> {
        self.execute_event_in_world(class_name, event_name, arena, None)
    }

    /// Execute an event with an optional world context for component ops.
    ///
    /// `Some(context)` lets `comp_*` nodes read and mutate the authoritative
    /// SceneDB state for the instance's bound entity (see
    /// [`super::component_ops`]); `None` keeps the pre-component behaviour,
    /// where component ops log errors instead of touching the world.
    pub fn execute_event_in_world(
        &self,
        class_name: &str,
        event_name: &str,
        arena: &mut ByteArena,
        context: Option<EventWorld<'_>>,
    ) -> Result<(), ExecutorError> {
        // Get loaded blueprint
        let blueprint = self
            .loaded_blueprints
            .get(class_name)
            .ok_or_else(|| ExecutorError::BlueprintNotLoaded(class_name.to_string()))?;

        // Get event program
        let program = blueprint.programs.get(event_name).ok_or_else(|| {
            ExecutorError::Execution(format!(
                "Event '{}' not found in blueprint '{}'",
                event_name, class_name
            ))
        })?;

        // Execute bytecode
        unsafe {
            let result = match context {
                Some(EventWorld { world, entity }) => {
                    run_with_component_context(world, entity, || {
                        vm::run_with_external_arena(program, arena.as_mut_ptr(), arena.size())
                    })
                }
                None => vm::run_with_external_arena(program, arena.as_mut_ptr(), arena.size()),
            };
            result.map_err(|e| ExecutorError::Execution(format!("{:?}", e)))?;
        }

        Ok(())
    }

    /// List all loaded blueprint class names.
    pub fn loaded_class_names(&self) -> Vec<&str> {
        self.loaded_blueprints.keys().map(|s| s.as_str()).collect()
    }

    /// Unload a blueprint.
    pub fn unload_blueprint(&mut self, class_name: &str) -> bool {
        self.loaded_blueprints.remove(class_name).is_some()
    }

    /// Unload all blueprints.
    pub fn unload_all(&mut self) {
        self.loaded_blueprints.clear();
    }
}

impl Drop for BlueprintExecutor {
    fn drop(&mut self) {
        tracing::debug!(
            "Dropping BlueprintExecutor with {} loaded blueprints",
            self.loaded_blueprints.len()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_executor_creation() {
        let result = BlueprintExecutor::new();
        assert!(
            result.is_ok(),
            "Failed to create executor: {:?}",
            result.err()
        );

        let executor = result.unwrap();
        assert_eq!(executor.loaded_blueprints.len(), 0);
    }

    #[test]
    fn test_load_blueprint() {
        let mut executor = BlueprintExecutor::new().unwrap();

        let bytecode = CompiledBytecode::new("TestBlueprint");

        let result = executor.load_blueprint(bytecode);
        assert!(
            result.is_ok(),
            "Failed to load blueprint: {:?}",
            result.err()
        );

        assert!(executor.is_loaded("TestBlueprint"));
        assert_eq!(executor.loaded_class_names(), vec!["TestBlueprint"]);
    }

    #[test]
    fn test_unload_blueprint() {
        let mut executor = BlueprintExecutor::new().unwrap();

        let bytecode = CompiledBytecode::new("TestBlueprint");
        executor.load_blueprint(bytecode).unwrap();

        assert!(executor.unload_blueprint("TestBlueprint"));
        assert!(!executor.is_loaded("TestBlueprint"));
        assert!(!executor.unload_blueprint("TestBlueprint")); // Already unloaded
    }

    #[test]
    fn test_unload_all() {
        let mut executor = BlueprintExecutor::new().unwrap();

        executor
            .load_blueprint(CompiledBytecode::new("BP1"))
            .unwrap();
        executor
            .load_blueprint(CompiledBytecode::new("BP2"))
            .unwrap();

        assert_eq!(executor.loaded_blueprints.len(), 2);

        executor.unload_all();

        assert_eq!(executor.loaded_blueprints.len(), 0);
    }
}
