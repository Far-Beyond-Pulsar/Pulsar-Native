//! # Agent harnesses from plugins
//!
//! A harness is a declarative agent definition: a markdown file with a small
//! frontmatter block and the system prompt as its body (see
//! `agent_runtime::harness`). A plugin registers harnesses as **source text**
//! next to its editor factories, in
//! [`EditorPluginEditor::register_editors`](crate::EditorPluginEditor):
//!
//! ```rust,ignore
//! fn register_editors(&'static self, registry: &mut EditorFactoryRegistry) {
//!     registry.register_harness(
//!         "shader-doctor",
//!         "---\nname: shader-doctor\ndescription: Diagnoses shader compile errors.\n---\nYou read shader sources...",
//!     );
//! }
//! ```
//!
//! Like panel restorers this rides `register_editors`: no FFI symbol, no vtable
//! change, and the registry is handed over by the host (a plugin links its own
//! copy of this crate, so a global here would be private to the plugin). The
//! host merges the entries into `plugin_manager::plugin_harnesses()`.
//!
//! The text is parsed by the agent runtime, not here, so this crate does not
//! depend on it. Precedence when the chat loads harnesses is
//! built-in < **plugin** < user-global < project: a project can always
//! override what a plugin ships. A plugin harness that fails to parse is
//! logged and skipped.

/// The harness sources one plugin registers, collected by the host.
#[derive(Default)]
pub struct HarnessSourceRegistry {
    entries: Vec<(String, String)>,
}

impl HarnessSourceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// `name` is the fallback name (the file stem) used when the frontmatter
    /// has none; `source` is the whole markdown file.
    pub fn register(&mut self, name: impl Into<String>, source: impl Into<String>) {
        self.entries.push((name.into(), source.into()));
    }

    pub fn entries(&self) -> &[(String, String)] {
        &self.entries
    }

    pub fn take(&mut self) -> Vec<(String, String)> {
        std::mem::take(&mut self.entries)
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditorFactoryRegistry;

    #[test]
    fn harnesses_registered_beside_factories_are_taken_by_the_host() {
        let mut registry = EditorFactoryRegistry::new();
        registry.register_harness("a", "---\nname: a\n---\nbody");
        registry.register_harness("b", "body");
        let mut taken = registry.take_harnesses();
        assert_eq!(taken.entries().len(), 2);
        assert_eq!(taken.take()[0].0, "a");
        assert!(registry.take_harnesses().is_empty(), "taken only once");
    }
}
