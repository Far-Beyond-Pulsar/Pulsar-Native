use pulsar_reflection::EngineClass as _;
use pulsar_reflection::{EngineClass, PropertyMetadata};

use crate::plugin::EditorPlugin;

// ============================================================================
// Extension Traits (optional capabilities)
// ============================================================================

/// Factory type for creating a default instance of a plugin-provided component.
///
/// The factory returns a `Box<dyn EngineClass>` which provides full reflection
/// metadata via `EngineClass::get_properties()`. This allows plugin components
/// to participate in the same serialization and property-editing pipeline as
/// built-in components derived with `#[engine_class]`.
///
/// # Safety
///
/// This closure lives in the plugin DLL. Because plugins are never unloaded,
/// the function pointer remains valid for the process lifetime.
pub type ComponentFactory = Box<dyn Fn() -> Box<dyn EngineClass> + Send + Sync>;

/// Definition of a custom engine component that a plugin provides.
///
/// Components are the building blocks of game objects in the Pulsar engine.
#[derive(Debug, Clone)]
pub struct ComponentDefinition {
    /// Unique identifier for this component type
    pub id: String,
    /// Human-readable name
    pub display_name: String,
    /// Category for organizing in the property panel (e.g., "Rendering", "Physics")
    pub category: String,
    /// Description of what this component does
    pub description: String,
    /// Optional icon for the component list
    pub icon: Option<ui::IconName>,
}

/// Trait for plugins that register custom engine components.
///
/// Plugins implementing this trait can provide custom component types that
/// behave identically to built-in engine components.
pub trait EditorPluginComponents: EditorPlugin {
    /// Returns all ComponentDefinitions for this plugin.
    fn component_definitions(&self) -> Vec<ComponentDefinition>;

    /// Returns factory functions for this plugin's components.
    ///
    /// Each entry maps a component class name (matching `ComponentDefinition.id`)
    /// to a factory that creates a default instance. The returned `EngineClass`
    /// provides full reflection metadata so the component can be serialized,
    /// displayed in the property panel, and edited — exactly like a built-in
    /// `#[engine_class]` component.
    ///
    /// Default: empty (component can only be referenced, not instantiated).
    fn component_factories(&self) -> Vec<(String, ComponentFactory)> {
        Vec::new()
    }
}
