use std::collections::{HashMap, VecDeque};

use crate::{Subsystem, SubsystemContext, SubsystemError, SubsystemId};

/// Registry for managing subsystems with dependency resolution.
///
/// All subsystems are stored as `Box<dyn Subsystem>` — fully type-erased.
/// Consumers downcast via `Any` to get their concrete type back.
///
/// # Ownership model
///
/// The registry **owns** every subsystem. Callers that need access borrow
/// through `get()` / `get_mut()` and downcast. At shutdown, the registry
/// calls `shutdown_all()`, which drains subsystems in reverse init order.
///
/// # First-registered-wins policy
///
/// When merging registries via [`merge`](SubsystemRegistry::merge) or
/// registering via [`register_boxed`](SubsystemRegistry::register_boxed),
/// if a subsystem with the same ID already exists, the first one wins.
/// This lets built-in subsystems take priority over plugin-provided ones
/// (see `EngineBackend::inject_plugin_subsystems`).
///
/// # Usage in the engine
///
/// - **`EngineBackend`** (`engine_backend/src/lib.rs`) — owns the primary
///   `SubsystemRegistry`. Built-in subsystems are registered first via
///   `register_boxed`, then plugin subsystems are merged in. The engine
///   calls `init_all()` once, then `update_all()` per frame, and finally
///   `shutdown_all()` on exit.
///
/// - **`PluginManager`** (`plugin_manager/src/lib.rs`) — collects
///   `Box<dyn Subsystem>` from loaded plugin DLLs via `drain_subsystems()`
///   and hands them to `EngineBackend::inject_plugin_subsystems`.
///
/// - **`plugin_editor_api`** (`plugin_editor_api/src/subsystems.rs`) —
///   defines the `EditorPluginSubsystems` trait that plugins implement to
///   provide their subsystems to the engine.
///
/// # Downcast pattern
///
/// ```rust,ignore
/// use std::any::Any;
///
/// fn get_concrete(registry: &SubsystemRegistry) -> Option<&ConcreteSubsystem> {
///     let ss = registry.get(MY_ID)?;
///     let any: &dyn Any = ss;
///     any.downcast_ref::<ConcreteSubsystem>()
/// }
/// ```
pub struct SubsystemRegistry {
    subsystems: HashMap<SubsystemId, Box<dyn Subsystem>>,
    init_order: Vec<SubsystemId>,
    initialized: bool,
}

impl SubsystemRegistry {
    /// Create a new empty registry.
    pub fn new() -> Self {
        Self {
            subsystems: HashMap::new(),
            init_order: Vec::new(),
            initialized: false,
        }
    }

