//! Profile event data structures

use serde::{Deserialize, Serialize};
use std::time::Instant;

/// A profiling event captured via instrumentation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileEvent {
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
}
