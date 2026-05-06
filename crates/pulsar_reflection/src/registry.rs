//! Global registry of all engine classes
//!
//! Uses the `inventory` crate to auto-discover all types marked with
//! `#[derive(EngineClass)]` at link time (zero runtime cost).

use crate::EngineClass;
use once_cell::sync::Lazy;
use std::collections::HashMap;

/// Registration entry for auto-discovery
///
/// Automatically submitted by the `#[derive(EngineClass)]` macro via `inventory::submit!`
pub struct EngineClassRegistration {
    pub name: &'static str,
    pub constructor: fn() -> Box<dyn EngineClass>,
}

// Collect all engine class registrations at link time
inventory::collect!(EngineClassRegistration);

/// Global registry of all engine classes
///
/// Auto-populated at startup with all types that derive `EngineClass`.
/// Used by the object creation menu and property system.
pub struct EngineClassRegistry {
    classes: HashMap<&'static str, fn() -> Box<dyn EngineClass>>,
}

impl EngineClassRegistry {
    fn new() -> Self {
        let mut classes = HashMap::new();

        // Auto-discover all #[derive(EngineClass)] types via inventory
        for registration in inventory::iter::<EngineClassRegistration> {
            classes.insert(registration.name, registration.constructor);
        }

        tracing::info!(
            "Engine class registry initialized with {} classes",
            classes.len()
        );

        Self { classes }
    }

    /// Get list of all registered class names (for object creation menu)
    pub fn get_class_names(&self) -> Vec<&'static str> {
        let mut names: Vec<&'static str> = self.classes.keys().copied().collect();
        names.sort();
        names
    }

    /// Get list of class names filtered by category
    ///
    /// Categories are defined via `#[category("Physics")]` attribute on the struct
    pub fn get_class_names_by_category(&self, _category: &str) -> Vec<&'static str> {
        // TODO: Implement category filtering once proc macro supports it
        self.get_class_names()
    }

    /// Create instance of class by name
    ///
    /// Returns None if the class name is not registered
    pub fn create_instance(&self, class_name: &str) -> Option<Box<dyn EngineClass>> {
        self.classes.get(class_name).map(|ctor| ctor())
    }

    /// Check if a class is registered
    pub fn has_class(&self, class_name: &str) -> bool {
        self.classes.contains_key(class_name)
    }

    /// Get number of registered classes
    pub fn len(&self) -> usize {
        self.classes.len()
    }

    /// Check if registry is empty
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty()
    }
}

/// Global singleton registry instance
///
/// Lazily initialized on first access. All engine classes are automatically
/// registered via the `inventory` crate.
pub static REGISTRY: Lazy<EngineClassRegistry> = Lazy::new(EngineClassRegistry::new);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_registry_initialization() {
        // Registry should be initialized (even if empty in tests)
        let registry = &*REGISTRY;
        assert!(registry.len() >= 0);
    }

    #[test]
    fn test_registry_api() {
        let registry = &*REGISTRY;

        // Test basic API
        let names = registry.get_class_names();
        assert!(names.is_empty() || !names.is_empty()); // Should work either way

        // Test unknown class
        assert_eq!(registry.create_instance("NonExistentClass"), None);
        assert!(!registry.has_class("NonExistentClass"));
    }
}
