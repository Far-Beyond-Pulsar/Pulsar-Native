//! Module containing all engine subsystems.
//! 
//! Each subsystem is responsible for a specific aspect of the engine's functionality,
//! such as rendering, audio, physics, input handling, and more.
//! 
//! This modular design allows for better organization, maintainability,
//! and scalability of the engine's codebase.

// TODO: Implement a generic subsystem trait and have each subsystem implement it.
//       This will allow for better management and interaction between subsystems
//       via generic function calls.
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