//! Dependency setup window module for Pulsar Native.
//!
//! This module provides an interactive UI for checking and installing required
//! development dependencies for building the Pulsar Native engine.
//!
//! # Module Structure
//!
//! - [`window`] - Main window implementation and UI rendering
//! - [`task`] - Task types and status tracking
//! - [`checks`] - Platform-specific dependency checks
//! - [`installer`] - Automated dependency installation
//! - [`scripts`] - Embedded setup scripts

mod checks;
mod installer;
mod scripts;
mod task;
mod window;

// Re-export public types
pub use task::{SetupTask, TaskStatus};
pub use window::{DependencySetupWindow, SetupComplete};
