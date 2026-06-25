//! # Pulsar Engine Backend
//!
//! This crate provides the backend functionalities for the Pulsar game engine, including
//! rendering, asset management, and core engine systems.
//! It is designed to be modular and extensible, allowing developers to
//! build high-performance games with ease.

pub mod component_registry;
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
    plugin_components: component_registry::PluginComponentRegistry,
}

impl EngineBackend {
    /// Initialize engine backend with built-in subsystems.
    ///
    /// Built-in subsystems are registered and initialized immediately.
    /// Plugin-provided subsystems are injected later via
    /// [`inject_plugin_subsystems`](Self::inject_plugin_subsystems).
    ///
    /// # Built-in Subsystems
    ///
    /// - **PhysicsEngine** — Rapier3D physics simulation
    ///
    /// NOTE: World subsystem cannot be registered here because
    /// PebbleVault::VaultManager doesn't implement `Send + Sync`.
    /// It must be managed separately.
    pub async fn init() -> Self {
        profiling::profile_scope!("EngineBackend::Init");
        tracing::debug!("Initializing Engine Backend with Subsystem Registry");

        let mut registry = SubsystemRegistry::new();
        let context = SubsystemContext::new(tokio::runtime::Handle::current());

        registry
            .register(PhysicsEngine::new())
            .expect("Failed to register PhysicsEngine subsystem");

        // Initialize built-in subsystems now so they are usable immediately.
        registry
            .init_all(&context)
            .expect("Failed to initialize built-in subsystems");

        tracing::info!("✅ Engine Backend initialized");

        EngineBackend {
            subsystems: registry,
            plugin_components: component_registry::PluginComponentRegistry::new(),
        }
    }

    /// Inject and initialize subsystems provided by plugins.
    ///
    /// Called once by `ui_core` after `PluginManager` has loaded all DLLs.
    /// Plugin subsystems are merged into the existing registry — if a plugin
    /// provides a subsystem with the same ID as a built-in one, the built-in
    /// wins (first-registered-wins from `register_boxed`).
    ///
    /// Each plugin subsystem is initialized individually after registration.
    pub fn inject_plugin_subsystems(
        &mut self,
        subsystems: Vec<Box<dyn engine_subsystems::Subsystem>>,
    ) -> Result<(), SubsystemError> {
        profiling::profile_scope!("EngineBackend::InjectPluginSubsystems");

        if subsystems.is_empty() {
            return Ok(());
        }

        tracing::info!(
            "Injecting {} plugin subsystem(s) into engine backend",
            subsystems.len()
        );

        let context = SubsystemContext::new(tokio::runtime::Handle::current());

        for mut ss in subsystems {
            let id = ss.id();
            match self.subsystems.register_boxed(ss) {
                Ok(()) => {
                    // Initialize just this new subsystem.
                    let ss = self.subsystems.get_mut(id).unwrap();
                    ss.init(&context).map_err(|e| {
                        SubsystemError::InitFailed(format!("{}: {}", id.as_str(), e))
                    })?;
                    tracing::debug!("  ✅ Plugin subsystem initialized: {}", id);
                }
                Err(SubsystemError::AlreadyRegistered(_)) => {
                    tracing::debug!(
                        "  ⏭️  Plugin subsystem '{}' already registered (built-in wins)",
                        id
                    );
                }
                Err(e) => {
                    tracing::error!("  ❌ Failed to register plugin subsystem '{}': {}", id, e);
                }
            }
        }

        tracing::info!("✅ Plugin subsystems injected");
        Ok(())
    }

    /// Inject plugin-provided component factories into the engine backend.
    ///
    /// Called once by `ui_core` after `PluginManager` has loaded all DLLs.
    /// Each entry is `(class_name, factory, default_data)`.
    pub fn inject_plugin_components(
        &mut self,
        registrations: Vec<(
            String,
            component_registry::ComponentFactory,
            serde_json::Value,
        )>,
    ) {
        if registrations.is_empty() {
            return;
        }

        tracing::info!(
            "Injecting {} plugin component(s) into engine backend",
            registrations.len()
        );

        self.plugin_components.register_all(registrations);

        tracing::info!("✅ Plugin components injected");
    }

    /// Get the plugin component registry.
    pub fn plugin_components(&self) -> &component_registry::PluginComponentRegistry {
        &self.plugin_components
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