    /// Register a subsystem (must be done before `init_all`).
    ///
    /// Returns `Err(AlreadyRegistered)` if a subsystem with the same ID
    /// already exists.
    pub fn register<S: Subsystem + 'static>(&mut self, subsystem: S) -> Result<(), SubsystemError> {
        let id = subsystem.id();
        if self.subsystems.contains_key(&id) {
            return Err(SubsystemError::AlreadyRegistered(id.as_str()));
        }
        self.subsystems.insert(id, Box::new(subsystem));
        Ok(())
    }

    /// Register a subsystem that is already boxed (e.g., from a plugin).
    ///
    /// Returns `Err(AlreadyRegistered)` if a subsystem with the same ID
    /// already exists. Used by `EngineBackend::inject_plugin_subsystems`.
    pub fn register_boxed(&mut self, subsystem: Box<dyn Subsystem>) -> Result<(), SubsystemError> {
        let id = subsystem.id();
        if self.subsystems.contains_key(&id) {
            return Err(SubsystemError::AlreadyRegistered(id.as_str()));
        }
        self.subsystems.insert(id, subsystem);
        Ok(())
    }

    /// Merge all subsystems from another registry into this one.
    ///
    /// Silently skips IDs that already exist (first-registered wins).
    /// Used when plugin subsystems are merged into the engine's main
    /// registry.
    pub fn merge(&mut self, other: SubsystemRegistry) {
        for (id, subsystem) in other.subsystems {
            if !self.subsystems.contains_key(&id) {
                self.subsystems.insert(id, subsystem);
            }
        }
    }

    /// Resolve dependencies using topological sort (Kahn's algorithm).
    ///
    /// Returns subsystem IDs in initialization order. Errors:
    /// - `MissingDependency` — a declared dependency was never registered
    /// - `DependencyCycle` — the dependency graph contains a cycle
    pub fn resolve_dependencies(&self) -> Result<Vec<SubsystemId>, SubsystemError> {
        let mut in_degree: HashMap<SubsystemId, usize> = HashMap::new();
        let mut adjacency: HashMap<SubsystemId, Vec<SubsystemId>> = HashMap::new();

        for id in self.subsystems.keys() {
            in_degree.insert(*id, 0);
            adjacency.insert(*id, Vec::new());
        }

        for (id, subsystem) in &self.subsystems {
            let deps = subsystem.dependencies();

            for dep in &deps {
                if !self.subsystems.contains_key(dep) {
                    return Err(SubsystemError::MissingDependency {
                        subsystem: id.as_str(),
                        dependency: dep.as_str(),
                    });
                }
            }

            *in_degree.get_mut(id).unwrap() += deps.len();

            for dep in deps {
                adjacency.get_mut(&dep).unwrap().push(*id);
            }
        }

        let mut queue: VecDeque<SubsystemId> = in_degree
            .iter()
            .filter(|(_, &degree)| degree == 0)
            .map(|(&id, _)| id)
            .collect();

        let mut order = Vec::new();

        while let Some(id) = queue.pop_front() {
            order.push(id);

            if let Some(dependents) = adjacency.get(&id) {
                for &dependent in dependents {
                    let degree = in_degree.get_mut(&dependent).unwrap();
                    *degree -= 1;
                    if *degree == 0 {
                        queue.push_back(dependent);
                    }
                }
            }
        }

        if order.len() != self.subsystems.len() {
            let unprocessed: Vec<&'static str> = self
                .subsystems
                .keys()
                .filter(|id| !order.contains(id))
                .map(|id| id.as_str())
                .collect();

            return Err(SubsystemError::DependencyCycle {
                subsystems: unprocessed,
            });
        }

        Ok(order)
    }

    /// Initialize all subsystems in dependency order.
    ///
    /// Idempotent — subsequent calls are no-ops (logged at `warn` level).
    /// Each subsystem's `init()` is called with the provided context.
    pub fn init_all(&mut self, context: &SubsystemContext) -> Result<(), SubsystemError> {
        if self.initialized {
            tracing::warn!("SubsystemRegistry already initialized, skipping");
            return Ok(());
        }

        let order = self.resolve_dependencies()?;
        tracing::info!(
            "Subsystem initialization order: {:?}",
            order.iter().map(|id| id.as_str()).collect::<Vec<_>>()
        );

        for id in &order {
            let subsystem = self.subsystems.get_mut(id).unwrap();
            tracing::debug!("Initializing subsystem: {}", id.as_str());
            subsystem
                .init(context)
                .map_err(|e| SubsystemError::InitFailed(format!("{}: {}", id.as_str(), e)))?;
        }

        self.init_order = order;
        self.initialized = true;

        tracing::info!("All subsystems initialized successfully");
        Ok(())
    }

    /// Shutdown all subsystems in reverse initialization order.
    ///
    /// Idempotent — safe to call multiple times. After shutdown, the
    /// registry can be re-initialized with `init_all()`.
    pub fn shutdown_all(&mut self) -> Result<(), SubsystemError> {
        if !self.initialized {
            return Ok(());
        }

        for id in self.init_order.iter().rev() {
            if let Some(subsystem) = self.subsystems.get_mut(id) {
                tracing::debug!("Shutting down subsystem: {}", id.as_str());
                subsystem.shutdown().map_err(|e| {
                    SubsystemError::ShutdownFailed(format!("{}: {}", id.as_str(), e))
                })?;
            }
        }

        self.initialized = false;
        tracing::info!("All subsystems shut down successfully");
        Ok(())
    }

    /// Call `on_frame` for all initialized subsystems, in init order.
    ///
    /// Called every tick by the engine main loop. No-op if not initialized.
    pub fn update_all(&mut self, delta_time: f32) {
        if !self.initialized {
            return;
        }

        for id in &self.init_order {
            if let Some(subsystem) = self.subsystems.get_mut(id) {
                subsystem.on_frame(delta_time);
            }
        }
    }

    /// Get a reference to a registered subsystem by ID.
    ///
    /// Returns `None` if no subsystem with that ID is registered.
    /// Downcast to the concrete type via `Any`.
    pub fn get(&self, id: SubsystemId) -> Option<&dyn Subsystem> {
        self.subsystems.get(&id).map(|b| &**b as &dyn Subsystem)
    }

    /// Get a mutable reference to a registered subsystem by ID.
    pub fn get_mut(&mut self, id: SubsystemId) -> Option<&mut dyn Subsystem> {
        self.subsystems
            .get_mut(&id)
            .map(|b| &mut **b as &mut dyn Subsystem)
    }

    /// Check if a subsystem is registered.
    pub fn contains(&self, id: SubsystemId) -> bool {
        self.subsystems.contains_key(&id)
    }

    /// Number of registered subsystems.
    pub fn len(&self) -> usize {
        self.subsystems.len()
    }

    /// True if no subsystems are registered.
    pub fn is_empty(&self) -> bool {
        self.subsystems.is_empty()
    }

    /// Iterate over all registered subsystem IDs.
    pub fn ids(&self) -> impl Iterator<Item = &SubsystemId> {
        self.subsystems.keys()
    }

    /// Whether `init_all` has been called successfully.
    pub fn is_initialized(&self) -> bool {
        self.initialized
    }
}

impl Default for SubsystemRegistry {
    fn default() -> Self {
        Self::new()
    }
}
