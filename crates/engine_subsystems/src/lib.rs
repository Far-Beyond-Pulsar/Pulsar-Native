//! # Engine Subsystems Framework
//!
//! Provides a trait-based framework for engine subsystems with:
//! - Dependency declaration and resolution (topological sort via Kahn's algorithm)
//! - Lifecycle management (init, shutdown, per-frame updates)
//! - Type-erased subsystem registry
//! - Shared context for runtime handles
//!
//! ## Architecture
//!
//! The registry stores all subsystems as `Box<dyn Subsystem>` — type-erased.
//! Subsystem consumers downcast to the concrete type they know:
//!
//! ```rust,ignore
//! let ss = registry.get(subsystem_ids::RENDERING).unwrap();
//! let any: &dyn std::any::Any = ss;
//! let renderer = any
//!     .downcast_ref::<HelioRenderer>()
//!     .expect("Renderer subsystem is not HelioRenderer");
//! ```
//!
//! This crate has no engine or UI dependencies — only `tokio` for the async
//! runtime handle. Both `engine_backend` (built-in subsystems) and
//! `plugin_editor_api` (plugin-provided subsystems) depend on this crate.

use std::any::Any;
use std::collections::{HashMap, VecDeque};
use tokio::runtime::Handle;

/// Unique identifier for a subsystem.
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

impl std::fmt::Display for SubsystemId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Well-known subsystem identifiers.
pub mod subsystem_ids {
    use super::SubsystemId;

    pub const PHYSICS: SubsystemId = SubsystemId::new("physics");
    pub const AUDIO: SubsystemId = SubsystemId::new("audio");
    pub const INPUT: SubsystemId = SubsystemId::new("input");
    pub const NETWORKING: SubsystemId = SubsystemId::new("networking");
    pub const SCRIPTING: SubsystemId = SubsystemId::new("scripting");
    pub const RENDERING: SubsystemId = SubsystemId::new("rendering");
    pub const WORLD: SubsystemId = SubsystemId::new("world");
}

/// Shared context provided to all subsystems during initialization.
#[derive(Clone)]
pub struct SubsystemContext {
    /// Tokio runtime handle for spawning async tasks.
    pub runtime: Handle,
}

impl SubsystemContext {
    pub fn new(runtime: Handle) -> Self {
        Self { runtime }
    }
}

/// Errors that can occur during subsystem operations.
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

    #[error("Subsystem not found: {0}")]
    NotFound(&'static str),
}

/// Trait that all engine subsystems must implement.
///
/// Subsystems are independent engine components that can declare dependencies
/// on other subsystems and participate in lifecycle management.
///
/// The `Any` bound allows consumers to downcast to the concrete type:
///
/// ```rust,ignore
/// use engine_subsystems::*;
///
/// struct PhysicsSubsystem {
///     engine: Option<PhysicsEngine>,
/// }
///
/// impl Subsystem for PhysicsSubsystem {
///     fn id(&self) -> SubsystemId { subsystem_ids::PHYSICS }
///     fn dependencies(&self) -> Vec<SubsystemId> { vec![] }
///     fn init(&mut self, context: &SubsystemContext) -> Result<(), SubsystemError> {
///         let engine = PhysicsEngine::new();
///         context.runtime.spawn(async move { /* physics loop */ });
///         self.engine = Some(engine);
///         Ok(())
///     }
///     fn shutdown(&mut self) -> Result<(), SubsystemError> { Ok(()) }
/// }
/// ```
pub trait Subsystem: Send + Sync + Any {
    /// Unique identifier for this subsystem.
    fn id(&self) -> SubsystemId;

    /// List of subsystems this one depends on (must be initialized first).
    fn dependencies(&self) -> Vec<SubsystemId>;

    /// Initialize the subsystem with the provided context.
    fn init(&mut self, context: &SubsystemContext) -> Result<(), SubsystemError>;

    /// Shutdown the subsystem (cleanup resources).
    fn shutdown(&mut self) -> Result<(), SubsystemError>;

    /// Optional per-frame update (default: no-op).
    fn on_frame(&mut self, _delta_time: f32) {}
}

