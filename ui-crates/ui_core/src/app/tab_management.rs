//! Tab management and file opening logic

use std::{path::PathBuf, sync::Arc};
use gpui::{AppContext, Context, Window};
use ui_file_manager::{FileSelected, FileType};
use ui_script_editor::ScriptEditorPanel;
use ui_level_editor::LevelEditorPanel;
use ui_daw_editor::DawEditorPanel;

use super::{PulsarApp, event_handlers};

impl PulsarApp {
    /// Open a path in the appropriate editor
    pub fn open_path(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Try plugin system first for all file types
        match self.state.plugin_manager.create_editor_for_file(&path, window, cx) {
            Ok((panel_ptr, _editor_instance)) => {
                tracing::info!("Plugin system opened file: {:?}", path);

                // SAFETY: panel_ptr is valid and owned by the plugin
                // Clone the panel entity using the trait method
                let panel = unsafe {
                    let panel_ref: &dyn ui::dock::PanelView = &*panel_ptr;
                    panel_ref.clone_panel()
                };

                self.state.center_tabs.update(cx, |tabs, cx| {
                    tabs.add_panel(panel, window, cx);
                });
                return;
            }
            Err(e) => {
                tracing::debug!("Plugin system couldn't open {:?}: {} - trying legacy handlers", path, e);
            }
        }

        // Legacy fallback for file types not yet migrated to plugins
        if let Some(extension) = path.extension() {
            match extension.to_str() {
                Some("pdaw") => {
                    self.open_daw_tab(path, window, cx);
                }
                Some("db") | Some("sqlite") | Some("sqlite3") => {
                    self.open_database_tab(path, window, cx);
                }
                Some("rs") | Some("js") | Some("ts") | Some("py") | Some("lua") => {
                    self.open_script_tab(path, window, cx);
                }
                _ => {}
            }
        }
    }

    /// Public handler for file selected events from external windows
    pub fn handle_file_selected_from_external_window(
        &mut self,
        event: &FileSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::debug!(
            "File selected from external window - path: {:?}, type: {:?}",
            event.path, event.file_type
        );

        match event.file_type {
            FileType::Class => {
                tracing::error!("Blueprint editor tried to load from external window, but it's now plugin-only!");
            }
            FileType::Script | FileType::Config | FileType::Document => {
                tracing::debug!("Opening script tab from external window");
                self.open_script_tab(event.path.clone(), window, cx);
            }
            FileType::DawProject => {
                tracing::debug!("Opening DAW tab from external window: {:?}", event.path);
                self.open_daw_tab(event.path.clone(), window, cx);
            }
            FileType::Database => {
                tracing::debug!("Opening database tab from external window: {:?}", event.path);
                self.open_database_tab(event.path.clone(), window, cx);
            }
            FileType::StructType => {
                tracing::debug!("Opening struct editor from external window: {:?}", event.path);
                self.open_struct_tab(event.path.clone(), window, cx);
            }
            FileType::EnumType => {
                tracing::debug!("Opening enum editor from external window: {:?}", event.path);
                self.open_enum_tab(event.path.clone(), window, cx);
            }
            FileType::TraitType => {
                tracing::debug!("Opening trait editor from external window: {:?}", event.path);
                self.open_trait_tab(event.path.clone(), window, cx);
            }
            FileType::AliasType => {
                tracing::debug!("Opening alias editor from external window: {:?}", event.path);
                self.open_alias_tab(event.path.clone(), window, cx);
            }
            _ => {
                tracing::debug!("Unknown file type from external window, ignoring");
            }
        }
    }

    /// Open or focus the script editor tab
    pub(super) fn open_script_tab(&mut self, file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Check if script editor already exists
        if let Some(script_editor) = &self.state.script_editor {
            script_editor.update(cx, |editor, cx| {
                editor.open_file(file_path, window, cx);
            });
            return;
        }

        // Create new script editor tab
        let script_editor = cx.new(|cx| ScriptEditorPanel::new(window, cx));

        // Wire up rust-analyzer to the script editor
        let analyzer = self.state.rust_analyzer.clone();
        script_editor.update(cx, |editor, cx| {
            editor.set_rust_analyzer(analyzer, cx);
        });

        // Load project in file explorer if we have a project path
        if let Some(ref project_path) = self.state.project_path {
            script_editor.update(cx, |editor, cx| {
                editor.set_project_path(project_path.clone(), window, cx);
            });
        }

        cx.subscribe_in(&script_editor, window, event_handlers::on_text_editor_event).detach();

        // Open the specific file
        script_editor.update(cx, |editor, cx| {
            editor.open_file(file_path, window, cx);
        });

        // Add the tab to the tab panel
        self.state.center_tabs.update(cx, |tabs, cx| {
            tabs.add_panel(Arc::new(script_editor.clone()), window, cx);
        });

        self.state.script_editor = Some(script_editor);
    }

