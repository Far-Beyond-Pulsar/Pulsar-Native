//! Module containing all engine subsystems.
//!
//! Each subsystem is responsible for a specific aspect of the engine's functionality,
//! such as rendering and the world. What an object kind simulates (physics,
//! input...) is its plugin's.
//!
//! This modular design allows for better organization, maintainability,
//! and scalability of the engine's codebase.

#[cfg(feature = "render")]
pub mod render;
pub mod world;
