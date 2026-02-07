//! # Initialization Dependency Graph
//!
//! Provides a declarative way to define engine initialization with explicit dependencies.
//!
//! ## Why?
//!
//! The old initialization was a 15-step procedural sequence where dependencies were implicit:
//! - Discord init depended on `set_global()` being called first (line 127 needed line 122)
//! - URI registration depended on runtime already running (line 131 needed line 98)
//! - No documentation of required ordering
//! - Hard to add new initialization steps
//!
//! ## How It Works
//!
//! 1. **Define tasks** with explicit dependencies:
//!    ```ignore
//!    graph.add_task(InitTask::new(
//!        DISCORD,
//!        "Discord Rich Presence",
//!        vec![SET_GLOBAL],  // Explicit dependency!
//!        Box::new(|ctx| { /* init code */ })
//!    ))
//!    ```
//!
//! 2. **Validate graph** - Detects cycles and missing dependencies:
//!    ```ignore
//!    let order = graph.build_order()?; // Returns Err if cycle detected
//!    ```
//!
//! 3. **Execute in order** - Tasks run in topological order:
//!    ```ignore
//!    graph.execute(&mut init_ctx)?;
//!    ```
//!
//! ## Profiling
//!
//! Each task is automatically profiled with scope `Engine::Init::{TaskName}` and
//! timing is logged with ▶ (start) and ✓ (complete) markers.
//!
//! ## Algorithm
//!
//! Uses **Kahn's algorithm** for topological sorting to determine execution order.

use std::collections::{HashMap, HashSet, VecDeque};
use thiserror::Error;
use tokio::runtime::Runtime;
use engine_state::{EngineContext, WindowRequestReceiver, WindowRequestSender};
use crate::args::ParsedArgs;
use crate::logging::LogGuard;

