use dashmap::DashMap;
use gpui::{App, Global};
use std::sync::Arc;

type Opener = Arc<dyn Fn(&mut App) + Send + Sync>;

/// Global registry mapping window names to their openers.
///
/// Zero-param windows register themselves via [`PulsarWindowExt::register`]; parameterised
/// windows can register custom openers manually. Any code can then open a window by name
/// without knowing the concrete type:
///
/// ```ignore
/// WindowRegistry::update_global(cx, |reg, cx| reg.open("SettingsWindow", cx));
/// ```
pub struct WindowRegistry {
    openers: DashMap<&'static str, Opener>,
}

impl WindowRegistry {
    pub fn new() -> Self {
        Self { openers: DashMap::new() }
    }

    /// Register an opener for `name`. Overwrites any previous registration.
    pub fn register(&self, name: &'static str, opener: impl Fn(&mut App) + Send + Sync + 'static) {
        self.openers.insert(name, Arc::new(opener));
        tracing::debug!("[WindowRegistry] registered '{}'", name);
    }

    /// Open the window registered under `name`. Logs a warning if not found.
    pub fn open(&self, name: &'static str, cx: &mut App) {
        match self.openers.get(name) {
            Some(opener) => {
                tracing::debug!("[WindowRegistry] opening '{}'", name);
                opener(cx);
            }
            None => tracing::warn!("[WindowRegistry] no opener registered for '{}'", name),
        }
    }

    pub fn is_registered(&self, name: &'static str) -> bool {
        self.openers.contains_key(name)
    }

    /// All currently registered window names — useful for dev tools / palette.
    pub fn registered_names(&self) -> Vec<&'static str> {
        self.openers.iter().map(|e| *e.key()).collect()
    }
}

impl Default for WindowRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl Global for WindowRegistry {}
