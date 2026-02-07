//! Subsystem Framework
//!
//! Provides a trait-based framework for engine subsystems with:
//! - Dependency declaration and resolution (topological sort)
//! - Lifecycle management (init, shutdown, per-frame updates)
//! - Type-safe subsystem registration
//! - Shared context for runtime handles and configuration

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use dashmap::DashMap;
use tokio::runtime::Handle;
use thiserror::Error;

/// Unique identifier for a subsystem
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SubsystemId(&'static str);

impl SubsystemId {
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

/// Common subsystem identifiers
pub mod subsystem_ids {
    use super::SubsystemId;

    pub const PHYSICS: SubsystemId = SubsystemId::new("physics");
    pub const AUDIO: SubsystemId = SubsystemId::new("audio");
    pub const INPUT: SubsystemId = SubsystemId::new("input");
    pub const NETWORKING: SubsystemId = SubsystemId::new("networking");
    pub const SCRIPTING: SubsystemId = SubsystemId::new("scripting");
    pub const RENDERING: SubsystemId = SubsystemId::new("rendering");
}

/// Shared context provided to all subsystems during initialization
#[derive(Clone)]
pub struct SubsystemContext {
    /// Tokio runtime handle for spawning async tasks
    pub runtime: Handle,
    /// Shared channel registry for inter-subsystem communication
    pub channels: Arc<ChannelRegistry>,
}

impl SubsystemContext {
    pub fn new(runtime: Handle) -> Self {
        Self {
            runtime,
            channels: Arc::new(ChannelRegistry::new()),
        }
    }
}

/// Registry for typed channels between subsystems
pub struct ChannelRegistry {
    channels: DashMap<String, Arc<dyn std::any::Any + Send + Sync>>,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self {
            channels: DashMap::new(),
        }
    }

    /// Register a channel sender
    pub fn register<T: Send + Sync + 'static>(&self, name: String, sender: T) {
        self.channels.insert(name, Arc::new(sender));
    }

    /// Get a channel sender
    pub fn get<T: Send + Sync + 'static>(&self, name: &str) -> Option<Arc<T>> {
        self.channels
            .get(name)
            .and_then(|entry| entry.value().clone().downcast::<T>().ok())
    }
}

/// Errors that can occur during subsystem operations
#[derive(Debug, thiserror::Error)]
pub enum SubsystemError {
    #[error("Subsystem initialization failed: {0}")]
    InitFailed(String),

    #[error("Subsystem shutdown failed: {0}")]
    ShutdownFailed(String),

    #[error("Dependency cycle detected involving: {subsystems:?}")]
    DependencyCycle { subsystems: Vec<&'static str> },

    #[error("Missing dependency: {dependency} required by {subsystem}")]
    MissingDependency {
        subsystem: &'static str,
        dependency: &'static str,
    },

    #[error("Subsystem already registered: {0}")]
    AlreadyRegistered(&'static str),
}

/// Trait that all engine subsystems must implement
///
/// Subsystems are independent engine components that can declare dependencies
/// on other subsystems and participate in lifecycle management.
///
/// # Example
///
/// ```ignore
/// use engine_backend::subsystems::framework::*;
///
/// struct PhysicsSubsystem {
///     engine: Option<PhysicsEngine>,
/// }
///
/// impl Subsystem for PhysicsSubsystem {
///     fn id(&self) -> SubsystemId {
///         subsystem_ids::PHYSICS
///     }
///
///     fn dependencies(&self) -> Vec<SubsystemId> {
///         vec![] // No dependencies
///     }
///
///     fn init(&mut self, context: &SubsystemContext) -> Result<(), SubsystemError> {
///         let engine = PhysicsEngine::new();
///         context.runtime.spawn(async move {
///             // Physics loop
///         });
///         self.engine = Some(engine);
///         Ok(())
///     }
///
///     fn shutdown(&mut self) -> Result<(), SubsystemError> {
///         self.engine = None;
///         Ok(())
///     }
/// }
/// ```
pub trait Subsystem: Send + Sync {
    /// Unique identifier for this subsystem
    fn id(&self) -> SubsystemId;

    /// List of subsystems this one depends on (must be initialized first)
    fn dependencies(&self) -> Vec<SubsystemId>;

    /// Initialize the subsystem with the provided context
    fn init(&mut self, context: &SubsystemContext) -> Result<(), SubsystemError>;

    /// Shutdown the subsystem (cleanup resources)
    fn shutdown(&mut self) -> Result<(), SubsystemError>;

    /// Optional per-frame update (default: no-op)
    fn on_frame(&mut self, _delta_time: f32) {}
}

/// Registry for managing subsystems with dependency resolution
pub struct SubsystemRegistry {
    subsystems: HashMap<SubsystemId, Box<dyn Subsystem>>,
    init_order: Vec<SubsystemId>,
    initialized: bool,
}

impl SubsystemRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self {
            subsystems: HashMap::new(),
            init_order: Vec::new(),
            initialized: false,
        }
    }

