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
use std::sync::{Arc, OnceLock};
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

/// Global instance handle, set once during engine init.
static GLOBAL_BACKEND: OnceLock<engine_state::ResourceHandle<EngineBackend>> = OnceLock::new();

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
    /// Initialize engine backend with empty subsystem registry.
    ///
    /// All subsystems (including built-in ones like PhysicsEngine) are
    /// registered later via the plugin pipeline so that everything — built-in
    /// and DLL-provided — goes through the same injection and initialization
    /// path. See `inject_plugin_subsystems()` and `inject_plugin_components()`.
    pub async fn init() -> Self {
        profiling::profile_scope!("EngineBackend::Init");
        tracing::debug!("Initializing Engine Backend (empty subsystem registry)");

        EngineBackend {
            subsystems: SubsystemRegistry::new(),
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

        let context = SubsystemContext::new();

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
    /// Each entry is `(class_name, factory)`.
    pub fn inject_plugin_components(
        &mut self,
        registrations: Vec<(String, component_registry::ComponentFactory)>,
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
        use engine_subsystems::SubsystemId;

        const PHYSICS: SubsystemId = SubsystemId::new("physics");

        self.subsystems.get(PHYSICS).and_then(|subsystem| {
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
        let handle = engine_state::ResourceHandle::new(backend);
        let _ = GLOBAL_BACKEND.set(handle);
    }

    /// Get global instance
    pub fn global() -> Option<&'static engine_state::ResourceHandle<EngineBackend>> {
        GLOBAL_BACKEND.get()
    }
}
