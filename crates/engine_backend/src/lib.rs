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
pub use subsystems::game::{GameThread, ManagedGameThread, GameState, GameObject};
pub use subsystems::world::World;
pub use subsystems::render::{WgpuRenderer, Framebuffer as RenderFramebuffer};
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
use std::sync::OnceLock;

static GLOBAL_BACKEND: OnceLock<Arc<parking_lot::RwLock<EngineBackend>>> = OnceLock::new();

pub struct EngineBackend {
    subsystems: SubsystemRegistry,
    game_thread: Option<Arc<GameThread>>,
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

        let managed_game_thread = ManagedGameThread::new(60.0); // 60 TPS target
        let game_thread_ref = managed_game_thread.game_thread().clone();

        registry.register(managed_game_thread)
            .expect("Failed to register ManagedGameThread subsystem");

        // NOTE: World subsystem cannot be registered here because PebbleVault::VaultManager
        // doesn't implement Send + Sync. It must be managed separately.

        // Create subsystem context with current runtime handle
        let runtime_handle = tokio::runtime::Handle::current();
        let context = SubsystemContext::new(runtime_handle);

        // Initialize all subsystems in dependency order
        registry.init_all(&context)
            .expect("Failed to initialize subsystems");

        tracing::info!("✅ Engine Backend initialized with all subsystems");

        EngineBackend {
            subsystems: registry,
            game_thread: Some(game_thread_ref),
        }
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

    /// Get the central GameThread instance
    pub fn game_thread(&self) -> Option<&Arc<GameThread>> {
        self.game_thread.as_ref()
    }

    /// Set as global instance (for access from other parts of the engine)
    pub fn set_global(backend: Arc<parking_lot::RwLock<Self>>) {
        GLOBAL_BACKEND.set(backend).ok();
    }

    /// Get global instance
    pub fn global() -> Option<&'static Arc<parking_lot::RwLock<EngineBackend>>> {
        GLOBAL_BACKEND.get()
    }
}