/// Unique identifier for an initialization task
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(&'static str);

impl TaskId {
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

/// Common initialization task IDs
pub mod task_ids {
    use super::TaskId;

    pub const LOGGING: TaskId = TaskId::new("logging");
    pub const APPDATA: TaskId = TaskId::new("appdata");
    pub const SETTINGS: TaskId = TaskId::new("settings");
    pub const RUNTIME: TaskId = TaskId::new("runtime");
    pub const BACKEND: TaskId = TaskId::new("backend");
    pub const CHANNELS: TaskId = TaskId::new("channels");
    pub const ENGINE_CONTEXT: TaskId = TaskId::new("engine_context");
    pub const URI_HANDLING: TaskId = TaskId::new("uri_handling");
    pub const SET_GLOBAL: TaskId = TaskId::new("set_global");
    pub const DISCORD: TaskId = TaskId::new("discord");
    pub const URI_REGISTRATION: TaskId = TaskId::new("uri_registration");
}

/// Errors that can occur during initialization
#[derive(Debug, Error)]
pub enum InitError {
    #[error("Task initialization failed: {task} - {error}")]
    TaskFailed { task: &'static str, error: String },

    #[error("Dependency cycle detected involving: {tasks:?}")]
    DependencyCycle { tasks: Vec<&'static str> },

    #[error("Missing dependency: {dependency} required by {task}")]
    MissingDependency {
        task: &'static str,
        dependency: &'static str,
    },

    #[error("Task already registered: {0}")]
    AlreadyRegistered(&'static str),

    #[error("Required context not available: {0}")]
    MissingContext(&'static str),
}

/// Context passed between initialization tasks
///
/// Each task can add to this context, and subsequent tasks can depend on values being present.
pub struct InitContext {
    /// Parsed command-line arguments
    pub launch_args: ParsedArgs,

    /// Log guard (must be kept alive)
    pub log_guard: Option<LogGuard>,

    /// Async runtime
    pub runtime: Option<Runtime>,

    /// Engine backend
    pub backend: Option<engine_backend::EngineBackend>,

    /// Window request sender channel
    pub window_tx: Option<WindowRequestSender>,

    /// Window request receiver channel
    pub window_rx: Option<WindowRequestReceiver>,

    /// Engine context (replaces EngineState)
    pub engine_context: Option<EngineContext>,
}

impl InitContext {
    /// Create a new initialization context
    pub fn new(launch_args: ParsedArgs) -> Self {
        Self {
            launch_args,
            log_guard: None,
            runtime: None,
            backend: None,
            window_tx: None,
            window_rx: None,
            engine_context: None,
        }
    }
}

/// Type alias for initialization task executor functions
pub type TaskExecutor = Box<dyn FnOnce(&mut InitContext) -> Result<(), InitError> + Send>;

/// A single initialization task with dependencies
pub struct InitTask {
    /// Unique identifier
    pub id: TaskId,

    /// Human-readable name
    pub name: &'static str,

    /// Tasks that must complete before this one
    pub dependencies: Vec<TaskId>,

    /// Function to execute
    pub executor: TaskExecutor,
}

impl InitTask {
    /// Create a new initialization task
    pub fn new(
        id: TaskId,
        name: &'static str,
        dependencies: Vec<TaskId>,
        executor: TaskExecutor,
    ) -> Self {
        Self {
            id,
            name,
            dependencies,
            executor,
        }
    }
}

/// Dependency graph for engine initialization
pub struct InitGraph {
    tasks: HashMap<TaskId, InitTask>,
}

impl InitGraph {
    /// Create a new empty initialization graph
    pub fn new() -> Self {
        Self {
            tasks: HashMap::new(),
        }
    }

    /// Add a task to the graph (builder pattern)
    pub fn add_task(&mut self, task: InitTask) -> Result<&mut Self, InitError> {
        let id = task.id;

        if self.tasks.contains_key(&id) {
            return Err(InitError::AlreadyRegistered(id.as_str()));
        }

        self.tasks.insert(id, task);
        Ok(self)
    }

    /// Build the initialization order using topological sort (Kahn's algorithm)
    pub fn build_order(&self) -> Result<Vec<TaskId>, InitError> {
        // Build dependency graph
        let mut in_degree: HashMap<TaskId, usize> = HashMap::new();
        let mut adjacency: HashMap<TaskId, Vec<TaskId>> = HashMap::new();

        // Initialize all tasks with in-degree 0
        for id in self.tasks.keys() {
            in_degree.insert(*id, 0);
            adjacency.insert(*id, Vec::new());
        }

        // Build the graph: for each task, add edges from dependencies to it
        for (id, task) in &self.tasks {
            // Validate all dependencies exist
            for dep in &task.dependencies {
                if !self.tasks.contains_key(dep) {
                    return Err(InitError::MissingDependency {
                        task: id.as_str(),
                        dependency: dep.as_str(),
                    });
                }
            }

            // Increment in-degree for this task (one per dependency)
            *in_degree.get_mut(id).unwrap() += task.dependencies.len();

            // Add edges from each dependency to this task
            for dep in &task.dependencies {
                adjacency.get_mut(dep).unwrap().push(*id);
            }
        }

        // Kahn's algorithm: process nodes with in-degree 0
        let mut queue: VecDeque<TaskId> = in_degree
            .iter()
            .filter(|(_, &degree)| degree == 0)
            .map(|(&id, _)| id)
            .collect();

        let mut order = Vec::new();

        while let Some(id) = queue.pop_front() {
            order.push(id);

            // For each dependent of this task
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

        // If we didn't process all tasks, there's a cycle
        if order.len() != self.tasks.len() {
            let unprocessed: Vec<&'static str> = self
                .tasks
                .keys()
                .filter(|id| !order.contains(id))
                .map(|id| id.as_str())
                .collect();

            return Err(InitError::DependencyCycle { tasks: unprocessed });
        }

        Ok(order)
    }

    /// Execute all tasks in dependency order
    pub fn execute(mut self, context: &mut InitContext) -> Result<(), InitError> {
        // Build execution order
        let order = self.build_order()?;

        tracing::info!(
            "Engine initialization order: {:?}",
            order.iter().map(|id| id.as_str()).collect::<Vec<_>>()
        );

        // Execute tasks in order
        for id in order {
            let task = self.tasks.remove(&id).unwrap();

            tracing::debug!("▶ Executing init task: {}", task.name);

            // Profile each initialization task with its specific name
            let scope_name = format!("Engine::Init::{}", task.name);
            profiling::profile_scope!(&scope_name);

            let start = std::time::Instant::now();
            (task.executor)(context).map_err(|e| InitError::TaskFailed {
                task: task.name,
                error: e.to_string(),
            })?;
            let duration = start.elapsed();

            tracing::debug!("✓ Completed init task: {} ({:?})", task.name, duration);
        }

        tracing::info!("🎉 Engine initialization complete");
        Ok(())
    }

    /// Visualize the dependency graph as DOT format (for documentation)
    pub fn to_dot(&self) -> String {
        let mut dot = String::from("digraph InitGraph {\n");
        dot.push_str("  rankdir=LR;\n");
        dot.push_str("  node [shape=box];\n\n");

        for (id, task) in &self.tasks {
            let label = task.name;
            dot.push_str(&format!("  \"{}\" [label=\"{}\"];\n", id.as_str(), label));

            for dep in &task.dependencies {
                dot.push_str(&format!("  \"{}\" -> \"{}\";\n", dep.as_str(), id.as_str()));
            }
        }

        dot.push_str("}\n");
        dot
    }
}

impl Default for InitGraph {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_dependency_order() {
        let mut graph = InitGraph::new();

        let a = TaskId::new("a");
        let b = TaskId::new("b");
        let c = TaskId::new("c");

        // b depends on a, c depends on b
        graph
            .add_task(InitTask::new(a, "Task A", vec![], Box::new(|_| Ok(()))))
            .unwrap();
        graph
            .add_task(InitTask::new(b, "Task B", vec![a], Box::new(|_| Ok(()))))
            .unwrap();
        graph
            .add_task(InitTask::new(c, "Task C", vec![b], Box::new(|_| Ok(()))))
            .unwrap();

        let order = graph.build_order().unwrap();

        // a must come before b, b must come before c
        let a_pos = order.iter().position(|&id| id == a).unwrap();
        let b_pos = order.iter().position(|&id| id == b).unwrap();
        let c_pos = order.iter().position(|&id| id == c).unwrap();

        assert!(a_pos < b_pos);
        assert!(b_pos < c_pos);
    }

    #[test]
    fn test_cycle_detection() {
        let mut graph = InitGraph::new();

        let a = TaskId::new("a");
        let b = TaskId::new("b");

        // Create a cycle: a depends on b, b depends on a
        graph
            .add_task(InitTask::new(a, "Task A", vec![b], Box::new(|_| Ok(()))))
            .unwrap();
        graph
            .add_task(InitTask::new(b, "Task B", vec![a], Box::new(|_| Ok(()))))
            .unwrap();

        let result = graph.build_order();
        assert!(matches!(result, Err(InitError::DependencyCycle { .. })));
    }

    #[test]
    fn test_missing_dependency() {
        let mut graph = InitGraph::new();

        let a = TaskId::new("a");
        let b = TaskId::new("b");

        // b depends on a, but a is not registered
        graph
            .add_task(InitTask::new(b, "Task B", vec![a], Box::new(|_| Ok(()))))
            .unwrap();

        let result = graph.build_order();
        assert!(matches!(result, Err(InitError::MissingDependency { .. })));
    }

    #[test]
    fn test_parallel_tasks() {
        let mut graph = InitGraph::new();

        let a = TaskId::new("a");
        let b = TaskId::new("b");
        let c = TaskId::new("c");

        // a and b have no dependencies, c depends on both
        graph
            .add_task(InitTask::new(a, "Task A", vec![], Box::new(|_| Ok(()))))
            .unwrap();
        graph
            .add_task(InitTask::new(b, "Task B", vec![], Box::new(|_| Ok(()))))
            .unwrap();
        graph
            .add_task(InitTask::new(c, "Task C", vec![a, b], Box::new(|_| Ok(()))))
            .unwrap();

        let order = graph.build_order().unwrap();

        // a and b must both come before c, but can be in any order relative to each other
        let a_pos = order.iter().position(|&id| id == a).unwrap();
        let b_pos = order.iter().position(|&id| id == b).unwrap();
        let c_pos = order.iter().position(|&id| id == c).unwrap();

        assert!(a_pos < c_pos);
        assert!(b_pos < c_pos);
    }

    #[test]
    fn test_dot_visualization() {
        let mut graph = InitGraph::new();

        let a = TaskId::new("a");
        let b = TaskId::new("b");

        graph
            .add_task(InitTask::new(a, "Task A", vec![], Box::new(|_| Ok(()))))
            .unwrap();
        graph
            .add_task(InitTask::new(b, "Task B", vec![a], Box::new(|_| Ok(()))))
            .unwrap();

        let dot = graph.to_dot();
        assert!(dot.contains("digraph InitGraph"));
        assert!(dot.contains("\"a\" -> \"b\""));
    }
}
