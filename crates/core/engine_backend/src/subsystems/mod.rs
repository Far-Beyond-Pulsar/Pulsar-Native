//! Module containing all engine subsystems.
//!
//! Each subsystem is responsible for a specific aspect of the engine's functionality,
//! such as rendering and the world. What an object kind simulates (physics,
//! input...) is its plugin's.
//!
//! This modular design allows for better organization, maintainability,
//! and scalability of the engine's codebase.
//!
//! ## Subsystem Framework
//!
//! The subsystem framework provides a trait-based architecture for all engine subsystems:
//! - **Subsystem trait**: Common interface for init, shutdown, and lifecycle management
//! - **SubsystemRegistry**: Manages subsystems with dependency resolution (topological sort)
//! - **Type-safe dependencies**: Explicit dependency declarations prevent initialization bugs
//!
//! ## Migrated Subsystems
//!
//! `World` cannot implement Subsystem (PebbleVault is not Send+Sync). A
//! plugin library may provide subsystems (`plugin_manager` collects them);
//! new subsystems implement the trait from the start.

// Subsystem framework
pub mod framework;
#[cfg(feature = "render")]
pub mod render;
pub mod world;
