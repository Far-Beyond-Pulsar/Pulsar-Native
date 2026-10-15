//! # Panel restorers
//!
//! A saved layout names a panel in one of two ways: the file it edits (rebuilt
//! through the editor factories) or a **restore key** `{ kind, data }` recorded
//! by [`PanelState::with_restore`](ui::dock::PanelState::with_restore). A
//! *restorer* is the function that turns a restore key back into a panel:
//! `kind -> fn(data, window, cx) -> Option<Arc<dyn PanelView>>`.
//!
//! ## How a plugin registers one
//!
//! Next to its editor factories, in
//! [`EditorPluginEditor::register_editors`](crate::EditorPluginEditor):
//!
//! ```rust,ignore
//! fn register_editors(&'static self, registry: &mut EditorFactoryRegistry) {
//!     registry.register_fn("my-editor".into(), |path, window, cx| { /* … */ });
//!     registry.register_restorer("my.preview", |data, window, cx| {
//!         let name = data.get("name")?.as_str()?;
//!         Some(Arc::new(cx.new(|cx| PreviewPanel::new(name, window, cx))))
//!     });
//! }
//! ```
//!
//! and its panel's `dump()` returns `PanelState::new(self).with_restore("my.preview", data)`.
//!
//! This rides the existing `register_editors` entry on purpose: a plugin
//! library links its own copy of this crate, so a process-wide registry here
//! would be the plugin's private copy and the host would never see it. The
//! registry is instead handed to the plugin by the host (like the editor
//! factory registry), the host collects the entries after `register_editors`
//! returns, and merges them into the one global `PanelRestorers` it owns
//! (`plugin_manager::panel_restorers()`). No new FFI symbol and no change to the
//! `EditorPluginFull` vtable: a plugin that registers no restorers loads exactly
//! as before.
//!
//! ## Contract
//!
//! - `kind` is globally unique; namespace it (`"<plugin-or-crate>.<panel>"`).
//!   A later registration of the same kind replaces the earlier one (a DLL
//!   plugin may replace a built-in, as for file types).
//! - `data` is opaque to the layout code. It is never path-rewritten; store
//!   project-relative or absolute paths yourself if you need them.
//! - Returning `None` means "this panel cannot be rebuilt (any more)" -- the tab
//!   is dropped. It is *not* "unknown kind"; an unregistered kind is kept
//!   dormant by the layout code so the tab returns when the plugin does.

use std::sync::Arc;

use gpui::{App, Window};
use serde_json::Value;
use ui::dock::PanelView;

/// Rebuilds one kind of panel from its saved `data`.
pub type PanelRestorer = Arc<
    dyn Fn(&Value, &mut Window, &mut App) -> Option<Arc<dyn PanelView>> + Send + Sync + 'static,
>;

/// The restorers one plugin registers, collected by the host.
#[derive(Default)]
pub struct PanelRestorerRegistry {
    entries: Vec<(String, PanelRestorer)>,
}

impl PanelRestorerRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register `restorer` for panels saved with restore kind `kind`.
    pub fn register(
        &mut self,
        kind: impl Into<String>,
        restorer: impl Fn(&Value, &mut Window, &mut App) -> Option<Arc<dyn PanelView>>
            + Send
            + Sync
            + 'static,
    ) {
        self.entries.push((kind.into(), Arc::new(restorer)));
    }

    /// The registered `(kind, restorer)` pairs, in registration order.
    pub fn entries(&self) -> &[(String, PanelRestorer)] {
        &self.entries
    }

    /// Take the registered pairs, leaving the registry empty.
    pub fn take(&mut self) -> Vec<(String, PanelRestorer)> {
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
    fn restorers_registered_beside_factories_are_taken_by_the_host() {
        let mut registry = EditorFactoryRegistry::new();
        registry.register_restorer("a.panel", |_, _, _| None);
        registry.register_restorer("b.panel", |_, _, _| None);

        let mut taken = registry.take_restorers();
        let kinds: Vec<&str> = taken.entries().iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(kinds, ["a.panel", "b.panel"]);
        assert_eq!(taken.take().len(), 2);
        assert!(taken.is_empty());
        assert!(registry.take_restorers().is_empty(), "taken only once");
    }
}