    /// Open a DAW editor tab for the given project path
    pub(super) fn open_daw_tab(&mut self, project_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        tracing::error!("DEBUG: open_daw_tab called with path: {:?}", project_path);

        if !self.state.daw_editors.is_empty() {
            tracing::error!("DEBUG: DAW editor already exists, focusing first one");
            return;
        }

        tracing::error!("DEBUG: Creating new DAW editor panel");
        self.state.next_tab_id += 1;

        let daw_editor = cx.new(|cx| DawEditorPanel::new_with_project(project_path.clone(), window, cx));

        tracing::error!("DEBUG: Adding DAW editor to tab panel");
        self.state.center_tabs.update(cx, |tabs, cx| {
            tabs.add_panel(Arc::new(daw_editor.clone()), window, cx);
        });

        tracing::error!("DEBUG: Storing DAW editor reference");
        self.state.daw_editors.push(daw_editor);

        tracing::error!("DEBUG: DAW tab opened successfully");
    }

    pub(super) fn open_level_editor_tab(&mut self, _scene_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        tracing::error!("DEBUG: Opening level editor");

        let level_editor = cx.new(|cx| LevelEditorPanel::new(window, cx));

        self.state.center_tabs.update(cx, |tabs, cx| {
            tabs.add_panel(Arc::new(level_editor.clone()), window, cx);
        });
    }

    /// Open a database editor tab for the given database path
    pub(super) fn open_database_tab(&mut self, db_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        tracing::error!("DEBUG: open_database_tab called with path: {:?}", db_path);

        // Check if a database editor for this path is already open
        let already_open = self
            .state.database_editors
            .iter()
            .enumerate()
            .find_map(|(ix, editor)| {
                editor
                    .read(cx)
                    .database_path
                    .as_ref()
                    .map(|p| p == &db_path)
                    .unwrap_or(false)
                    .then_some(ix)
            });

        if let Some(ix) = already_open {
            tracing::error!("DEBUG: Database editor already exists, focusing it");
            if let Some(editor_entity) = self.state.database_editors.get(ix) {
                let target_id = editor_entity.entity_id();
                self.state.center_tabs.update(cx, |tabs, cx| {
                    if let Some(tab_ix) = tabs.index_of_panel_by_entity_id(target_id) {
                        tabs.set_active_tab(tab_ix, window, cx);
                    }
                });
            }
            return;
        }

        tracing::error!("DEBUG: Creating new database editor");
        self.state.next_tab_id += 1;

        let database_editor = cx.new(|cx| {
            ui_editor_table::DataTableEditor::open_database(db_path.clone(), window, cx)
                .unwrap_or_else(|e| {
                    tracing::error!("Failed to open database: {}", e);
                    ui_editor_table::DataTableEditor::new(window, cx)
                })
        });

        tracing::error!("DEBUG: Adding database editor to tab panel");
        self.state.center_tabs.update(cx, |tabs, cx| {
            tabs.add_panel(Arc::new(database_editor.clone()), window, cx);
        });

        tracing::error!("DEBUG: Storing database editor reference");
        self.state.database_editors.push(database_editor);

        tracing::error!("DEBUG: Database tab opened successfully");
    }

    /// Open a struct editor tab for the given struct file
    pub(super) fn open_struct_tab(&mut self, file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        tracing::error!("DEBUG: open_struct_tab called with path: {:?}", file_path);

        let actual_file_path = if file_path.is_dir() {
            file_path.join("struct.json")
        } else {
            file_path.clone()
        };

        let already_open = self
            .state.struct_editors
            .iter()
            .enumerate()
            .find_map(|(ix, editor)| {
                editor
                    .read(cx)
                    .file_path()
                    .map(|p| p == actual_file_path)
                    .unwrap_or(false)
                    .then_some(ix)
            });

        if let Some(ix) = already_open {
            tracing::error!("DEBUG: Struct editor already exists, focusing it");
            if let Some(editor_entity) = self.state.struct_editors.get(ix) {
                let target_id = editor_entity.entity_id();
                self.state.center_tabs.update(cx, |tabs, cx| {
                    if let Some(tab_ix) = tabs.index_of_panel_by_entity_id(target_id) {
                        tabs.set_active_tab(tab_ix, window, cx);
                    }
                });
            }
            return;
        }

        tracing::error!("DEBUG: Creating new struct editor");
        self.state.next_tab_id += 1;

        let struct_editor = cx.new(|cx| {
            ui_struct_editor::StructEditor::new_with_file(actual_file_path.clone(), window, cx)
        });

        tracing::error!("DEBUG: Adding struct editor to tab panel");
        self.state.center_tabs.update(cx, |tabs, cx| {
            tabs.add_panel(Arc::new(struct_editor.clone()), window, cx);
        });

        tracing::error!("DEBUG: Storing struct editor reference");
        self.state.struct_editors.push(struct_editor);

        tracing::error!("DEBUG: Struct tab opened successfully");
    }

