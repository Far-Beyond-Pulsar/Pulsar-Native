//! The host-side set of agent harnesses that plugins ship.
//!
//! Plugins add theirs through `EditorFactoryRegistry::register_harness` (see
//! `plugin_editor_api::agent_harness` for the SDK side); the plugin manager
//! merges them in when a plugin loads. The agent chat reads
//! [`PluginHarnesses::snapshot`] and layers it below the user-global and the
//! project harness folders.

use std::sync::OnceLock;

use parking_lot::RwLock;

/// The process-wide harness sources. Get it with [`plugin_harnesses`].
#[derive(Default)]
pub struct PluginHarnesses {
    /// `(fallback name, markdown source)`, in registration order.
    entries: RwLock<Vec<(String, String)>>,
}

impl PluginHarnesses {
    /// Add a harness source. Returns `true` if it replaced an earlier one with
    /// the same fallback name (the later registration wins).
    pub fn register(&self, name: impl Into<String>, source: impl Into<String>) -> bool {
        let (name, source) = (name.into(), source.into());
        let mut entries = self.entries.write();
        match entries.iter_mut().find(|(existing, _)| *existing == name) {
            Some(entry) => {
                entry.1 = source;
                true
            }
            None => {
                entries.push((name, source));
                false
            }
        }
    }

    /// Every registered `(name, source)` pair, in registration order.
    pub fn snapshot(&self) -> Vec<(String, String)> {
        self.entries.read().clone()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.read().is_empty()
    }
}

/// The process-wide [`PluginHarnesses`].
pub fn plugin_harnesses() -> &'static PluginHarnesses {
    static HARNESSES: OnceLock<PluginHarnesses> = OnceLock::new();
    HARNESSES.get_or_init(PluginHarnesses::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_registration_of_a_name_wins() {
        let set = PluginHarnesses::default();
        assert!(!set.register("a", "one"));
        assert!(!set.register("b", "two"));
        assert!(set.register("a", "three"));
        assert_eq!(
            set.snapshot(),
            vec![("a".to_string(), "three".to_string()), ("b".to_string(), "two".to_string())]
        );
    }
}
