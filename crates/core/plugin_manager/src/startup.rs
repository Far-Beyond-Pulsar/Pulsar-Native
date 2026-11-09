//! Startup hand-offs between the splash screen and the editor window.
//!
//! The splash prepares plugin libraries on worker threads ([`prewarm_plugins`]),
//! installs them into a manager and hands it over ([`stage_manager`]); the
//! editor then takes both and runs each plugin's preload in parallel
//! ([`publish_preload_targets`], [`run_plugin_preloads`]).

use crate::{PluginManager, PluginManagerError, PreparedPlugin};
use plugin_editor_api::*;
use std::path::{Path, PathBuf};

pub type PrewarmedPlugins = Vec<(PathBuf, Result<PreparedPlugin, PluginManagerError>)>;

static PREWARMED_PLUGINS: parking_lot::Mutex<Option<PrewarmedPlugins>> =
    parking_lot::Mutex::new(None);

/// Prepare every plugin library in `dir` now (in parallel) and keep the result
/// for [`take_prewarmed_plugins`]. Meant for the splash screen, so the editor
/// window only has to install them. Blocks until all libraries are prepared.
pub fn prewarm_plugins(dir: impl AsRef<Path>) -> usize {
    let prepared = PluginManager::prepare_plugins_in_dir(dir);
    let count = prepared.len();
    *PREWARMED_PLUGINS.lock() = Some(prepared);
    count
}

/// The libraries [`prewarm_plugins`] prepared, once. `None` if it never ran.
pub fn take_prewarmed_plugins() -> Option<PrewarmedPlugins> {
    PREWARMED_PLUGINS.lock().take()
}

// ============================================================================
// Splash-screen staging
// ============================================================================

thread_local! {
    /// A manager the splash screen filled with plugins, waiting for the editor.
    /// Only the UI thread touches it (the splash and the editor both live
    /// there), so it needs no `Send`.
    static STAGED_MANAGER: std::cell::RefCell<Option<PluginManager>> =
        const { std::cell::RefCell::new(None) };
}

/// Hand a manager whose plugins are installed to the editor that opens next.
pub fn stage_manager(manager: PluginManager) {
    STAGED_MANAGER.with(|slot| *slot.borrow_mut() = Some(manager));
}

/// The manager [`stage_manager`] left, once. `None` if the splash did not run.
pub fn take_staged_manager() -> Option<PluginManager> {
    STAGED_MANAGER.with(|slot| slot.borrow_mut().take())
}

type PreloadTargets = Vec<(PluginId, &'static dyn EditorPluginFull)>;

static PRELOAD_TARGETS: (std::sync::Mutex<Option<PreloadTargets>>, std::sync::Condvar) =
    (std::sync::Mutex::new(None), std::sync::Condvar::new());

/// Say which plugins are installed, releasing [`run_plugin_preloads`].
pub fn publish_preload_targets(manager: &PluginManager) {
    let targets: PreloadTargets = manager
        .plugins
        .iter()
        .map(|(id, loaded)| (id.clone(), loaded.plugin))
        .collect();
    if let Ok(mut slot) = PRELOAD_TARGETS.0.lock() {
        *slot = Some(targets);
    }
    PRELOAD_TARGETS.1.notify_all();
}

/// Let every installed plugin preload for the project (see
/// [`EditorPlugin::preload`]), all at once on worker threads. Waits up to
/// `wait` for the plugins to be installed first (the UI thread does that), and
/// does nothing if they never are. A plugin that panics is skipped.
pub fn run_plugin_preloads(
    ctx: &PreloadContext<'_>,
    wait: std::time::Duration,
) -> Vec<(PluginId, Option<String>)> {
    let targets = {
        let Ok(guard) = PRELOAD_TARGETS.0.lock() else {
            return Vec::new();
        };
        match PRELOAD_TARGETS
            .1
            .wait_timeout_while(guard, wait, |targets| targets.is_none())
        {
            Ok((mut guard, _)) => guard.take(),
            Err(_) => None,
        }
    };
    let Some(targets) = targets else {
        tracing::warn!("plugins were not installed in time; skipping plugin preloads");
        return Vec::new();
    };
    std::thread::scope(|scope| {
        let workers: Vec<_> = targets
            .iter()
            .map(|(id, plugin)| {
                let plugin = *plugin;
                (id.clone(), scope.spawn(move || plugin.preload(ctx)))
            })
            .collect();
        workers
            .into_iter()
            .map(|(id, worker)| (id, worker.join().ok().flatten()))
            .collect()
    })
}