/// Registry for managing subsystems with dependency resolution.
///
/// All subsystems are stored as `Box<dyn Subsystem>` — fully type-erased.
/// Consumers downcast via `Any` to get their concrete type back.
///
/// # Example
///
/// ```rust,ignore
/// use engine_subsystems::*;
///
/// let mut registry = SubsystemRegistry::new();
/// registry.register(MySubsystem::new()).unwrap();
/// registry.init_all(&context).unwrap();
///
/// // Later, get and downcast (must cast through `Any`):
/// let ss = registry.get(subsystem_ids::PHYSICS).unwrap();
/// let any: &dyn std::any::Any = ss;
/// let phys = any.downcast_ref::<MySubsystem>().unwrap();
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
    pub fn register<S: Subsystem + 'static>(&mut self, subsystem: S) -> Result<(), SubsystemError> {
        let id = subsystem.id();
        if self.subsystems.contains_key(&id) {
            return Err(SubsystemError::AlreadyRegistered(id.as_str()));
        }
        self.subsystems.insert(id, Box::new(subsystem));
        Ok(())
    }

    /// Register a subsystem that is already boxed (e.g., from a plugin).
    pub fn register_boxed(
        &mut self,
        subsystem: Box<dyn Subsystem>,
    ) -> Result<(), SubsystemError> {
        let id = subsystem.id();
        if self.subsystems.contains_key(&id) {
            return Err(SubsystemError::AlreadyRegistered(id.as_str()));
        }
        self.subsystems.insert(id, subsystem);
        Ok(())
    }

    /// Merge all subsystems from another registry into this one.
    /// Silently skips IDs that already exist (first-registered wins).
    pub fn merge(&mut self, other: SubsystemRegistry) {
        for (id, subsystem) in other.subsystems {
            if !self.subsystems.contains_key(&id) {
                self.subsystems.insert(id, subsystem);
            }
        }
    }

    /// Resolve dependencies using topological sort (Kahn's algorithm).
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
    pub fn shutdown_all(&mut self) -> Result<(), SubsystemError> {
        if !self.initialized {
            return Ok(());
        }

        for id in self.init_order.iter().rev() {
            if let Some(subsystem) = self.subsystems.get_mut(id) {
                tracing::debug!("Shutting down subsystem: {}", id.as_str());
                subsystem
                    .shutdown()
                    .map_err(|e| SubsystemError::ShutdownFailed(format!("{}: {}", id.as_str(), e)))?;
            }
        }

        self.initialized = false;
        tracing::info!("All subsystems shut down successfully");
        Ok(())
    }

    /// Call `on_frame` for all initialized subsystems, in init order.
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

    /// Get a subsystem by ID.
    pub fn get(&self, id: SubsystemId) -> Option<&dyn Subsystem> {
        self.subsystems.get(&id).map(|b| &**b as &dyn Subsystem)
    }

    /// Get a mutable subsystem by ID.
    pub fn get_mut(&mut self, id: SubsystemId) -> Option<&mut dyn Subsystem> {
        self.subsystems.get_mut(&id).map(|b| &mut **b as &mut dyn Subsystem)
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
            Self { id, deps, init_called: false }
        }
    }

    impl Subsystem for MockSubsystem {
        fn id(&self) -> SubsystemId { self.id }
        fn dependencies(&self) -> Vec<SubsystemId> { self.deps.clone() }

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

        registry.register(MockSubsystem::new(a, vec![])).unwrap();
        registry.register(MockSubsystem::new(b, vec![a])).unwrap();
        registry.register(MockSubsystem::new(c, vec![b])).unwrap();

        let order = registry.resolve_dependencies().unwrap();

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

        registry.register(MockSubsystem::new(b, vec![a])).unwrap();

        let result = registry.resolve_dependencies();
        assert!(matches!(result, Err(SubsystemError::MissingDependency { .. })));
    }

    #[test]
    fn test_register_boxed() {
        let mut registry = SubsystemRegistry::new();
        let id = SubsystemId::new("boxed");

        registry
            .register_boxed(Box::new(MockSubsystem::new(id, vec![])))
            .unwrap();

        assert!(registry.contains(id));
    }

    #[test]
    fn test_merge() {
        let mut reg1 = SubsystemRegistry::new();
        let mut reg2 = SubsystemRegistry::new();

        let a = SubsystemId::new("a");
        let b = SubsystemId::new("b");

        reg1.register(MockSubsystem::new(a, vec![])).unwrap();
        reg2.register(MockSubsystem::new(b, vec![])).unwrap();

        reg1.merge(reg2);
        assert!(reg1.contains(a));
        assert!(reg1.contains(b));
    }

    #[test]
    fn test_downcast_after_register() {
        let mut registry = SubsystemRegistry::new();
        let id = SubsystemId::new("downcast_test");
        registry.register(MockSubsystem::new(id, vec![])).unwrap();

        let ss = registry.get(id).unwrap();
        // Must cast through Any since `dyn Subsystem` doesn't expose
        // `dyn Any` methods directly.
        let any: &dyn std::any::Any = ss;
        let downcasted = any.downcast_ref::<MockSubsystem>();
        assert!(downcasted.is_some());
    }
}
