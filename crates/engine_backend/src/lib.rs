//! # Pulsar Engine Backend
//!
//! This crate provides the backend functionalities for the Pulsar game engine, including
//! rendering, asset management, and core engine systems.
//! It is designed to be modular and extensible, allowing developers to
//! build high-performance games with ease.

pub mod scene;
pub mod services;
pub mod subsystems;

pub use services::{GpuRenderer, RustAnalyzerManager};
use std::sync::Arc;
pub use subsystems::framework::{Subsystem, SubsystemContext, SubsystemError, SubsystemRegistry};
pub use subsystems::physics::PhysicsEngine;
pub use subsystems::render::{Framebuffer as RenderFramebuffer, WgpuRenderer};
pub use subsystems::world::World;

// Re-export Helio types for UI integration
pub use helio::GizmoMode;

// Re-export reflection system for convenience
pub use pulsar_reflection::*;

// Re-export scene types used by UI crates
pub use scene::{
    ComponentInstance, EditorObjectId, HelioActorHandle, MetadataObjectType, SceneMetadataDb,
    SceneObjectMetadata, SceneSnapshot,
};

pub const ENGINE_THREADS: [&str; 7] = [
    "RenderThread",
    "AssetLoaderThread",
    "PhysicsThread",
    "AIThread",
    "AudioThread",
    "NetworkThread",
    "InputThread",
];

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
        registry
            .register(PhysicsEngine::new())
            .expect("Failed to register PhysicsEngine subsystem");

        // NOTE: World subsystem cannot be registered here because PebbleVault::VaultManager
        // doesn't implement Send + Sync. It must be managed separately.

        // Create subsystem context with current runtime handle
        let runtime_handle = tokio::runtime::Handle::current();
        let context = SubsystemContext::new(runtime_handle);

        // Initialize all subsystems in dependency order
        registry
            .init_all(&context)
            .expect("Failed to initialize subsystems");

        tracing::info!("✅ Engine Backend initialized with all subsystems");

        EngineBackend {
            subsystems: registry,
        }
    }

    /// Get the physics query service for raycasting
    pub fn get_physics_query_service(&self) -> Option<Arc<crate::services::PhysicsQueryService>> {
        use crate::subsystems::framework::subsystem_ids;

        self.subsystems
            .get(subsystem_ids::PHYSICS)
            .and_then(|subsystem| {
                // Downcast to PhysicsEngine
                (subsystem as &dyn std::any::Any)
                    .downcast_ref::<PhysicsEngine>()
                    .map(|physics| physics.query_service())
            })
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

    /// Set as global instance (for access from other parts of the engine)
    pub fn set_global(backend: Self) {
        if let Some(ctx) = engine_state::EngineContext::global() {
            ctx.store.insert(backend);
        }
    }

    /// Get global instance
    pub fn global() -> Option<engine_state::ResourceHandle<EngineBackend>> {
        engine_state::EngineContext::global()?
            .store
            .get::<EngineBackend>()
    }
}
