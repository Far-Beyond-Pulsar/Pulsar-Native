//! Tab management and file opening logic
//!
//! This module uses the plugin system exclusively - NO match statements.
//! All editors (built-in and plugin-based) are handled through the trait system.

use gpui::{App, Context, Window};
use std::path::PathBuf;
use ui_file_manager::FileSelected;

use super::{open_editors::OpenEditorInfo, PulsarApp};

impl PulsarApp {
    pub(crate) fn refresh_open_editor_snapshot(&self, cx: &App) {
        let active_index = self.state.center_tabs.read(cx).active_tab_index();
        let snapshot = self
            .state
            .center_tabs
            .read(cx)
            .all_panels()
            .into_iter()
            .enumerate()
            .map(|(index, panel)| {
                let panel_name = panel.panel_name(cx).to_string();
                let tab_name = panel
                    .tab_name(cx)
                    .map(|name| name.to_string())
                    .unwrap_or_else(|| panel_name.clone());
                let file_path = panel.panel_file_path(cx).map(|p| p.display().to_string());
                OpenEditorInfo {
                    index,
                    panel_name,
                    tab_name,
                    is_active: active_index == Some(index),
                    file_path,
                }
            })
            .collect::<Vec<_>>();

        super::open_editors::set_snapshot(snapshot);
    }

    /// Open a path in the appropriate editor using the plugin system.
    ///
    /// This is the ONLY method needed for file opening - the plugin system handles everything.
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        tracing::debug!("Opening path: {:?}", path);

        // Update plugin manager with current project root
        if let Some(pm_lock) = plugin_manager::global() {
            if let Ok(mut pm) = pm_lock.write() {
                pm.set_project_root(self.state.project_path.clone());

                // Let the plugin system handle everything - no match statements needed!
                match pm.create_editor_for_file(&path, window, cx) {
                    Ok(panel) => {
                        tracing::debug!("Successfully created editor for: {:?}", path);
                        self.state.center_tabs.update(cx, |tabs, cx| {
                            tabs.add_panel(panel, window, cx);
                        });
                        self.refresh_open_editor_snapshot(cx);
                    }
                    Err(e) => {
                        tracing::error!("Failed to open file {:?}: {}", path, e);
                    }
                }
            }
        }
    }

    /// Handle file selected events from the file manager.
    pub fn handle_file_selected_from_external_window(
        &mut self,
        event: &FileSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::debug!("File selected from external window: {:?}", event.path);
        self.open_path(event.path.clone(), window, cx);
    }
}
