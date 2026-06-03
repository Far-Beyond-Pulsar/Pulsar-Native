//! Blueprint event dispatcher and instance registry.
//!
//! Keeps runtime instances and routes lifecycle/tick events to the executor.

use super::executor::{BlueprintExecutor, ExecutorError};
use super::instance::BlueprintInstance;
use super::CompiledBytecode;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::path::Path;

/// Runtime execution mode used by the dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionMode {
    Bytecode,
    Native,
}

/// Blueprint lifecycle events.
#[derive(Debug, Clone)]
pub enum BlueprintEvent {
    BeginPlay { object_id: String },
    Tick { object_id: String, delta_time: f32 },
    EndPlay { object_id: String },
}

/// Owns the blueprint executor and per-object runtime instances.
pub struct BlueprintDispatcher {
    executor: BlueprintExecutor,
    instances: HashMap<String, BlueprintInstance>,
    execution_mode: ExecutionMode,
}

impl BlueprintDispatcher {
    pub fn new() -> Result<Self, ExecutorError> {
        Ok(Self {
            executor: BlueprintExecutor::new()?,
            instances: HashMap::new(),
            execution_mode: ExecutionMode::Bytecode,
        })
    }

    pub fn execution_mode(&self) -> ExecutionMode {
        self.execution_mode
    }

    pub fn set_execution_mode(&mut self, mode: ExecutionMode) {
        self.execution_mode = mode;
    }

    /// Returns a snapshot of all registered object IDs.
    pub fn instance_ids(&self) -> Vec<String> {
        self.instances.keys().cloned().collect()
    }

    /// Register a scene object instance from a compiled bytecode file.
    pub fn register_instance(
        &mut self,
        object_id: String,
        bytecode_path: &Path,
        variable_overrides: Option<HashMap<String, JsonValue>>,
    ) -> Result<(), ExecutorError> {
        let json = std::fs::read_to_string(bytecode_path)?;
        let bytecode: CompiledBytecode = serde_json::from_str(&json)?;
        let class_name = bytecode.source_class.clone();

        self.executor.load_blueprint(bytecode)?;

        let loaded = self
            .executor
            .get_loaded_blueprint(&class_name)
            .ok_or_else(|| ExecutorError::BlueprintNotLoaded(class_name.clone()))?;

        let instance =
            BlueprintInstance::new_bytecode(object_id.clone(), &loaded, variable_overrides);
        self.instances.insert(object_id, instance);
        Ok(())
    }

    pub fn unregister_instance(&mut self, object_id: &str) -> Option<BlueprintInstance> {
        self.instances.remove(object_id)
    }

    pub fn dispatch_event(&mut self, event: BlueprintEvent) -> Result<(), ExecutorError> {
        match event {
            BlueprintEvent::BeginPlay { object_id } => self.execute_event(&object_id, "begin_play"),
            BlueprintEvent::Tick {
                object_id,
                delta_time,
            } => {
                let _ = delta_time;
                self.execute_event(&object_id, "tick")
            }
            BlueprintEvent::EndPlay { object_id } => self.execute_event(&object_id, "end_play"),
        }
    }

    fn execute_event(&mut self, object_id: &str, event_name: &str) -> Result<(), ExecutorError> {
        let instance = self.instances.get_mut(object_id).ok_or_else(|| {
            ExecutorError::Execution(format!("Blueprint instance not found: {}", object_id))
        })?;
        let class_name = instance.class_name.clone();

        let arena = instance
            .state_arena_mut()
            .ok_or_else(|| ExecutorError::Execution("No arena in bytecode instance".to_string()))?;

        self.executor.execute_event(&class_name, event_name, arena)
    }
}
