//! Blueprint event dispatcher and instance registry.
//!
//! Keeps runtime instances and routes lifecycle/tick events to the executor.
//! Since #648 every instance can be bound to a real scene [`Entity`]:
//! lifecycle events dispatch per bound instance against the shared world,
//! multiple instances of one class coexist (each with its own arena), and
//! gameplay code spawns/binds/unbinds instances at runtime.

use super::executor::{BlueprintExecutor, EventWorld, ExecutorError};
use super::instance::BlueprintInstance;
use super::CompiledBytecode;
use pulsar_scenedb::{Entity, World};
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
    /// Runtime instances keyed by object id. Multiple instances of one
    /// class coexist: each entry owns an independent state arena and its
    /// own entity binding (#648).
    instances: HashMap<String, BlueprintInstance>,
    execution_mode: ExecutionMode,
    /// Object IDs registered but not yet given their `begin_play`.
    ///
    /// Instances are queued here rather than dispatched immediately by
    /// registration because registration happens during level setup —
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
    ///
    /// The instance starts unbound; see [`Self::spawn_instance_for_entity`]
    /// for the bound variant and [`Self::bind_instance`] for late binding.
    pub fn register_instance(
        &mut self,
        object_id: String,
        bytecode_path: &Path,
        variable_overrides: Option<HashMap<String, JsonValue>>,
    ) -> Result<(), ExecutorError> {
        let bytecode = Self::read_bytecode(bytecode_path)?;
        self.register_bytecode(object_id, bytecode, None, variable_overrides)
    }

    /// Spawn an instance bound to a live scene entity in one step (#648).
    ///
    /// This is the gameplay-driven spawning API: register from a compiled
    /// bytecode file AND bind to `entity` atomically, so the instance's
    /// first dispatched event (its queued `begin_play`) already addresses
    /// component ops at `entity`. Multiple instances of the same class each
    /// get their own state arena; give every spawn a distinct `object_id`.
    pub fn spawn_instance_for_entity(
        &mut self,
        object_id: String,
        bytecode_path: &Path,
        entity: Entity,
        variable_overrides: Option<HashMap<String, JsonValue>>,
    ) -> Result<(), ExecutorError> {
        let bytecode = Self::read_bytecode(bytecode_path)?;
        self.register_bytecode(object_id, bytecode, Some(entity), variable_overrides)
    }

    /// In-memory registration core shared by all entry points.
    ///
    /// Class programs are shared across instances: if the class is already
    /// loaded it is NOT re-prepared, so spawning N enemies of one class pays
    /// preparation once. Deliberate implementation swaps go through
    /// [`Self::reload_blueprint`].
    pub fn register_bytecode(
        &mut self,
        object_id: String,
        bytecode: CompiledBytecode,
        entity: Option<Entity>,
        variable_overrides: Option<HashMap<String, JsonValue>>,
    ) -> Result<(), ExecutorError> {
        let class_name = bytecode.source_class.clone();

        if !self.executor.is_loaded(&class_name) {
            self.executor.load_blueprint(bytecode)?;
        }

        let loaded = self
            .executor
            .get_loaded_blueprint(&class_name)
            .ok_or_else(|| ExecutorError::BlueprintNotLoaded(class_name.clone()))?;

        let mut instance =
            BlueprintInstance::new_bytecode(object_id.clone(), &loaded, variable_overrides);
        if let Some(entity) = entity {
            instance.bind_entity(entity);
        }
        self.instances.insert(object_id.clone(), instance);
        tracing::info!(class = %class_name, "Queued '{object_id}' for deferred begin_play");
        self.pending_begin_play.push(object_id);
        Ok(())
    }

    fn read_bytecode(bytecode_path: &Path) -> Result<CompiledBytecode, ExecutorError> {
        let json = std::fs::read_to_string(bytecode_path)?;
        serde_json::from_str(&json).map_err(ExecutorError::from)
    }

    /// Bind a registered instance to a scene entity at runtime (#648).
    ///
    /// Late-binding path for instances created before their entities existed
    /// (level hydration lands after `setup()`); also how a surviving
    /// instance is re-attached after a respawn. Binding is validated only
    /// lazily — component ops refuse through liveness checks if the entity
    /// is stale by the time an event runs.
    pub fn bind_instance(&mut self, object_id: &str, entity: Entity) -> Result<(), ExecutorError> {
        let instance = self
            .instances
            .get_mut(object_id)
            .ok_or_else(|| ExecutorError::InstanceNotRegistered(object_id.to_string()))?;
        if instance
            .bind_entity(entity)
            .is_some_and(|prev| prev != entity)
        {
            tracing::info!("Rebound '{object_id}' to a new entity");
        }
        Ok(())
    }

    /// Detach a registered instance from its scene entity (#648).
    ///
    /// Returns the removed binding, or `None` when the instance is unknown
    /// or was never bound. The instance keeps ticking its graph; only
    /// component ops refuse until it is bound again.
    pub fn unbind_instance(&mut self, object_id: &str) -> Option<Entity> {
        self.instances.get_mut(object_id)?.unbind_entity()
    }

    /// The scene entity an instance is bound to, if registered and bound.
    pub fn instance_entity(&self, object_id: &str) -> Option<Entity> {
        self.instances.get(object_id).and_then(|i| i.bound_entity())
    }

    /// Read one instance variable's raw bytes (tooling/debug seam).
    ///
    /// Exposed for hosts and tests that verify per-instance state isolation;
    /// game logic should read variables inside graphs instead.
    pub fn instance_variable_bytes(&self, object_id: &str, var_name: &str) -> Option<Vec<u8>> {
        let instance = self.instances.get(object_id)?;
        let bytes = instance.get_variable_bytes(var_name)?;
        Some(bytes)
    }

    /// Hot-swap a loaded class' programs with fresh bytecode (#648).
    ///
    /// The PIE-recompile entry point: the editor compiles a new version of a
    /// running class and hands it here. Programs swap immediately; every
    /// instance of the class keeps its identity, entity binding, and any
    /// variables whose layout is unchanged (`BlueprintInstance::
    /// rehydrate_after_reload` rebuilds arenas exact-match-only), so a
    /// recompile changes behaviour without respawning entities. Fails with
    /// [`ExecutorError::BlueprintNotLoaded`] if the class was never loaded.
    pub fn reload_blueprint(&mut self, bytecode: CompiledBytecode) -> Result<(), ExecutorError> {
        let class_name = bytecode.source_class.clone();
        self.executor.reload_blueprint(bytecode)?;

        let loaded = self
            .executor
            .get_loaded_blueprint(&class_name)
            .ok_or_else(|| ExecutorError::BlueprintNotLoaded(class_name.clone()))?;

        let mut rebuilt = 0usize;
        for instance in self.instances.values_mut() {
            if instance.class_name == class_name {
                instance.rehydrate_after_reload(&loaded);
                rebuilt += 1;
            }
        }
        tracing::info!(
            class = %class_name,
            instances = rebuilt,
            "Hot-reloaded blueprint; instance state preserved where layout matched"
        );
        Ok(())
    }

    pub fn unregister_instance(&mut self, object_id: &str) -> Option<BlueprintInstance> {
        self.pending_begin_play.retain(|id| id != object_id);
        self.instances.remove(object_id)
    }

    /// Dispatches `begin_play` to every instance registered since the last
    /// call, then clears the queue. Safe to call every tick — it's a no-op
    /// once the queue is empty.
    ///
    /// `world` backs component ops for the dispatched events. Instances
    /// bound at spawn time address their own entity; instances registered
    /// unbound (level wiring lands later, #650) run their graphs with
    /// component ops refusing until [`Self::bind_instance`] is called.
    pub fn dispatch_pending_begin_play(&mut self, world: &mut World) {
        if self.pending_begin_play.is_empty() {
            return;
        }
        let pending = std::mem::take(&mut self.pending_begin_play);
        tracing::info!(
            "Dispatching begin_play to {} VM blueprint instance(s)",
            pending.len()
        );
        for object_id in pending {
            match self.execute_event(&object_id, "begin_play", world) {
                Ok(()) => {
                    tracing::info!("begin_play executed for VM blueprint instance '{object_id}'")
                }
                Err(e) => {
                    tracing::warn!("begin_play failed for VM blueprint instance '{object_id}': {e}")
                }
            }
        }
    }

    /// Dispatches `tick` to every registered instance against the shared
    /// world (#648).
    ///
    /// Each bound instance runs its graph on its OWN state arena with
    /// component ops addressed at its OWN entity — two instances of one
    /// class never see each other's variables or components. Called by the
    /// tick loop's blueprint phase while it holds the store write lock.
    pub fn dispatch_tick_all(&mut self, world: &mut World, delta_time: f32) {
        let object_ids = self.instance_ids();
        for object_id in object_ids {
            if let Err(e) = self.execute_tick(&object_id, world, delta_time) {
                tracing::warn!("tick failed for VM blueprint instance '{object_id}': {e}");
            }
        }
    }

    /// Dispatches `end_play` to every currently-registered instance.
    ///
    /// Called once as the tick loop shuts down so blueprints can release
    /// resources and run teardown logic, mirroring `ActorRegistry`'s
    /// lifecycle contract.
    pub fn dispatch_end_play_all(&mut self, world: &mut World) {
        let object_ids = self.instance_ids();
        for object_id in object_ids {
            if let Err(e) = self.execute_event(&object_id, "end_play", world) {
                tracing::warn!("end_play failed for VM blueprint instance '{object_id}': {e}");
            }
        }
    }

    pub fn dispatch_event(
        &mut self,
        event: BlueprintEvent,
        world: &mut World,
    ) -> Result<(), ExecutorError> {
        match event {
            BlueprintEvent::BeginPlay { object_id } => {
                self.execute_event(&object_id, "begin_play", world)
            }
            BlueprintEvent::Tick {
                object_id,
                delta_time,
            } => self.execute_tick(&object_id, world, delta_time),
            BlueprintEvent::EndPlay { object_id } => {
                self.execute_event(&object_id, "end_play", world)
            }
        }
    }

    /// Run one event for one instance with its binding resolved into an
    /// [`EventWorld`] context (#648).
    fn execute_event(
        &mut self,
        object_id: &str,
        event_name: &str,
        world: &mut World,
    ) -> Result<(), ExecutorError> {
        let instance = self
            .instances
            .get_mut(object_id)
            .ok_or_else(|| ExecutorError::InstanceNotRegistered(object_id.to_string()))?;
        let class_name = instance.class_name.clone();
        let bound_entity = instance.bound_entity();

        let arena = instance
            .state_arena_mut()
            .ok_or_else(|| ExecutorError::Execution("No arena in bytecode instance".to_string()))?;

        let context = match bound_entity {
            Some(entity) => EventWorld::bound(world, entity),
            None => EventWorld::unbound(world),
        };

        self.executor
            .execute_event_in_world(&class_name, event_name, arena, Some(context))
    }

    /// `execute_event` for ticks — same resolution, `delta_time` reserved
    /// for the graph-level time input seam (not yet wired into programs).
    fn execute_tick(
        &mut self,
        object_id: &str,
        world: &mut World,
        _delta_time: f32,
    ) -> Result<(), ExecutorError> {
        self.execute_event(object_id, "tick", world)
    }
}
