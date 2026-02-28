//! Typed Renderer Registry
//!
//! Replaces the Arc<dyn Any> renderer registry with a type-safe enum-based system.
//! This eliminates runtime downcasting errors and provides compile-time type safety.

use std::sync::Arc;
use std::sync::Mutex;
use dashmap::DashMap;

use crate::context::WindowId;

/// Type-safe renderer handle using an enum instead of Arc<dyn Any>
///
/// This allows safe downcasting with compile-time type checking instead of
/// runtime type_id comparisons.
#[derive(Clone)]
pub enum RendererType {
    /// Bevy renderer (D3D12-based, used for 3D viewports)
    Bevy(Arc<dyn std::any::Any + Send + Sync>),

    /// WGPU renderer (cross-platform, future renderer option)
    Wgpu(Arc<dyn std::any::Any + Send + Sync>),

    /// Placeholder for custom renderers from plugins
    Custom {
        name: String,
        renderer: Arc<dyn std::any::Any + Send + Sync>,
    },
}

impl RendererType {
    /// Get as Bevy renderer if this is a Bevy variant
    pub fn as_bevy<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        match self {
            RendererType::Bevy(renderer) => renderer.clone().downcast::<T>().ok(),
            _ => None,
        }
    }

    /// Get as WGPU renderer if this is a WGPU variant
    pub fn as_wgpu<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        match self {
            RendererType::Wgpu(renderer) => renderer.clone().downcast::<T>().ok(),
            _ => None,
        }
    }

    /// Get as custom renderer with dynamic casting (for plugins)
    pub fn as_custom<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        match self {
            RendererType::Custom { renderer, .. } => {
                renderer.clone().downcast::<T>().ok()
            }
            _ => None,
        }
    }

    /// Get the name of this renderer type
    pub fn name(&self) -> &str {
        match self {
            RendererType::Bevy(_) => "Bevy",
            RendererType::Wgpu(_) => "WGPU",
            RendererType::Custom { name, .. } => name,
        }
    }
}

/// Typed renderer handle with window association
#[derive(Clone)]
pub struct TypedRendererHandle {
    /// The renderer type and instance
    pub renderer_type: RendererType,
    /// Window this renderer is associated with
    pub window_id: WindowId,
}

impl TypedRendererHandle {
    /// Create a new typed renderer handle
    pub fn new(window_id: WindowId, renderer_type: RendererType) -> Self {
        Self {
            renderer_type,
            window_id,
        }
    }

    /// Convenience method to create a Bevy renderer handle
    pub fn bevy<T: Send + Sync + 'static>(
        window_id: WindowId,
        renderer: Arc<T>,
    ) -> Self {
        Self::new(window_id, RendererType::Bevy(renderer))
    }

    /// Convenience method to create a WGPU renderer handle
    pub fn wgpu<T: Send + Sync + 'static>(
        window_id: WindowId,
        renderer: Arc<T>,
    ) -> Self {
        Self::new(window_id, RendererType::Wgpu(renderer))
    }

    /// Convenience method to create a custom renderer handle
    pub fn custom(
        window_id: WindowId,
        name: String,
        renderer: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Self {
        Self::new(window_id, RendererType::Custom { name, renderer })
    }

    /// Get as Bevy renderer (type-safe, no runtime errors)
    pub fn as_bevy<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.renderer_type.as_bevy()
    }

    /// Get as WGPU renderer (type-safe, no runtime errors)
    pub fn as_wgpu<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.renderer_type.as_wgpu()
    }

    /// Get as custom renderer
    pub fn as_custom<T: Send + Sync + 'static>(&self) -> Option<Arc<T>> {
        self.renderer_type.as_custom::<T>()
    }
}

/// Registry for typed renderer handles
///
/// Replaces the old RendererRegistry that used Arc<dyn Any> with a type-safe version.
#[derive(Clone)]
pub struct TypedRendererRegistry {
    renderers: Arc<DashMap<u64, TypedRendererHandle>>,
}

impl TypedRendererRegistry {
    /// Create a new typed renderer registry
    pub fn new() -> Self {
        Self {
            renderers: Arc::new(DashMap::new()),
        }
    }

    /// Register a renderer for a window (using u64 ID for compatibility)
    pub fn register(&self, window_id: u64, handle: TypedRendererHandle) {
        let renderer_name = handle.renderer_type.name().to_string();
        self.renderers.insert(window_id, handle);
        tracing::debug!("Registered {} renderer for window {}", renderer_name, window_id);
    }

    /// Get a renderer for a window
    pub fn get(&self, window_id: u64) -> Option<TypedRendererHandle> {
        self.renderers.get(&window_id).map(|entry| entry.value().clone())
    }

    /// Unregister a renderer
    pub fn unregister(&self, window_id: u64) -> Option<TypedRendererHandle> {
        self.renderers.remove(&window_id).map(|(_, handle)| {
            tracing::debug!("Unregistered {} renderer for window {}", handle.renderer_type.name(), window_id);
            handle
        })
    }

    /// Check if a window has a registered renderer
    pub fn has_renderer(&self, window_id: u64) -> bool {
        self.renderers.contains_key(&window_id)
    }

    /// Get all registered window IDs
    pub fn window_ids(&self) -> Vec<u64> {
        self.renderers.iter().map(|entry| *entry.key()).collect()
    }

    /// Clear all renderers
    pub fn clear(&self) {
        self.renderers.clear();
    }
}

impl Default for TypedRendererRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Migration helper to convert from old RendererHandle (Arc<dyn Any>) to TypedRendererHandle
///
/// During the transition period, code may need to work with both old and new renderer types.
pub mod migration {
    use super::*;

    /// Try to convert old Arc<dyn Any> renderer handle to typed handle
    ///
    /// This is used during migration when we receive an old-style renderer handle
    /// and need to convert it to the new typed system.
    pub fn from_any_bevy<T: Send + Sync + 'static>(
        window_id: WindowId,
        handle: Arc<dyn std::any::Any + Send + Sync>,
    ) -> Option<TypedRendererHandle> {
        handle
            .downcast::<T>()
            .ok()
            .map(|renderer| TypedRendererHandle::bevy(window_id, renderer))
    }

    /// Convert u64 window ID to WindowId
    ///
    /// This is a no-op since WindowId is now defined as u64 in engine_state.
    /// Kept for API compatibility during migration.
    pub fn u64_to_window_id(id: u64) -> WindowId {
        id
    }

    /// Convert WindowId to u64
    ///
    /// This is a no-op since WindowId is now defined as u64 in engine_state.
    /// Kept for API compatibility during migration.
    pub fn window_id_to_u64(window_id: WindowId) -> u64 {
        window_id
    }
}