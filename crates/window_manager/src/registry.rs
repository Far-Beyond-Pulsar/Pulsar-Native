use dashmap::DashMap;
use gpui::{App, Global};
use std::sync::Arc;

/// Distributed slice populated by `#[window_manager::register_window]` in each window crate.
/// `register_all_windows` iterates it once during app startup.
#[linkme::distributed_slice]
pub static WINDOW_REGISTRANTS: [fn(&mut App)];


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

    /// Open the window registered under `name`. Logs timing at INFO level.
    pub fn open(&self, name: &'static str, cx: &mut App) {
        match self.openers.get(name) {
            Some(opener) => {
                let t0 = std::time::Instant::now();
                tracing::info!("[WindowRegistry] opening '{}'", name);
                opener(cx);
                let elapsed = t0.elapsed();
                if elapsed.as_millis() > 50 {
                    tracing::warn!("[WindowRegistry] '{}' opener took {:?} (slow)", name, elapsed);
                } else {
                    tracing::info!("[WindowRegistry] '{}' opened in {:?}", name, elapsed);
                }
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
