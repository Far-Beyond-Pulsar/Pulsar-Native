//! Engine Initialization Module
//!
//! Provides a dependency graph-based initialization system to replace
//! the procedural initialization in main.rs.

pub mod graph;

pub use graph::{task_ids, InitContext, InitError, InitGraph, InitTask, TaskId};
