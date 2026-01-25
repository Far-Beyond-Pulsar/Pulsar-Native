#![allow(warnings)]

//! # Pulsar Engine Backend
//!
//! This crate provides the backend functionalities for the Pulsar game engine, including
//! rendering, asset management, and core engine systems.
//! It is designed to be modular and extensible, allowing developers to
//! build high-performance games with ease.

pub mod subsystems;
pub mod gpu_interop;
pub mod services;

pub use tokio;
pub use subsystems::physics::PhysicsEngine;
pub use subsystems::game::{GameThread, GameState, GameObject};
pub use subsystems::world::World;
pub use subsystems::render::{WgpuRenderer, BevyRenderer, Framebuffer as RenderFramebuffer};
pub use subsystems::framework::{SubsystemRegistry, SubsystemContext, Subsystem, SubsystemError};
pub use services::{GpuRenderer, GlobalRustAnalyzerCompletionProvider, RustAnalyzerManager};
pub use std::sync::Arc;

pub const ENGINE_THREADS: [&str; 8] = [
    "GameThread",
    "RenderThread",
    "AssetLoaderThread",
    "PhysicsThread",
    "AIThread",
    "AudioThread",
    "NetworkThread",
    "InputThread",
];

use tokio::sync::Mutex;

pub struct EngineBackend {
    subsystems: SubsystemRegistry,
}

impl EngineBackend {
    pub async fn init() -> Self {
        profiling::profile_scope!("EngineBackend::Init");

        tracing::debug!("Initializing Engine Backend with Subsystem Registry");

        // Create subsystem registry
        let mut registry = SubsystemRegistry::new();

        // Register all subsystems
        registry.register(PhysicsEngine::new())
            .expect("Failed to register PhysicsEngine subsystem");

        registry.register(GameThread::new(60.0)) // 60 TPS target
            .expect("Failed to register GameThread subsystem");

        // NOTE: World subsystem cannot be registered here because PebbleVault::VaultManager
        // doesn't implement Send + Sync. It must be managed separately.

        // Create subsystem context with current runtime handle
        let runtime_handle = tokio::runtime::Handle::current();
        let context = SubsystemContext::new(runtime_handle);

        // Initialize all subsystems in dependency order
        registry.init_all(&context)
            .expect("Failed to initialize subsystems");

        tracing::info!("✅ Engine Backend initialized with all subsystems");

        EngineBackend { subsystems: registry }
    }

    /// Get a reference to the subsystem registry
    pub fn subsystems(&self) -> &SubsystemRegistry {
        &self.subsystems
    }

    /// Get a mutable reference to the subsystem registry
    pub fn subsystems_mut(&mut self) -> &mut SubsystemRegistry {
        &mut self.subsystems
    }

    /// Shutdown all subsystems gracefully
    pub fn shutdown(&mut self) -> Result<(), SubsystemError> {
        profiling::profile_scope!("EngineBackend::Shutdown");
        tracing::info!("Shutting down Engine Backend");
        self.subsystems.shutdown_all()
    }
}
