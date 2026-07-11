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
    /// Object IDs registered but not yet given their `begin_play`.
    ///
    /// Instances are queued here rather than dispatched immediately by
    /// `register_instance` because registration happens during level setup —
    /// before the window, GPU surface, and scene are ready. The `TickLoop`
    /// drains this queue on its first tick (after `spawn_ecs_thread`, which
    /// only runs once the primary window is open), so `begin_play` observes a
    /// fully-initialised world, matching native-actor lifecycle ordering.
    pending_begin_play: Vec<String>,
}

impl BlueprintDispatcher {
    pub fn new() -> Result<Self, ExecutorError> {
        Ok(Self {
            executor: BlueprintExecutor::new()?,
            instances: HashMap::new(),
            execution_mode: ExecutionMode::Bytecode,
            pending_begin_play: Vec::new(),
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
        self.instances.insert(object_id.clone(), instance);
        tracing::info!("Queued '{object_id}' for deferred begin_play");
        self.pending_begin_play.push(object_id);
        Ok(())
    }

    pub fn unregister_instance(&mut self, object_id: &str) -> Option<BlueprintInstance> {
        self.pending_begin_play.retain(|id| id != object_id);
        self.instances.remove(object_id)
    }

    /// Dispatches `begin_play` to every instance registered since the last
    /// call, then clears the queue. Safe to call every tick — it's a no-op
    /// once the queue is empty.
    pub fn dispatch_pending_begin_play(&mut self) {
        if self.pending_begin_play.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending_begin_play);
        tracing::info!(
            "Dispatching begin_play to {} VM blueprint instance(s)",
            pending.len()
        );
        for object_id in pending {
            match self.execute_event(&object_id, "begin_play") {
                Ok(()) => {
                    tracing::info!("begin_play executed for VM blueprint instance '{object_id}'")
                }
                Err(e) => {
                    tracing::warn!("begin_play failed for VM blueprint instance '{object_id}': {e}")
                }
            }
        }
    }

    /// Dispatches `end_play` to every currently-registered instance.
    ///
    /// Called once as the tick loop shuts down so blueprints can release
    /// resources and run teardown logic, mirroring `ActorRegistry`'s
    /// lifecycle contract.
    pub fn dispatch_end_play_all(&mut self) {
        let object_ids = self.instance_ids();
        for object_id in object_ids {
            if let Err(e) = self.execute_event(&object_id, "end_play") {
                tracing::warn!("end_play failed for VM blueprint instance '{object_id}': {e}");
            }
        }
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
