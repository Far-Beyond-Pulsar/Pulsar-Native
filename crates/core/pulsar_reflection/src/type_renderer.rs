//! Custom type rendering system for plugin extensibility
//!
//! Allows plugins to register custom property editors for their types without
//! modifying the core engine. This is the key to true plugin extensibility.
//!
//! # Architecture
//!
//! - Each type can have ONE registered renderer
//! - Renderers are registered at startup via inventory
//! - The property panel checks for custom renderers before using default rendering
//!
//! # Example
//!
//! ```ignore
//! use pulsar_reflection::type_renderer::*;
//! use pulsar_reflection::RuntimeTypeInfo;
//! use std::any::Any;
//! use std::sync::Arc;
//!
//! // Define a custom renderer for your type
//! struct ColorPickerRenderer;
//!
//! impl TypeRenderer for ColorPickerRenderer {
//!     fn can_render(&self, type_info: &RuntimeTypeInfo) -> bool {
//!         type_info.type_name == "my_plugin::Color"
//!     }
//!
//!     fn render(&self, ui_context: &mut dyn Any, value: &mut dyn Any, type_info: &RuntimeTypeInfo) -> RenderResult {
//!         // Downcast to your UI framework's context
//!         // Downcast value to your type
//!         // Render custom color picker UI
//!         // Return whether the value changed
//!         RenderResult::Unchanged
//!     }
//! }
//!
//! // Register the renderer (usually done in plugin init)
//! inventory::submit! {
//!     TypeRendererRegistration::new(
//!         TypeId::of::<my_plugin::Color>(),
//!         Arc::new(ColorPickerRenderer)
//!     )
//! }
//! ```

use crate::runtime_types::RuntimeTypeInfo;
use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

/// Result of a rendering operation
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderResult {
    /// Value was changed by the UI
    Changed,
    /// Value was not changed
    Unchanged,
}

/// Trait for custom property renderers
///
/// Implement this trait to provide custom UI for your types in the property inspector.
/// The UI context is passed as `dyn Any` to remain framework-agnostic.
pub trait TypeRenderer: Send + Sync {
    /// Check if this renderer can handle the given type
    fn can_render(&self, type_info: &RuntimeTypeInfo) -> bool;

    /// Render the property editor UI
    ///
    /// # Arguments
    ///
    /// - `ui_context`: UI framework context (e.g., egui::Ui, gpui::WindowContext)
    /// - `value`: The value to edit (must be downcast to concrete type)
    /// - `type_info`: Runtime type information
    ///
    /// # Returns
    ///
    /// `RenderResult::Changed` if the value was modified, `RenderResult::Unchanged` otherwise
    fn render(
        &self,
        ui_context: &mut dyn Any,
        value: &mut dyn Any,
        type_info: &RuntimeTypeInfo,
    ) -> RenderResult;
}

/// Registration entry for type renderers (collected via inventory)
pub struct TypeRendererRegistration {
    pub type_id: TypeId,
    pub renderer: Arc<dyn TypeRenderer>,
}

impl TypeRendererRegistration {
    /// Create a new renderer registration
    pub fn new(type_id: TypeId, renderer: Arc<dyn TypeRenderer>) -> Self {
        Self { type_id, renderer }
    }
}

// Collect type renderer registrations at link-time
inventory::collect!(TypeRendererRegistration);

/// Global registry of custom type renderers
pub struct TypeRendererRegistry {
    renderers: HashMap<TypeId, Arc<dyn TypeRenderer>>,
}

impl TypeRendererRegistry {
    /// Create a new registry from inventory
    fn new() -> Self {
        let mut renderers = HashMap::new();

        // Auto-discover all type renderer registrations
        for registration in inventory::iter::<TypeRendererRegistration> {
            renderers.insert(registration.type_id, registration.renderer.clone());
        }

        tracing::info!(
            "Type renderer registry initialized with {} custom renderers",
            renderers.len()
        );

        Self { renderers }
    }

    /// Get a custom renderer for the given type, if one is registered
    pub fn get_renderer(&self, type_id: TypeId) -> Option<&dyn TypeRenderer> {
        self.renderers.get(&type_id).map(|b| &**b)
    }

    /// Check if a custom renderer is registered for the given type
    pub fn has_renderer(&self, type_id: TypeId) -> bool {
        self.renderers.contains_key(&type_id)
    }

    /// Get a custom renderer by runtime type info
    pub fn get_renderer_for_type(&self, type_info: &RuntimeTypeInfo) -> Option<&dyn TypeRenderer> {
        self.get_renderer(type_info.type_id)
    }

    /// Manually register a renderer (for testing or dynamic registration)
    ///
    /// Note: Prefer using inventory::submit! for static registration
    pub fn register(&mut self, type_id: TypeId, renderer: Arc<dyn TypeRenderer>) {
        self.renderers.insert(type_id, renderer);
    }

    /// Get number of registered renderers
    pub fn len(&self) -> usize {
        self.renderers.len()
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        self.renderers.is_empty()
    }
}

/// Global singleton registry instance
///
/// Lazily initialized on first access. Custom renderers are automatically
/// registered via the `inventory` crate.
pub static TYPE_RENDERER_REGISTRY: LazyLock<Mutex<TypeRendererRegistry>> =
    LazyLock::new(|| Mutex::new(TypeRendererRegistry::new()));

/// Helper function to register a custom renderer at runtime
///
/// This is provided for convenience, but prefer using inventory::submit!
/// for static registration.
pub fn register_type_renderer(type_id: TypeId, renderer: Arc<dyn TypeRenderer>) {
    TYPE_RENDERER_REGISTRY
        .lock()
        .unwrap()
        .register(type_id, renderer);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestRenderer;

    impl TypeRenderer for TestRenderer {
        fn can_render(&self, type_info: &RuntimeTypeInfo) -> bool {
            type_info.type_name == "test::TestType"
        }

        fn render(
            &self,
            _ui_context: &mut dyn Any,
            _value: &mut dyn Any,
            _type_info: &RuntimeTypeInfo,
        ) -> RenderResult {
            RenderResult::Unchanged
        }
    }

    #[test]
    fn test_renderer_registry() {
        let mut registry = TypeRendererRegistry::new();
        assert_eq!(registry.len(), 0);

        let test_type_id = TypeId::of::<i32>();
        registry.register(test_type_id, Arc::new(TestRenderer));

        assert_eq!(registry.len(), 1);
        assert!(registry.has_renderer(test_type_id));
        assert!(registry.get_renderer(test_type_id).is_some());
    }

    #[test]
    fn test_global_registry() {
        // The global registry should be accessible
        let registry = TYPE_RENDERER_REGISTRY.lock().unwrap();
        let _ = registry.len(); // Should not panic
    }
}
