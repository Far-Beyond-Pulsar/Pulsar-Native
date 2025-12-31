//! Task types for dependency setup tracking.
//!
//! This module defines the types used to track individual setup tasks
//! and their execution status.

/// A single setup task in the dependency installation process.
///
/// Each task represents a step in the setup workflow, such as checking
/// for Rust installation or verifying build tools.
#[derive(Clone, Debug)]
pub struct SetupTask {
    /// Human-readable name of the task.
    pub name: String,
    
    /// Detailed description of what the task does.
    pub description: String,
    
    /// Current execution status of the task.
    pub status: TaskStatus,
}

/// The execution status of a setup task.
#[derive(Clone, Debug, PartialEq)]
pub enum TaskStatus {
    /// Task has not started yet.
    Pending,
    
    /// Task is currently executing.
    InProgress,
    
    /// Task completed successfully.
    Completed,
    
    /// Task failed with an error message.
    Failed(String),
}

impl SetupTask {
    /// Creates a new setup task with a pending status.
    ///
    /// # Arguments
    ///
    /// * `name` - Display name for the task
    /// * `description` - Detailed description of what the task does
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            status: TaskStatus::Pending,
        }
    }
}
