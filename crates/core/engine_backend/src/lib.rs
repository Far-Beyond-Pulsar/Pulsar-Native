//! # Pulsar Engine Backend
//!
//! This crate provides the backend functionalities for the Pulsar game engine, including
//! rendering, asset management, and core engine systems.
//! It is designed to be modular and extensible, allowing developers to
//! build high-performance games with ease.

#![warn(clippy::undocumented_unsafe_blocks)]

pub mod component_registry;
/// Persistent GPU objects (bind group cache, readback ring, upload pool).
#[cfg(any(feature = "render", feature = "pie"))]
pub mod gpu;
pub mod scene;
pub mod services;
pub mod subsystems;

// Force-link `helio_component`: this crate relies on its `inventory`-based
// registrations (the motion gate, texture reloads), but does not name every
// `helio_component` item directly. Without an explicit symbol reference the
// linker can dead-strip the rest of `helio_component`'s `#[used]` inventory
// statics in an optimized build.
use helio_component as _;
pub use scene::{component_asset_events, ComponentEventCatalog};

#[cfg(feature = "render")]
pub use services::GpuRenderer;
#[cfg(feature = "editor-ui")]
pub use services::RustAnalyzerManager;
use std::sync::{Arc, OnceLock};
#[cfg(feature = "render")]
pub use subsystems::render::{Framebuffer as RenderFramebuffer, WgpuRenderer};
pub use subsystems::world::World;

// Re-export Helio types for UI integration
#[cfg(feature = "render")]
pub use scene::GizmoType as GizmoMode;

// Re-export reflection system for convenience
pub use pulsar_reflection::*;

// Re-export scene types used by UI crates
pub use scene::{ComponentInstance, EditorObjectId};

/// Global instance handle, set once during engine init.
static GLOBAL_BACKEND: OnceLock<engine_state::ResourceHandle<EngineBackend>> = OnceLock::new();

pub struct EngineBackend {
    plugin_components: component_registry::PluginComponentRegistry,
}

impl EngineBackend {
    /// Initialize the engine backend; plugin components arrive later through
    /// `inject_plugin_components()`.
    pub async fn init() -> Self {
        profiling::profile_scope!("EngineBackend::Init");
        tracing::debug!("Initializing Engine Backend");

        EngineBackend {
            plugin_components: component_registry::PluginComponentRegistry::new(),
        }
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
