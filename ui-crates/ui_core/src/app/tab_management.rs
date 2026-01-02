//! Tab management and file opening logic
//!
//! This module uses the plugin system exclusively - NO match statements.
//! All editors (built-in and plugin-based) are handled through the trait system.

use std::path::PathBuf;
use gpui::{Context, Window};
use ui_file_manager::FileSelected;

use super::PulsarApp;

impl PulsarApp {
    /// Open a path in the appropriate editor using the plugin system.
    ///
    /// This is the ONLY method needed for file opening - the plugin system handles everything.
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        tracing::debug!("Opening path: {:?}", path);
        
        // Update plugin manager with current project root
        self.state.plugin_manager.set_project_root(self.state.project_path.clone());
        
        // Let the plugin system handle everything - no match statements needed!
        match self.state.plugin_manager.create_editor_for_file(&path, window, cx) {
            Ok((panel, _editor_instance)) => {
                tracing::debug!("Successfully created editor for: {:?}", path);
                self.state.center_tabs.update(cx, |tabs, cx| {
                    tabs.add_panel(panel, window, cx);
                });
            }
            Err(e) => {
                tracing::error!("Failed to open file {:?}: {}", path, e);
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

