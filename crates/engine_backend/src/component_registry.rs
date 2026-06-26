use pulsar_reflection::EngineClass;
use std::collections::HashMap;

pub type ComponentFactory = Box<dyn Fn() -> Box<dyn EngineClass> + Send + Sync>;

/// Runtime registry of plugin-provided component factories.
///
/// Plugin components are registered at startup via `inject_plugin_components()`
/// and are accessible globally through `EngineBackend::global()`.
///
/// Each factory returns a `Box<dyn EngineClass>` which provides full reflection
/// metadata, serialization, and property editing — identical to built-in
/// `#[engine_class]` components.
pub struct PluginComponentRegistry {
    factories: HashMap<String, ComponentFactory>,
}

impl PluginComponentRegistry {
    pub fn new() -> Self {
        Self {
            factories: HashMap::new(),
        }
    }

    /// Register a single plugin component factory.
    pub fn register(&mut self, name: String, factory: ComponentFactory) {
        self.factories.entry(name).or_insert(factory);
    }

    /// Register multiple plugin component factories at once.
    pub fn register_all(&mut self, entries: Vec<(String, ComponentFactory)>) {
        for (name, factory) in entries {
            self.register(name, factory);
        }
    }

    /// Create a default instance of a plugin component.
    ///
    /// Returns `None` if no factory is registered for the given name.
    pub fn create_instance(&self, name: &str) -> Option<Box<dyn EngineClass>> {
        self.factories.get(name).map(|factory| factory())
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
