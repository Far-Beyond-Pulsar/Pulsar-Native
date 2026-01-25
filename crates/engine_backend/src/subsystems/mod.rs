//! Module containing all engine subsystems.
//!
//! Each subsystem is responsible for a specific aspect of the engine's functionality,
//! such as rendering, audio, physics, input handling, and more.
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
//! The following subsystems have been migrated to use the Subsystem trait:
//! - ✅ **PhysicsEngine** - Rapier3D physics simulation with async task spawning
//! - ✅ **GameThread** - Fixed timestep game loop with std::thread
//! - ❌ **World** - Cannot implement Subsystem due to PebbleVault not being Send+Sync
//!
//! ## Pending Subsystems
//!
//! These subsystems are planned for future migration:
//! - ⏳ Audio, Input, Networking, Scripting, UI
//! - All new subsystems should implement the Subsystem trait from the start

// Subsystem framework
pub mod framework;
pub mod assets;
pub mod audio;
pub mod classes;
pub mod world;
pub mod render;
pub mod physics;
pub mod game;
pub mod input;
pub mod networking;
pub mod game_network;
pub mod ui;
pub mod scripting;
pub mod themes;
pub mod settings;