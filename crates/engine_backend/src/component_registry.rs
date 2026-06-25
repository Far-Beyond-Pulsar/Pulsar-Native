use std::any::Any;
use std::collections::HashMap;

pub type ComponentFactory = Box<dyn Fn() -> Box<dyn Any + Send + Sync> + Send + Sync>;

/// Runtime registry of plugin-provided component factories.
///
/// Plugin components are registered at startup via `inject_plugin_components()`
/// and are accessible globally through `EngineBackend::global()`.
/// Each entry maps a component class name to:
/// - A factory that creates a default instance (type-erased)
/// - Default serialized data (`serde_json::Value`) for scene storage
pub struct PluginComponentRegistry {
    factories: HashMap<String, (ComponentFactory, serde_json::Value)>,
}

impl PluginComponentRegistry {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    /// Register a single plugin component factory.
    pub fn register(
        &mut self,
        name: String,
        factory: ComponentFactory,
        default_data: serde_json::Value,
    ) {
        self.factories.entry(name).or_insert((factory, default_data));
    }

    /// Register multiple plugin component factories at once.
    pub fn register_all(
        &mut self,
        entries: Vec<(String, ComponentFactory, serde_json::Value)>,
    ) {
        for (name, factory, default_data) in entries {
            self.register(name, factory, default_data);
        }
    }

    /// Get the default serialized data for a component.
    pub fn get_default_data(&self, name: &str) -> Option<&serde_json::Value> {
        self.factories.get(name).map(|(_, data)| data)
    }

    /// Create a default instance of a plugin component.
    pub fn create_instance(&self, name: &str) -> Option<Box<dyn Any + Send + Sync>> {
        self.factories.get(name).map(|(factory, _)| factory())
    }

    /// Check if a plugin component is registered.
    pub fn has(&self, name: &str) -> bool {
        self.factories.contains_key(name)
    }

    /// Get all registered plugin component names.
    pub fn names(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.factories.keys().map(|s| s.as_str()).collect();
        names.sort();
        names
    }

    /// Number of registered plugin components.
    pub fn len(&self) -> usize {
        self.factories.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.factories.is_empty()
    }
}

impl Default for PluginComponentRegistry {
    fn default() -> Self {
        Self::new()
    }
}
