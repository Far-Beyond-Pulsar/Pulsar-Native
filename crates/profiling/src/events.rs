//! Profile event data structures

use serde::{Deserialize, Serialize};

/// A profiling event captured via instrumentation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileEvent {
    /// Stable identity for this completed scope. Unlike `thread_id`, this
    /// remains valid when work is continued on another thread or queue.
    #[serde(default)]
    pub scope_id: u64,
    /// Stable identity of the logical scope that spawned/contains this work.
    #[serde(default)]
    pub parent_scope_id: Option<u64>,
    /// Event name (function/scope name)
    pub name: String,
    /// Thread ID
    pub thread_id: u64,
    /// Thread name (if set)
    pub thread_name: Option<String>,
    /// Process ID
    pub process_id: u32,
    /// Parent scope name (if nested)
    pub parent_name: Option<String>,
    /// Start time in nanoseconds (absolute)
    pub start_ns: u64,
    /// Duration in nanoseconds
    pub duration_ns: u64,
    /// Stack depth / nesting level
    pub depth: u32,
    /// File location (file:line)
    pub location: Option<String>,
    /// Additional metadata (optional)
    pub metadata: Option<String>,
    /// Optional producer-defined logical track. This is deliberately separate
    /// from the OS thread name and supports CPU workers, GPU queues, async
    /// executors, and other domains uniformly.
    #[serde(default)]
    pub track_name: Option<String>,
}
