//! The host-side set of panel restorers.
//!
//! `kind -> restorer` for every panel that a saved layout names by restore key
//! rather than by file (see `plugin_editor_api::panel_restore` for the SDK
//! side and the contract). One instance exists per process, owned by the host:
//!
//! - plugins add theirs through `EditorFactoryRegistry::register_restorer`; the
//!   plugin manager merges them in when the plugin loads;
//! - built-in code (`ui_core`, ...) calls [`PanelRestorers::register`] directly.
//!
//! Layout restore calls [`PanelRestorers::restore`] and distinguishes three
//! outcomes, because they mean different things for the saved tab:
//! [`Restored`](RestoreOutcome::Restored) (attach it),
//! [`Declined`](RestoreOutcome::Declined) (the kind is known but the panel is
//! gone; drop the tab) and [`UnknownKind`](RestoreOutcome::UnknownKind) (no
//! restorer registered, e.g. the plugin is disabled; keep the tab dormant).

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use gpui::{App, Window};
use parking_lot::RwLock;
use plugin_editor_api::PanelRestorer;
use serde_json::Value;
use ui::dock::PanelView;

/// What restoring one saved panel came to.
pub enum RestoreOutcome {
    Restored(Arc<dyn PanelView>),
    /// A restorer exists for the kind but could not rebuild this panel.
    Declined,
    /// No restorer is registered for the kind.
    UnknownKind,
}

/// The process-wide restorer registry. Get it with [`panel_restorers`].
#[derive(Default)]
pub struct PanelRestorers {
    by_kind: RwLock<HashMap<String, PanelRestorer>>,
}

impl PanelRestorers {
    /// Register `restorer` for `kind`. Returns `true` if it replaced an earlier
    /// registration of the same kind (the later one wins).
    pub fn register(
        &self,
        kind: impl Into<String>,
        restorer: impl Fn(&Value, &mut Window, &mut App) -> Option<Arc<dyn PanelView>>
            + Send
            + Sync
            + 'static,
    ) -> bool {
        self.register_shared(kind.into(), Arc::new(restorer))
    }

    /// [`Self::register`] for an already shared restorer (what a plugin hands over).
    pub fn register_shared(&self, kind: String, restorer: PanelRestorer) -> bool {
        self.by_kind.write().insert(kind, restorer).is_some()
    }

    pub fn contains(&self, kind: &str) -> bool {
        self.by_kind.read().contains_key(kind)
    }

    /// Rebuild the panel saved as `(kind, data)`.
    pub fn restore(
        &self,
        kind: &str,
        data: &Value,
        window: &mut Window,
        cx: &mut App,
    ) -> RestoreOutcome {
        // Clone the restorer out so the lock is not held while it runs (it may
        // itself register or restore).
        let restorer = self.by_kind.read().get(kind).cloned();
        match restorer {
            None => RestoreOutcome::UnknownKind,
            Some(restorer) => match restorer(data, window, cx) {
                Some(panel) => RestoreOutcome::Restored(panel),
                None => RestoreOutcome::Declined,
            },
        }
    }
}

/// The process-wide [`PanelRestorers`].
pub fn panel_restorers() -> &'static PanelRestorers {
    static RESTORERS: OnceLock<PanelRestorers> = OnceLock::new();
    RESTORERS.get_or_init(PanelRestorers::default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn later_registration_of_a_kind_replaces_the_earlier() {
        let restorers = PanelRestorers::default();
        assert!(!restorers.contains("k"));
        assert!(!restorers.register("k", |_, _, _| None));
        assert!(restorers.contains("k"));
        assert!(restorers.register("k", |_, _, _| None));
        assert!(!restorers.contains("other"));
    }
}