    /// Open an enum editor tab for the given enum file
    pub(super) fn open_enum_tab(&mut self, file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        tracing::error!("DEBUG: open_enum_tab called with path: {:?}", file_path);

        let actual_file_path = if file_path.is_dir() {
            file_path.join("enum.json")
        } else {
            file_path.clone()
        };

        let already_open = self
            .state.enum_editors
            .iter()
            .enumerate()
            .find_map(|(ix, editor)| {
                editor
                    .read(cx)
                    .file_path()
                    .map(|p| p == actual_file_path)
                    .unwrap_or(false)
                    .then_some(ix)
            });

        if let Some(ix) = already_open {
            tracing::error!("DEBUG: Enum editor already exists, focusing it");
            if let Some(editor_entity) = self.state.enum_editors.get(ix) {
                let target_id = editor_entity.entity_id();
                self.state.center_tabs.update(cx, |tabs, cx| {
                    if let Some(tab_ix) = tabs.index_of_panel_by_entity_id(target_id) {
                        tabs.set_active_tab(tab_ix, window, cx);
                    }
                });
            }
            return;
        }

        tracing::error!("DEBUG: Creating new enum editor");
        self.state.next_tab_id += 1;

        let enum_editor = cx.new(|cx| {
            ui_enum_editor::EnumEditor::new_with_file(actual_file_path.clone(), window, cx)
        });

        tracing::error!("DEBUG: Adding enum editor to tab panel");
        self.state.center_tabs.update(cx, |tabs, cx| {
            tabs.add_panel(Arc::new(enum_editor.clone()), window, cx);
        });

        tracing::error!("DEBUG: Storing enum editor reference");
        self.state.enum_editors.push(enum_editor);

        tracing::error!("DEBUG: Enum tab opened successfully");
    }

    /// Open a trait editor tab for the given trait file
    pub(super) fn open_trait_tab(&mut self, file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        tracing::error!("DEBUG: open_trait_tab called with path: {:?}", file_path);

        let actual_file_path = if file_path.is_dir() {
            file_path.join("trait.json")
        } else {
            file_path.clone()
        };

        let already_open = self
            .state.trait_editors
            .iter()
            .enumerate()
            .find_map(|(ix, editor)| {
                editor
                    .read(cx)
                    .file_path()
                    .map(|p| p == actual_file_path)
                    .unwrap_or(false)
                    .then_some(ix)
            });

        if let Some(ix) = already_open {
            tracing::error!("DEBUG: Trait editor already exists, focusing it");
            if let Some(editor_entity) = self.state.trait_editors.get(ix) {
                let target_id = editor_entity.entity_id();
                self.state.center_tabs.update(cx, |tabs, cx| {
                    if let Some(tab_ix) = tabs.index_of_panel_by_entity_id(target_id) {
                        tabs.set_active_tab(tab_ix, window, cx);
                    }
                });
            }
            return;
        }

        tracing::error!("DEBUG: Creating new trait editor");
        self.state.next_tab_id += 1;

        let trait_editor = cx.new(|cx| {
            ui_trait_editor::TraitEditor::new_with_file(actual_file_path.clone(), window, cx)
        });

        tracing::error!("DEBUG: Adding trait editor to tab panel");
        self.state.center_tabs.update(cx, |tabs, cx| {
            tabs.add_panel(Arc::new(trait_editor.clone()), window, cx);
        });

        tracing::error!("DEBUG: Storing trait editor reference");
        self.state.trait_editors.push(trait_editor);

        tracing::error!("DEBUG: Trait tab opened successfully");
    }

    /// Open an alias editor tab for the given alias file
    pub(super) fn open_alias_tab(&mut self, file_path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        tracing::error!("DEBUG: open_alias_tab called with path: {:?}", file_path);

        let actual_file_path = if file_path.is_dir() {
            file_path.join("alias.json")
        } else {
            file_path.clone()
        };

        let already_open = self
            .state.alias_editors
            .iter()
            .enumerate()
            .find_map(|(ix, editor)| {
                editor
                    .read(cx)
                    .file_path()
                    .map(|p| p == actual_file_path)
                    .unwrap_or(false)
                    .then_some(ix)
            });

        if let Some(ix) = already_open {
            tracing::error!("DEBUG: Alias editor already exists, focusing it");
            if let Some(editor_entity) = self.state.alias_editors.get(ix) {
                let target_id = editor_entity.entity_id();
                self.state.center_tabs.update(cx, |tabs, cx| {
                    if let Some(tab_ix) = tabs.index_of_panel_by_entity_id(target_id) {
                        tabs.set_active_tab(tab_ix, window, cx);
                    }
                });
            }
            return;
        }

        tracing::error!("DEBUG: Creating new alias editor");
        self.state.next_tab_id += 1;

        let alias_editor = cx.new(|cx| {
            ui_alias_editor::AliasEditor::new_with_file(actual_file_path.clone(), window, cx)
        });

        cx.subscribe_in(&alias_editor, window, event_handlers::on_show_type_picker_request).detach();

        tracing::error!("DEBUG: Adding alias editor to tab panel");
        self.state.center_tabs.update(cx, |tabs, cx| {
            tabs.add_panel(Arc::new(alias_editor.clone()), window, cx);
        });

        tracing::error!("DEBUG: Storing alias editor reference");
        self.state.alias_editors.push(alias_editor);

        tracing::error!("DEBUG: Alias tab opened successfully");
    }
}