    /// Register a subsystem (must be done before init_all)
    pub fn register<S: Subsystem + 'static>(&mut self, subsystem: S) -> Result<(), SubsystemError> {
        let id = subsystem.id();

        if self.subsystems.contains_key(&id) {
            return Err(SubsystemError::AlreadyRegistered(id.as_str()));
        }

        self.subsystems.insert(id, Box::new(subsystem));
        Ok(())
    }

    /// Resolve dependencies using topological sort (Kahn's algorithm)
    ///
    /// Returns initialization order that respects all dependencies
    pub fn resolve_dependencies(&self) -> Result<Vec<SubsystemId>, SubsystemError> {
        // Build dependency graph
        let mut in_degree: HashMap<SubsystemId, usize> = HashMap::new();
        let mut adjacency: HashMap<SubsystemId, Vec<SubsystemId>> = HashMap::new();

        // Initialize all subsystems with in-degree 0
        for id in self.subsystems.keys() {
            in_degree.insert(*id, 0);
            adjacency.insert(*id, Vec::new());
        }

        // Build the graph: for each subsystem, add edges from dependencies to it
        for (id, subsystem) in &self.subsystems {
            let deps = subsystem.dependencies();

            // Validate all dependencies exist
            for dep in &deps {
                if !self.subsystems.contains_key(dep) {
                    return Err(SubsystemError::MissingDependency {
                        subsystem: id.as_str(),
                        dependency: dep.as_str(),
                    });
                }
            }

            // Increment in-degree for this subsystem (one per dependency)
            *in_degree.get_mut(id).unwrap() += deps.len();

            // Add edges from each dependency to this subsystem
            for dep in deps {
                adjacency.get_mut(&dep).unwrap().push(*id);
            }
        }

        // Kahn's algorithm: process nodes with in-degree 0
        let mut queue: VecDeque<SubsystemId> = in_degree
            .iter()
            .filter(|(_, &degree)| degree == 0)
            .map(|(&id, _)| id)
            .collect();

        let mut order = Vec::new();

        while let Some(id) = queue.pop_front() {
            order.push(id);

            // For each dependent of this subsystem
            if let Some(dependents) = adjacency.get(&id) {
                for &dependent in dependents {
                    // Decrease in-degree
                    let degree = in_degree.get_mut(&dependent).unwrap();
                    *degree -= 1;

                    // If in-degree becomes 0, add to queue
                    if *degree == 0 {
                        queue.push_back(dependent);
                    }
                }
            }
        }

        // If we didn't process all subsystems, there's a cycle
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

    /// Initialize all subsystems in dependency order
    pub fn init_all(&mut self, context: &SubsystemContext) -> Result<(), SubsystemError> {
        if self.initialized {
            tracing::warn!("SubsystemRegistry already initialized, skipping");
            return Ok(());
        }

        // Resolve initialization order
        let order = self.resolve_dependencies()?;
        tracing::info!("Subsystem initialization order: {:?}",
            order.iter().map(|id| id.as_str()).collect::<Vec<_>>());

        // Initialize in order
        for id in &order {
            let subsystem = self.subsystems.get_mut(id).unwrap();
            tracing::debug!("Initializing subsystem: {}", id.as_str());

            subsystem.init(context).map_err(|e| {
                SubsystemError::InitFailed(format!("{}: {}", id.as_str(), e))
            })?;
        }

        self.init_order = order;
        self.initialized = true;

        tracing::info!("All subsystems initialized successfully");
        Ok(())
    }

    /// Shutdown all subsystems in reverse initialization order
    pub fn shutdown_all(&mut self) -> Result<(), SubsystemError> {
        if !self.initialized {
            return Ok(());
        }

        // Shutdown in reverse order
        for id in self.init_order.iter().rev() {
            let subsystem = self.subsystems.get_mut(id).unwrap();
            tracing::debug!("Shutting down subsystem: {}", id.as_str());

            subsystem.shutdown().map_err(|e| {
                SubsystemError::ShutdownFailed(format!("{}: {}", id.as_str(), e))
            })?;
        }

        self.initialized = false;
        tracing::info!("All subsystems shut down successfully");
        Ok(())
    }

    /// Call on_frame for all initialized subsystems
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

    /// Get a subsystem by ID (for direct access if needed)
    pub fn get(&self, id: SubsystemId) -> Option<&dyn Subsystem> {
        self.subsystems.get(&id).map(|b| &**b)
    }

    /// Get a mutable subsystem by ID
    pub fn get_mut(&mut self, id: SubsystemId) -> Option<&mut (dyn Subsystem + '_)> {
        match self.subsystems.get_mut(&id) {
            Some(b) => Some(&mut **b),
            None => None,
        }
    }
}

