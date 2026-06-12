//! Tab management and file opening logic
//!
//! This module uses the plugin system exclusively - NO match statements.
//! All editors (built-in and plugin-based) are handled through the trait system.

use gpui::{App, Context, Entity, Window};
use std::path::PathBuf;
use ui::dock::{DockItem, TabPanel};
use ui_file_manager::FileSelected;

use super::{open_editors::OpenEditorInfo, PulsarApp};

impl PulsarApp {
    pub(crate) fn refresh_open_editor_snapshot(&self, cx: &App) {
        let mut snapshot = Vec::new();
        let mut global_index = 0usize;

        fn visit_item(
            item: &DockItem,
            snapshot: &mut Vec<OpenEditorInfo>,
            global_index: &mut usize,
            cx: &App,
        ) {
            match item {
                DockItem::Split { items, .. } => {
                    for child in items {
                        visit_item(child, snapshot, global_index, cx);
                    }
                }
                DockItem::Tabs { view, .. } => {
                    let active_local = view.read(cx).active_tab_index();
                    let panels = view.read(cx).all_panels();
                    for (local_ix, panel) in panels.into_iter().enumerate() {
                        let panel_name = panel.panel_name(cx).to_string();
                        let tab_name = panel
                            .tab_name(cx)
                            .map(|name| name.to_string())
                            .unwrap_or_else(|| panel_name.clone());
                        let file_path = panel.panel_file_path(cx).map(|p| p.display().to_string());
                        snapshot.push(OpenEditorInfo {
                            index: *global_index,
                            panel_name,
                            tab_name,
                            is_active: active_local == Some(local_ix),
                            file_path,
                        });
                        *global_index += 1;
                    }
                }
                DockItem::Tiles { .. } | DockItem::Panel { .. } => {}
            }
        }

        let items = {
            let dock = self.state.dock_area.read(cx);
            dock.items().clone()
        };
        visit_item(&items, &mut snapshot, &mut global_index, cx);

        super::open_editors::set_snapshot(snapshot);
    }

    pub(crate) fn activate_open_editor_by_global_index(
        &self,
        target_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let mut current_index = 0usize;

        fn find_and_activate(
            item: &DockItem,
            current_index: &mut usize,
            target_index: usize,
            window: &mut Window,
            cx: &mut Context<PulsarApp>,
        ) -> bool {
            match item {
                DockItem::Split { items, .. } => {
                    for child in items {
                        if find_and_activate(child, current_index, target_index, window, cx) {
                            return true;
                        }
                    }
                    false
                }
                DockItem::Tabs { view, .. } => {
                    let panels = view.read(cx).all_panels();
                    for (local_ix, _panel) in panels.into_iter().enumerate() {
                        if *current_index == target_index {
                            // Found it - activate this tab in this TabPanel
                            view.update(cx, |tab_panel, cx| {
                                tab_panel.set_active_tab(local_ix, window, cx);
                            });
                            return true;
                        }
                        *current_index += 1;
                    }
                    false
                }
                DockItem::Tiles { .. } | DockItem::Panel { .. } => false,
            }
        }

        let items = {
            let dock = self.state.dock_area.read(cx);
            dock.items().clone()
        };
        find_and_activate(&items, &mut current_index, target_index, window, cx)
    }

    /// Open a path in the appropriate editor using the plugin system.
    ///
    /// This is the ONLY method needed for file opening - the plugin system handles everything.
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        tracing::debug!("Opening path: {:?}", path);

        // Update plugin manager with current project root
        if let Some(pm_lock) = plugin_manager::global() {
            let mut pm = pm_lock.write();
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
