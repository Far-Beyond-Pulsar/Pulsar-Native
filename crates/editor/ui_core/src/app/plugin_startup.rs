//! Editor plugin startup, off the UI thread.
//!
//! Loading a plugin library (OS loader, SHA-256 of the file, integrity
//! manifest) is slow and needs no UI, so every library is prepared in parallel
//! on worker threads while the window paints. Only running a plugin's own code
//! stays on the UI thread, one plugin per turn so frames keep flowing.
//!
//! Layout restore needs the plugins (they build the editors), so it runs when
//! this finishes: [`PulsarApp::plugins_ready`].

use std::time::{Duration, Instant};

use gpui::{Context, Window};
use plugin_manager::PluginManager;

use super::PulsarApp;

const PLUGINS_DIR: &str = "plugins/editor";

impl PulsarApp {
    /// Load the editor plugins into `manager` in the background, publish it as
    /// the global manager, then restore the layout. A window opened after the
    /// first finds the global manager already there and goes straight to ready.
    pub(super) fn start_plugin_load(
        &mut self,
        manager: PluginManager,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handle = window.window_handle();
        let load = plugin_manager::global().is_none();
        cx.spawn(async move |this, cx| {
            if load {
                let mut manager = manager;
                let started = Instant::now();
                let prepared = cx
                    .background_executor()
                    .spawn(async { PluginManager::prepare_plugins_in_dir(PLUGINS_DIR) })
                    .await;
                tracing::info!(
                    "[PulsarApp] {} plugin librar(ies) prepared in {:?}",
                    prepared.len(),
                    started.elapsed()
                );
                for (path, result) in prepared {
                    match result {
                        Ok(prepared) => {
                            match cx.update(|cx| manager.install_prepared_plugin(prepared, cx)) {
                                Ok(id) => tracing::info!("✅ Successfully loaded plugin: {id}"),
                                Err(e) => {
                                    tracing::error!("❌ Failed to load plugin {path:?}: {e}")
                                }
                            }
                        }
                        Err(e) => tracing::error!("❌ Failed to load plugin {path:?}: {e}"),
                    }
                    // One plugin per turn: let the window paint in between.
                    cx.background_executor()
                        .timer(Duration::from_millis(1))
                        .await;
                }
                cx.update(|cx| {
                    // Drain plugin subsystems and components into the engine
                    // backend before the manager becomes globally accessible.
                    let subsystems = manager.drain_subsystems();
                    if !subsystems.is_empty() {
                        match engine_backend::EngineBackend::global() {
                            Some(backend) => {
                                if let Err(e) =
                                    backend.update(|b| b.inject_plugin_subsystems(subsystems))
                                {
                                    tracing::error!("Failed to inject plugin subsystems: {}", e);
                                }
                            }
                            None => tracing::warn!(
                                "EngineBackend not yet available — plugin subsystems will not be registered"
                            ),
                        }
                    }
                    let components = manager.drain_component_registrations();
                    if !components.is_empty() {
                        match engine_backend::EngineBackend::global() {
                            Some(backend) => {
                                backend.update(|b| b.inject_plugin_components(components))
                            }
                            None => tracing::warn!(
                                "EngineBackend not yet available — plugin components will not be registered"
                            ),
                        }
                    }
                    plugin_manager::initialize_global(manager);
                });
                tracing::info!("[PulsarApp] plugins ready after {:?}", started.elapsed());
            }

            _ = cx.update(|cx| {
                handle.update(cx, |_, window, cx| {
                    _ = this.update(cx, |app, cx| app.plugins_ready(window, cx));
                })
            });
        })
        .detach();
    }

    /// The plugin registries are final: show what they provide, then restore
    /// the saved layout (which builds editors through them).
    fn plugins_ready(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut file_types: Vec<plugin_editor_api::FileTypeDefinition> = Vec::new();
        if let Some(manager) = plugin_manager::global() {
            file_types = manager
                .read()
                .file_type_registry()
                .get_all_file_types()
                .into_iter()
                .cloned()
                .collect();
        }

        // File creation choices come from the same plugin file-type registry
        // used by the file manager's New menu.
        let mut creatable: Vec<_> = file_types
            .iter()
            .filter(|file_type| file_type.is_creatable())
            .cloned()
            .collect();
        creatable.sort_by(|a, b| a.display_name.cmp(&b.display_name));
        if let Some(palette) = self.state.command_palette.clone() {
            palette.update(cx, |palette, cx| {
                for file_type in creatable {
                    let id = file_type.id.to_string();
                    palette.add_item_with_keywords(
                        format!("Create {}", file_type.display_name),
                        format!("Create a new .{} asset", file_type.extension),
                        file_type.icon,
                        "Create",
                        vec![file_type.extension, id.clone(), "new".into()],
                        move |window, cx| {
                            window.dispatch_action(
                                Box::new(ui_common::menu::NewAsset {
                                    file_type_id: id.clone(),
                                }),
                                cx,
                            );
                        },
                        cx,
                    );
                }
            });
        }

        self.state.file_manager_drawer.update(cx, |drawer, cx| {
            drawer.update_file_types(file_types);
            cx.notify();
        });

        if self.state.layout_persist {
            self.restore_layout_now(window, cx);
        }
    }
}
