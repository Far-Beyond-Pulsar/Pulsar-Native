//! Engine Initialization Module
//!
//! Provides a dependency graph-based initialization system to replace
//! the procedural initialization in main.rs.

pub mod graph;

pub use graph::{InitGraph, InitTask, InitContext, InitError, TaskId, task_ids};