impl Default for SubsystemRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct MockSubsystem {
        id: SubsystemId,
        deps: Vec<SubsystemId>,
        init_called: bool,
    }

    impl MockSubsystem {
        fn new(id: SubsystemId, deps: Vec<SubsystemId>) -> Self {
            Self {
                id,
                deps,
                init_called: false,
            }
        }
    }

    impl Subsystem for MockSubsystem {
        fn id(&self) -> SubsystemId {
            self.id
        }

        fn dependencies(&self) -> Vec<SubsystemId> {
            self.deps.clone()
        }

        fn init(&mut self, _context: &SubsystemContext) -> Result<(), SubsystemError> {
            self.init_called = true;
            Ok(())
        }

        fn shutdown(&mut self) -> Result<(), SubsystemError> {
            Ok(())
        }
    }

    #[test]
    fn test_simple_dependency_resolution() {
        let mut registry = SubsystemRegistry::new();

        let a = SubsystemId::new("a");
        let b = SubsystemId::new("b");
        let c = SubsystemId::new("c");

        // b depends on a, c depends on b
        registry.register(MockSubsystem::new(a, vec![])).unwrap();
        registry.register(MockSubsystem::new(b, vec![a])).unwrap();
        registry.register(MockSubsystem::new(c, vec![b])).unwrap();

        let order = registry.resolve_dependencies().unwrap();

        // a must come before b, b must come before c
        let a_pos = order.iter().position(|&id| id == a).unwrap();
        let b_pos = order.iter().position(|&id| id == b).unwrap();
        let c_pos = order.iter().position(|&id| id == c).unwrap();

        assert!(a_pos < b_pos);
        assert!(b_pos < c_pos);
    }

    #[test]
    fn test_cycle_detection() {
        let mut registry = SubsystemRegistry::new();

        let a = SubsystemId::new("a");
        let b = SubsystemId::new("b");

        // Create a cycle: a depends on b, b depends on a
        registry.register(MockSubsystem::new(a, vec![b])).unwrap();
        registry.register(MockSubsystem::new(b, vec![a])).unwrap();

        let result = registry.resolve_dependencies();
        assert!(matches!(result, Err(SubsystemError::DependencyCycle { .. })));
    }

    #[test]
    fn test_missing_dependency() {
        let mut registry = SubsystemRegistry::new();

        let a = SubsystemId::new("a");
        let b = SubsystemId::new("b");

        // b depends on a, but a is not registered
        registry.register(MockSubsystem::new(b, vec![a])).unwrap();

        let result = registry.resolve_dependencies();
        assert!(matches!(result, Err(SubsystemError::MissingDependency { .. })));
    }
}
