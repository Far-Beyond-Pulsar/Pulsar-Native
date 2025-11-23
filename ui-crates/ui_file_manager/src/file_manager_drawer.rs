use gpui::*;
use gpui::prelude::FluentBuilder;
use ui::{
    button::{Button, ButtonVariants as _},
    context_menu::ContextMenuExt,
    h_flex,
    input::{InputState, TextInput},
    resizable::{h_resizable, resizable_panel, v_resizable, ResizableState},
    v_flex, ActiveTheme as _, Icon, IconName, StyledExt,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::path::{Path, PathBuf};

use gpui::Axis;

// Actions for file management
#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct NewClass {
    pub folder_path: String,
}

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct NewFolder {
    pub folder_path: String,
}

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct NewFile {
    pub folder_path: String,
}

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct DeleteItem {
    pub item_path: String,
}

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager)]
pub struct RenameItem {
    pub item_path: String,
}

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct CommitRename;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct CancelRename;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct RefreshFileManager;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct CollapseAllFolders;

#[derive(Action, Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema)]
#[action(namespace = file_manager, no_json)]
pub struct ToggleHiddenFiles;

#[derive(Clone, Debug, PartialEq)]
pub enum FileType {
    Folder,
    Class, // A folder containing graph_save.json
    Script,
    DawProject, // .pdaw files
    Database, // .db, .sqlite, .sqlite3 files
    Config, // .toml files
    // Type System Files
    StructType,  // .struct.json files
    EnumType,    // .enum.json files
    TraitType,   // .trait.json files
    AliasType,   // .alias.json files
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ViewMode {
    Grid,
    List,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SortBy {
    Name,
    DateModified,
    Size,
    Type,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SortOrder {
    Ascending,
    Descending,
}

#[derive(Clone, Debug)]
pub struct FileItem {
    pub path: PathBuf,
    pub name: String,
    pub file_type: FileType,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}

impl FileItem {
    pub fn is_class_folder(path: &Path) -> bool {
        path.is_dir() && path.join("graph_save.json").exists()
    }

    pub fn is_struct_folder(path: &Path) -> bool {
        path.is_dir() && path.join("struct.json").exists()
    }

    pub fn is_enum_folder(path: &Path) -> bool {
        path.is_dir() && path.join("enum.json").exists()
    }

    pub fn is_trait_folder(path: &Path) -> bool {
        path.is_dir() && path.join("trait.json").exists()
    }

    pub fn is_alias_folder(path: &Path) -> bool {
        path.is_dir() && path.join("alias.json").exists()
    }

    pub fn from_path(path: &Path) -> Option<Self> {
        let name = path.file_name()?.to_str()?.to_string();

        let file_type = if path.is_dir() {
            if Self::is_class_folder(path) {
                FileType::Class
            } else if Self::is_struct_folder(path) {
                FileType::StructType
            } else if Self::is_enum_folder(path) {
                FileType::EnumType
            } else if Self::is_trait_folder(path) {
                FileType::TraitType
            } else if Self::is_alias_folder(path) {
                FileType::AliasType
            } else {
                FileType::Folder
            }
        } else {
            // Check for type system files first (they have .json extension but specific naming)
            let file_name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if file_name.ends_with(".struct.json") {
                FileType::StructType
            } else if file_name.ends_with(".enum.json") {
                FileType::EnumType
            } else if file_name.ends_with(".trait.json") {
                FileType::TraitType
            } else if file_name.ends_with(".alias.json") {
                FileType::AliasType
            } else {
                match path.extension().and_then(|s| s.to_str()) {
                    Some("rs") => FileType::Script,
                    Some("pdaw") => FileType::DawProject,
                    Some("db") | Some("sqlite") | Some("sqlite3") => FileType::Database,
                    Some("toml") => FileType::Config,
                    _ => FileType::Other,
                }
            }
        };

        // Get file metadata for size and modified date
        let metadata = std::fs::metadata(path).ok();
        let size = metadata.as_ref().map(|m| m.len()).unwrap_or(0);
        let modified = metadata.and_then(|m| m.modified().ok());

        Some(FileItem {
            path: path.to_path_buf(),
            name,
            file_type,
            size,
            modified,
        })
    }
}

#[derive(Clone, Debug)]
pub struct FolderNode {
    pub path: PathBuf,
    pub name: String,
    pub children: Vec<FolderNode>,
    pub expanded: bool,
    pub is_class: bool,
}

impl FolderNode {
    pub fn from_path(path: &Path) -> Option<Self> {
        if !path.is_dir() {
            return None;
        }

        let name = path.file_name()?.to_str()?.to_string();
        let is_class = FileItem::is_class_folder(path);

        // Class folders should NOT appear in the tree at all
        if is_class {
            return None;
        }

        // Only recurse into non-class folders
        let children = std::fs::read_dir(path)
            .ok()?
            .filter_map(|entry| {
                let entry = entry.ok()?;
                let entry_path = entry.path();

                // Skip hidden files and non-directories
                if !entry_path.is_dir() {
                    return None;
                }
                if entry_path.file_name()?.to_str()?.starts_with('.') {
                    return None;
                }

                FolderNode::from_path(&entry_path)
            })
            .collect();

        Some(FolderNode {
            path: path.to_path_buf(),
            name,
            children,
            expanded: false,
            is_class,
        })
    }

    fn toggle_expanded(&mut self, target_path: &Path) -> bool {
        if self.path == target_path {
            self.expanded = !self.expanded;
            return true;
        }

        for child in &mut self.children {
            if child.toggle_expanded(target_path) {
                return true;
            }
        }

        false
    }
}

pub struct FileManagerDrawer {
    project_path: Option<PathBuf>,
    folder_tree: Option<FolderNode>,
    selected_folder: Option<PathBuf>,
    resizable_state: Entity<ResizableState>,
    height_resizable_state: Entity<ResizableState>,
    renaming_item: Option<PathBuf>,
    rename_input_state: Entity<InputState>,
    is_in_window: bool, // Track if this drawer is rendered in a separate window
    view_mode: ViewMode,
    sort_by: SortBy,
    sort_order: SortOrder,
    search_query: String,
    folder_search_state: Entity<InputState>,
    file_filter_query: String,
    file_filter_state: Entity<InputState>,
    show_hidden_files: bool,
    selected_file_types: Vec<FileType>, // Filter by file type
}

impl FileManagerDrawer {
    pub fn new(project_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_context(project_path, window, cx, false)
    }

    pub fn new_in_window(project_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new_with_context(project_path, window, cx, true)
    }

    fn new_with_context(
        project_path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
        is_in_window: bool,
    ) -> Self {
        let resizable_state = ResizableState::new(cx);
        let height_resizable_state = ResizableState::new(cx);
        let rename_input_state = cx.new(|cx| InputState::new(window, cx));
        let folder_search_state = cx.new(|cx| InputState::new(window, cx));
        let file_filter_state = cx.new(|cx| InputState::new(window, cx));

        // Subscribe to input events to handle Enter key for committing rename
        cx.subscribe(
            &rename_input_state,
            |drawer, _input, event: &ui::input::InputEvent, cx| match event {
                ui::input::InputEvent::PressEnter { .. } => {
                    if drawer.renaming_item.is_some() {
                        cx.dispatch_action(&CommitRename);
                    }
                }
                _ => {}
            },
        )
        .detach();

        // Subscribe to folder search input for live filtering
        cx.subscribe(
            &folder_search_state,
            |drawer, _input, event: &ui::input::InputEvent, cx| match event {
                ui::input::InputEvent::Change { .. } => {
                    drawer.search_query = drawer.folder_search_state.read(cx).text().to_string();
                    cx.notify();
                }
                _ => {}
            },
        )
        .detach();

        // Subscribe to file filter input for live filtering
        cx.subscribe(
            &file_filter_state,
            |drawer, _input, event: &ui::input::InputEvent, cx| match event {
                ui::input::InputEvent::Change { .. } => {
                    drawer.file_filter_query = drawer.file_filter_state.read(cx).text().to_string();
                    cx.notify();
                }
                _ => {}
            },
        )
        .detach();

        Self {
            folder_tree: project_path.as_ref().and_then(|p| FolderNode::from_path(p)),
            project_path: project_path.clone(),
            selected_folder: project_path,
            resizable_state,
            height_resizable_state,
            renaming_item: None,
            rename_input_state,
            is_in_window,
            view_mode: ViewMode::Grid,
            sort_by: SortBy::Name,
            sort_order: SortOrder::Ascending,
            search_query: String::new(),
            folder_search_state,
            file_filter_query: String::new(),
            file_filter_state,
            show_hidden_files: false,
            selected_file_types: Vec::new(),
        }
    }

    pub fn set_project_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.project_path = Some(path.clone());
        self.folder_tree = FolderNode::from_path(&path);
        self.selected_folder = Some(path);
        self.resizable_state = ResizableState::new(cx);
        self.height_resizable_state = ResizableState::new(cx);
        cx.notify();
    }

    fn toggle_folder(&mut self, path: &Path, cx: &mut Context<Self>) {
        if let Some(tree) = &mut self.folder_tree {
            tree.toggle_expanded(path);
            cx.notify();
        }
    }

    fn select_folder(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.selected_folder = Some(path);
        cx.notify();
    }

    fn get_folder_contents(&self) -> Vec<FileItem> {
        let folder = match &self.selected_folder {
            Some(f) => f,
            None => return Vec::new(),
        };

        let mut items: Vec<FileItem> = std::fs::read_dir(folder)
            .ok()
            .map(|entries| {
                entries
                    .filter_map(|entry| {
                        let entry = entry.ok()?;
                        let path = entry.path();

                        // Skip hidden files if toggle is off
                        if !self.show_hidden_files {
                            if path.file_name()?.to_str()?.starts_with('.') {
                                return None;
                            }
                        }

                        FileItem::from_path(&path)
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Apply file filter query
        if !self.file_filter_query.is_empty() {
            let query_lower = self.file_filter_query.to_lowercase();
            items.retain(|item| {
                item.name.to_lowercase().contains(&query_lower)
            });
        }

        // Apply file type filter
        if !self.selected_file_types.is_empty() {
            items.retain(|item| {
                self.selected_file_types.contains(&item.file_type)
            });
        }

        // Sort items
        items.sort_by(|a, b| {
            let comparison = match self.sort_by {
                SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortBy::DateModified => {
                    match (a.modified, b.modified) {
                        (Some(a_time), Some(b_time)) => a_time.cmp(&b_time),
                        (Some(_), None) => std::cmp::Ordering::Less,
                        (None, Some(_)) => std::cmp::Ordering::Greater,
                        (None, None) => std::cmp::Ordering::Equal,
                    }
                }
                SortBy::Size => a.size.cmp(&b.size),
                SortBy::Type => {
                    // Sort by file type, then by name
                    let type_ord = format!("{:?}", a.file_type).cmp(&format!("{:?}", b.file_type));
                    if type_ord == std::cmp::Ordering::Equal {
                        a.name.to_lowercase().cmp(&b.name.to_lowercase())
                    } else {
                        type_ord
                    }
                }
            };

            match self.sort_order {
                SortOrder::Ascending => comparison,
                SortOrder::Descending => comparison.reverse(),
            }
        });

        items
    }

    /// Handle clicking on an item in the file manager drawer
    fn handle_item_click(&mut self, item: &FileItem, cx: &mut Context<Self>) {
        eprintln!("DEBUG: handle_item_click called for: {:?}, type: {:?}", item.path, item.file_type);
        
        match &item.file_type {
            FileType::Class | FileType::Script | FileType::DawProject | FileType::Database
            | FileType::StructType | FileType::EnumType | FileType::TraitType | FileType::AliasType => {
                eprintln!("DEBUG: Emitting FileSelected event");
                // Emit event to open this class in BP editor, script in script editor, DAW project, database, or type editor
                cx.emit(FileSelected {
                    path: item.path.clone(),
                    file_type: item.file_type.clone(),
                });
            }
            FileType::Folder => {
                eprintln!("DEBUG: Selecting folder");
                // Select this folder to show its contents
                self.select_folder(item.path.clone(), cx);
            }
            _ => {
                eprintln!("DEBUG: Unknown file type, ignoring");
            }
        }
    }

    fn on_new_folder(&mut self, action: &NewFolder, window: &mut Window, cx: &mut Context<Self>) {
        let base_path = std::path::Path::new(&action.folder_path);
        let new_folder_name = "NewFolder";
        let new_path = base_path.join(new_folder_name);

        if let Err(e) = std::fs::create_dir_all(&new_path) {
            eprintln!("Failed to create folder: {}", e);
            return;
        }

        // Refresh the folder tree
        if let Some(project_path) = &self.project_path {
            self.folder_tree = FolderNode::from_path(project_path);
        }

        // Start rename mode for the new folder
        self.start_rename(new_path, window, cx);
    }

    fn on_new_class(&mut self, action: &NewClass, window: &mut Window, cx: &mut Context<Self>) {
        let base_path = std::path::Path::new(&action.folder_path);
        let new_class_name = "NewClass";
        let new_path = base_path.join(new_class_name);

        // Create class folder and events subfolder
        if let Err(e) = std::fs::create_dir_all(&new_path.join("events")) {
            eprintln!("Failed to create class folder: {}", e);
            return;
        }

        // Create empty graph_save.json with header
        let now = chrono::Local::now();
        let version = ui::ENGINE_VERSION;
        let header = format!(
            "// Auto Generated by the Pulsar Blueprint Editor\n\
             // DO NOT EDIT MANUALLY - YOUR CHANGES WILL BE OVERWRITTEN\n\
             // Generated on {} - Engine version {}\n\
             //\n\
             // This file contains the visual blueprint graph for this class.\n\
             // EDITING THE JSON STRUCTURE COULD BREAK THE EDITOR - BE CAREFUL\n\n",
            now.format("%Y-%m-%d %H:%M:%S"),
            version
        );

        let empty_graph = ui::graph::GraphDescription {
            nodes: std::collections::HashMap::new(),
            connections: Vec::new(),
            metadata: ui::graph::GraphMetadata {
                name: new_class_name.to_string(),
                description: String::from("New class created from file manager"),
                version: ui::ENGINE_VERSION.to_string(),
                created_at: now.to_rfc3339(),
                modified_at: now.to_rfc3339(),
            },
            comments: Vec::new(),
        };

        let json = serde_json::to_string_pretty(&empty_graph).unwrap_or_default();
        let content = format!("{}{}", header, json);

        if let Err(e) = std::fs::write(new_path.join("graph_save.json"), content) {
            eprintln!("Failed to create graph_save.json: {}", e);
            return;
        }

        // Refresh the folder tree
        if let Some(project_path) = &self.project_path {
            self.folder_tree = FolderNode::from_path(project_path);
        }

        // Start rename mode for the new class
        self.start_rename(new_path, window, cx);
    }

    fn on_new_file(&mut self, action: &NewFile, window: &mut Window, cx: &mut Context<Self>) {
        let base_path = std::path::Path::new(&action.folder_path);
        let new_file_name = "new_file.rs";
        let new_path = base_path.join(new_file_name);

        // Create empty Rust file with basic header
        let content = "// New Rust file\n\npub fn main() {\n    // Your code here\n}\n";

        if let Err(e) = std::fs::write(&new_path, content) {
            eprintln!("Failed to create file: {}", e);
            return;
        }

        // Start rename mode for the new file
        self.start_rename(new_path, window, cx);
    }

    fn on_delete_item(
        &mut self,
        action: &DeleteItem,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let item_path = std::path::Path::new(&action.item_path);

        // TODO: Add confirmation dialog
        if item_path.is_dir() {
            if let Err(e) = std::fs::remove_dir_all(item_path) {
                eprintln!("Failed to delete folder: {}", e);
                return;
            }
        } else {
            if let Err(e) = std::fs::remove_file(item_path) {
                eprintln!("Failed to delete file: {}", e);
                return;
            }
        }

        // Refresh the folder tree
        if let Some(project_path) = &self.project_path {
            self.folder_tree = FolderNode::from_path(project_path);
        }
        cx.notify();
    }

    fn on_rename_item(&mut self, action: &RenameItem, window: &mut Window, cx: &mut Context<Self>) {
        let item_path = std::path::Path::new(&action.item_path);
        if let Some(file_name) = item_path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
        {
            self.renaming_item = Some(item_path.to_path_buf());
            self.rename_input_state.update(cx, |state, cx| {
                state.set_value(&file_name, window, cx);
            });
            cx.notify();
        }
    }

    fn start_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(file_name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
        {
            self.renaming_item = Some(path);
            self.rename_input_state.update(cx, |state, cx| {
                state.set_value(&file_name, window, cx);
            });
            cx.notify();
        }
    }

    fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(old_path) = &self.renaming_item {
            // Get text as string
            let new_name = self
                .rename_input_state
                .read(cx)
                .text()
                .to_string()
                .trim()
                .to_string();
            if new_name.is_empty() {
                self.cancel_rename(cx);
                return;
            }

            if let Some(parent) = old_path.parent() {
                let new_path = parent.join(new_name);

                if old_path != &new_path {
                    if let Err(e) = std::fs::rename(old_path, &new_path) {
                        eprintln!("Failed to rename: {}", e);
                    } else {
                        // Update selected folder if we renamed it
                        if self.selected_folder.as_ref() == Some(old_path) {
                            self.selected_folder = Some(new_path);
                        }

                        // Refresh the folder tree
                        if let Some(project_path) = &self.project_path {
                            self.folder_tree = FolderNode::from_path(project_path);
                        }
                    }
                }
            }

            self.cancel_rename(cx);
        }
    }

    fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        self.renaming_item = None;
        cx.notify();
    }

    fn on_commit_rename(
        &mut self,
        _action: &CommitRename,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.commit_rename(window, cx);
    }

    fn on_cancel_rename(
        &mut self,
        _action: &CancelRename,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel_rename(cx);
    }

    fn on_refresh(&mut self, _action: &RefreshFileManager, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(project_path) = &self.project_path {
            self.folder_tree = FolderNode::from_path(project_path);
        }
        cx.notify();
    }

    fn on_collapse_all(&mut self, _action: &CollapseAllFolders, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(tree) = &mut self.folder_tree {
            Self::collapse_all_recursive(tree);
        }
        cx.notify();
    }

    fn collapse_all_recursive(node: &mut FolderNode) {
        node.expanded = false;
        for child in &mut node.children {
            Self::collapse_all_recursive(child);
        }
    }

    fn on_toggle_hidden(&mut self, _action: &ToggleHiddenFiles, _window: &mut Window, cx: &mut Context<Self>) {
        self.show_hidden_files = !self.show_hidden_files;
        cx.notify();
    }

    fn toggle_view_mode(&mut self, mode: ViewMode, cx: &mut Context<Self>) {
        self.view_mode = mode;
        cx.notify();
    }

    fn set_sort_by(&mut self, sort_by: SortBy, cx: &mut Context<Self>) {
        if self.sort_by == sort_by {
            // Toggle sort order if same field
            self.sort_order = match self.sort_order {
                SortOrder::Ascending => SortOrder::Descending,
                SortOrder::Descending => SortOrder::Ascending,
            };
        } else {
            self.sort_by = sort_by;
            self.sort_order = SortOrder::Ascending;
        }
        cx.notify();
    }

    fn toggle_file_type_filter(&mut self, file_type: FileType, cx: &mut Context<Self>) {
        if let Some(pos) = self.selected_file_types.iter().position(|t| t == &file_type) {
            self.selected_file_types.remove(pos);
        } else {
            self.selected_file_types.push(file_type);
        }
        cx.notify();
    }

    fn format_file_size(size: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if size >= GB {
            format!("{:.2} GB", size as f64 / GB as f64)
        } else if size >= MB {
            format!("{:.2} MB", size as f64 / MB as f64)
        } else if size >= KB {
            format!("{:.2} KB", size as f64 / KB as f64)
        } else {
            format!("{} B", size)
        }
    }

    fn format_modified_time(time: Option<std::time::SystemTime>) -> String {
        time.and_then(|t| {
            t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| {
                let datetime = chrono::DateTime::<chrono::Local>::from(std::time::UNIX_EPOCH + d);
                datetime.format("%b %d, %Y %H:%M").to_string()
            })
        })
        .unwrap_or_else(|| "Unknown".to_string())
    }

    fn get_breadcrumbs(&self) -> Vec<(String, PathBuf)> {
        let mut breadcrumbs = Vec::new();
        if let Some(selected) = &self.selected_folder {
            if let Some(project) = &self.project_path {
                let relative = selected.strip_prefix(project).unwrap_or(selected);
                let mut current = project.clone();
                
                breadcrumbs.push(("Project".to_string(), current.clone()));
                
                for component in relative.components() {
                    if let std::path::Component::Normal(name) = component {
                        current = current.join(name);
                        breadcrumbs.push((name.to_string_lossy().to_string(), current.clone()));
                    }
                }
            }
        }
        breadcrumbs
    }

    fn matches_folder_search(&self, node: &FolderNode) -> bool {
        if self.search_query.is_empty() {
            return true;
        }
        let query_lower = self.search_query.to_lowercase();
        node.name.to_lowercase().contains(&query_lower) ||
            node.children.iter().any(|child| self.matches_folder_search(child))
    }

    fn render_folder_tree_node(
        &self,
        node: &FolderNode,
        depth: usize,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // Skip nodes that don't match search
        if !self.matches_folder_search(node) {
            return div().into_any_element();
        }

        let indent = (depth + 1) * 12;
        let icon = if node.is_class {
            IconName::Component
        } else if node.expanded {
            IconName::ChevronDown
        } else {
            IconName::ChevronRight
        };

        let path_clone = node.path.clone();
        let path_clone2 = node.path.clone();
        let path_for_menu = node.path.clone();
        let is_selected = self.selected_folder.as_ref() == Some(&node.path);
        let is_class = node.is_class;

        // COMPACT TREE NODE
        let node_button = div()
            .w_full()
            .h(px(26.))
            .rounded(px(4.))
            .pl(px(indent as f32 + 6.))
            .pr_1p5()
            .flex()
            .items_center()
            .gap_1p5()
            .cursor_pointer()
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |drawer, _, _, cx| {
                    if !is_class {
                        drawer.toggle_folder(&path_clone, cx);
                    }
                    drawer.select_folder(path_clone2.clone(), cx);
                }),
            )
            .when(is_selected, |this| {
                this.bg(cx.theme().primary.opacity(0.12))
                    .border_l_2()
                    .border_color(cx.theme().primary)
            })
            .when(!is_selected, |this| {
                this.hover(|this| {
                    this.bg(cx.theme().muted.opacity(0.4))
                })
            })
            .child(
                Icon::new(icon)
                    .size(px(14.))
                    .text_color(
                        if is_selected {
                            cx.theme().primary
                        } else if node.is_class {
                            cx.theme().accent
                        } else {
                            cx.theme().muted_foreground.opacity(0.7)
                        }
                    )
            )
            .child(
                if self.renaming_item.as_ref() == Some(&node.path) {
                    div()
                        .flex_1()
                        .child(
                            TextInput::new(&self.rename_input_state)
                                .appearance(false)
                                .w_full()
                        )
                        .into_any_element()
                } else {
                    div()
                        .flex_1()
                        .text_xs()
                        .when(is_selected, |this| {
                            this.font_semibold()
                                .text_color(cx.theme().foreground)
                        })
                        .when(!is_selected, |this| {
                            this.text_color(cx.theme().foreground.opacity(0.8))
                        })
                        .child(node.name.clone())
                        .into_any_element()
                }
            )
            .when(is_class, |this| {
                this.child(
                    // Compact class badge
                    div()
                        .px_1()
                        .py_0p5()
                        .rounded(px(3.))
                        .bg(cx.theme().accent.opacity(0.15))
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().accent)
                                .child("BP")
                        )
                )
            });

        v_flex()
            .w_full()
            .gap_0p5()
            .child(
                div()
                    .id(SharedString::from(format!(
                        "tree-item-{}",
                        node.path.display()
                    )))
                    .w_full()
                    .context_menu(move |menu, _window, _cx| {
                        menu.menu(
                            "New Folder",
                            Box::new(NewFolder {
                                folder_path: path_for_menu.to_string_lossy().to_string(),
                            }),
                        )
                        .menu(
                            "New Class",
                            Box::new(NewClass {
                                folder_path: path_for_menu.to_string_lossy().to_string(),
                            }),
                        )
                        .separator()
                        .menu(
                            "Rename",
                            Box::new(RenameItem {
                                item_path: path_for_menu.to_string_lossy().to_string(),
                            }),
                        )
                        .menu(
                            "Delete",
                            Box::new(DeleteItem {
                                item_path: path_for_menu.to_string_lossy().to_string(),
                            }),
                        )
                    })
                    .child(node_button)
            )
            .children(if node.expanded && !node.is_class {
                Some(
                    v_flex()
                        .w_full()
                        .gap_0p5()
                        .children(
                            node.children
                                .iter()
                                .map(|child| self.render_folder_tree_node(child, depth + 1, cx)),
                        ),
                )
            } else {
                None
            })
            .into_any_element()
    }

    fn render_content_item(&self, item: &FileItem, cx: &mut Context<Self>) -> gpui::AnyElement {
        let icon = match &item.file_type {
            FileType::Folder => IconName::Folder,
            FileType::Class => IconName::Component,
            FileType::Script => IconName::Code,
            FileType::DawProject => IconName::MusicNote,
            FileType::Database => IconName::Database,
            FileType::Config => IconName::Settings,
            FileType::StructType => IconName::Box,
            FileType::EnumType => IconName::List,
            FileType::TraitType => IconName::Code,
            FileType::AliasType => IconName::Link,
            FileType::Other => IconName::Page,
        };

        let item_clone = item.clone();
        let item_path = item.path.clone();
        let item_type = item.file_type.clone();

        match self.view_mode {
            ViewMode::Grid => self.render_grid_item(item, icon, item_clone, item_path, item_type, cx),
            ViewMode::List => self.render_list_item(item, icon, item_clone, item_path, item_type, cx),
        }
    }

    fn render_grid_item(
        &self,
        item: &FileItem,
        icon: IconName,
        item_clone: FileItem,
        item_path: PathBuf,
        item_type: FileType,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // COMPACT GRID ITEM
        div()
            .id(SharedString::from(format!(
                "content-item-{}",
                item.path.display()
            )))
            .w(px(90.))
            .h(px(100.))
            .context_menu(move |menu, _window, _cx| {
                let path_str = item_path.to_string_lossy().to_string();
                match item_type {
                    FileType::Folder => menu
                        .menu(
                            "New Folder",
                            Box::new(NewFolder {
                                folder_path: path_str.clone(),
                            }),
                        )
                        .menu(
                            "New Class",
                            Box::new(NewClass {
                                folder_path: path_str.clone(),
                            }),
                        )
                        .separator()
                        .menu(
                            "Rename",
                            Box::new(RenameItem {
                                item_path: path_str.clone(),
                            }),
                        )
                        .menu(
                            "Delete",
                            Box::new(DeleteItem {
                                item_path: path_str.clone(),
                            }),
                        ),
                    FileType::Class => menu
                        .separator()
                        .menu(
                            "Rename",
                            Box::new(RenameItem {
                                item_path: path_str.clone(),
                            }),
                        )
                        .menu(
                            "Delete",
                            Box::new(DeleteItem {
                                item_path: path_str.clone(),
                            }),
                        ),
                    _ => menu
                        .menu(
                            "Rename",
                            Box::new(RenameItem {
                                item_path: path_str.clone(),
                            }),
                        )
                        .menu(
                            "Delete",
                            Box::new(DeleteItem {
                                item_path: path_str.clone(),
                            }),
                        ),
                }
            })
            .child(
                div()
                    .w_full()
                    .h_full()
                    .p_1p5()
                    .rounded(px(6.))
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.4))
                    .bg(cx.theme().background)
                    .cursor_pointer()
                    .hover(|this| {
                        this.bg(cx.theme().muted.opacity(0.25))
                            .border_color(cx.theme().primary.opacity(0.4))
                            .shadow_sm()
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |drawer, _, _, cx| {
                            drawer.handle_item_click(&item_clone, cx);
                        }),
                    )
                    .child(
                        v_flex()
                            .w_full()
                            .h_full()
                            .gap_1p5()
                            .items_center()
                            .justify_center()
                            .child(
                                // Compact icon
                                div()
                                    .size(px(44.))
                                    .rounded(px(6.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(match &item.file_type {
                                        FileType::Class => cx.theme().accent.opacity(0.12),
                                        FileType::Folder => cx.theme().primary.opacity(0.08),
                                        FileType::Script => cx.theme().info.opacity(0.12),
                                        FileType::DawProject => cx.theme().success.opacity(0.12),
                                        _ => cx.theme().muted.opacity(0.08),
                                    })
                                    .border_1()
                                    .border_color(match &item.file_type {
                                        FileType::Class => cx.theme().accent.opacity(0.25),
                                        FileType::Folder => cx.theme().primary.opacity(0.18),
                                        FileType::Script => cx.theme().info.opacity(0.25),
                                        FileType::DawProject => cx.theme().success.opacity(0.25),
                                        _ => cx.theme().border.opacity(0.25),
                                    })
                                    .child(
                                        Icon::new(icon)
                                            .size(px(24.))
                                            .text_color(match &item.file_type {
                                                FileType::Class => cx.theme().accent,
                                                FileType::Folder => cx.theme().primary,
                                                FileType::Script => cx.theme().info,
                                                FileType::DawProject => cx.theme().success,
                                                _ => cx.theme().muted_foreground,
                                            })
                                    )
                            )
                            .child(
                                if self.renaming_item.as_ref() == Some(&item.path) {
                                    div()
                                        .w_full()
                                        .child(
                                            TextInput::new(&self.rename_input_state)
                                                .appearance(false)
                                                .w_full()
                                        )
                                        .into_any_element()
                                } else {
                                    div()
                                        .w_full()
                                        .text_xs()
                                        .text_center()
                                        .font_medium()
                                        .text_color(cx.theme().foreground.opacity(0.9))
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(item.name.clone())
                                        .into_any_element()
                                }
                            )
                            .when(matches!(item.file_type, FileType::Class), |this| {
                                this.child(
                                    // Compact blueprint badge
                                    div()
                                        .px_1p5()
                                        .py_0p5()
                                        .rounded(px(3.))
                                        .bg(cx.theme().accent.opacity(0.15))
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_semibold()
                                                .text_color(cx.theme().accent)
                                                .child("BP")
                                        )
                                )
                            })
                    )
            )
            .into_any_element()
    }

    fn render_list_item(
        &self,
        item: &FileItem,
        icon: IconName,
        item_clone: FileItem,
        item_path: PathBuf,
        item_type: FileType,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        // LIST VIEW ITEM
        div()
            .id(SharedString::from(format!(
                "content-item-list-{}",
                item.path.display()
            )))
            .w_full()
            .h(px(38.))
            .context_menu(move |menu, _window, _cx| {
                let path_str = item_path.to_string_lossy().to_string();
                match item_type {
                    FileType::Folder => menu
                        .menu(
                            "New Folder",
                            Box::new(NewFolder {
                                folder_path: path_str.clone(),
                            }),
                        )
                        .menu(
                            "New Class",
                            Box::new(NewClass {
                                folder_path: path_str.clone(),
                            }),
                        )
                        .separator()
                        .menu(
                            "Rename",
                            Box::new(RenameItem {
                                item_path: path_str.clone(),
                            }),
                        )
                        .menu(
                            "Delete",
                            Box::new(DeleteItem {
                                item_path: path_str.clone(),
                            }),
                        ),
                    FileType::Class => menu
                        .separator()
                        .menu(
                            "Rename",
                            Box::new(RenameItem {
                                item_path: path_str.clone(),
                            }),
                        )
                        .menu(
                            "Delete",
                            Box::new(DeleteItem {
                                item_path: path_str.clone(),
                            }),
                        ),
                    _ => menu
                        .menu(
                            "Rename",
                            Box::new(RenameItem {
                                item_path: path_str.clone(),
                            }),
                        )
                        .menu(
                            "Delete",
                            Box::new(DeleteItem {
                                item_path: path_str.clone(),
                            }),
                        ),
                }
            })
            .child(
                div()
                    .w_full()
                    .h_full()
                    .px_3()
                    .py_1p5()
                    .rounded(px(4.))
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.2))
                    .bg(cx.theme().background)
                    .cursor_pointer()
                    .hover(|this| {
                        this.bg(cx.theme().muted.opacity(0.15))
                            .border_color(cx.theme().primary.opacity(0.3))
                    })
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |drawer, _, _, cx| {
                            drawer.handle_item_click(&item_clone, cx);
                        }),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .h_full()
                            .items_center()
                            .gap_3()
                            .child(
                                // Icon
                                div()
                                    .size(px(28.))
                                    .rounded(px(4.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .bg(match &item.file_type {
                                        FileType::Class => cx.theme().accent.opacity(0.1),
                                        FileType::Folder => cx.theme().primary.opacity(0.08),
                                        FileType::Script => cx.theme().info.opacity(0.1),
                                        FileType::DawProject => cx.theme().success.opacity(0.1),
                                        _ => cx.theme().muted.opacity(0.06),
                                    })
                                    .child(
                                        Icon::new(icon)
                                            .size(px(16.))
                                            .text_color(match &item.file_type {
                                                FileType::Class => cx.theme().accent,
                                                FileType::Folder => cx.theme().primary,
                                                FileType::Script => cx.theme().info,
                                                FileType::DawProject => cx.theme().success,
                                                _ => cx.theme().muted_foreground,
                                            })
                                    )
                            )
                            .child(
                                // Name (flex-grow)
                                if self.renaming_item.as_ref() == Some(&item.path) {
                                    div()
                                        .flex_1()
                                        .child(
                                            TextInput::new(&self.rename_input_state)
                                                .appearance(false)
                                                .w_full()
                                        )
                                        .into_any_element()
                                } else {
                                    div()
                                        .flex_1()
                                        .text_sm()
                                        .font_medium()
                                        .text_color(cx.theme().foreground)
                                        .overflow_hidden()
                                        .text_ellipsis()
                                        .whitespace_nowrap()
                                        .child(item.name.clone())
                                        .into_any_element()
                                }
                            )
                            .when(matches!(item.file_type, FileType::Class), |this| {
                                this.child(
                                    div()
                                        .px_1p5()
                                        .py_0p5()
                                        .rounded(px(3.))
                                        .bg(cx.theme().accent.opacity(0.12))
                                        .child(
                                            div()
                                                .text_xs()
                                                .font_semibold()
                                                .text_color(cx.theme().accent)
                                                .child("BP")
                                        )
                                )
                            })
                            .child(
                                // Modified date
                                div()
                                    .w(px(140.))
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground.opacity(0.7))
                                    .child(Self::format_modified_time(item.modified))
                            )
                            .child(
                                // Size
                                div()
                                    .w(px(80.))
                                    .text_xs()
                                    .text_right()
                                    .text_color(cx.theme().muted_foreground.opacity(0.7))
                                    .child(
                                        if matches!(item.file_type, FileType::Folder) {
                                            "-".to_string()
                                        } else {
                                            Self::format_file_size(item.size)
                                        }
                                    )
                            )
                    )
            )
            .into_any_element()
    }
}

#[derive(Clone, Debug)]
pub struct FileSelected {
    pub path: PathBuf,
    pub file_type: FileType,
}

#[derive(Clone, Debug)]
pub struct PopoutFileManagerEvent {
    pub project_path: Option<PathBuf>,
}

impl EventEmitter<FileSelected> for FileManagerDrawer {}
impl EventEmitter<PopoutFileManagerEvent> for FileManagerDrawer {}

impl Render for FileManagerDrawer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let contents = self.get_folder_contents();
        let breadcrumbs = self.get_breadcrumbs();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().background)
            .border_t_1()
            .border_color(cx.theme().border)
            .on_action(cx.listener(Self::on_new_folder))
            .on_action(cx.listener(Self::on_new_class))
            .on_action(cx.listener(Self::on_new_file))
            .on_action(cx.listener(Self::on_delete_item))
            .on_action(cx.listener(Self::on_rename_item))
            .on_action(cx.listener(Self::on_commit_rename))
            .on_action(cx.listener(Self::on_cancel_rename))
            .on_action(cx.listener(Self::on_refresh))
            .on_action(cx.listener(Self::on_collapse_all))
            .on_action(cx.listener(Self::on_toggle_hidden))
            .child(
                // Vertical resizable for drawer height with resizable from top edge
                v_resizable("file-manager-height", self.height_resizable_state.clone())
                    .child(
                        resizable_panel()
                            .size(px(350.))
                            .size_range(px(200.)..px(800.))
                            .child(
                                v_flex()
                                    .size_full()
                                    .child(
                                        // SLEEK MODERN HEADER with popout
                                        v_flex()
                                            .w_full()
                                            .gap_2()
                                            .px_3()
                                            .py_2p5()
                                            .bg(cx.theme().secondary.opacity(0.4))
                                            .border_b_1()
                                            .border_color(cx.theme().border)
                                            .child(
                                                // Main header row
                                                h_flex()
                                                    .w_full()
                                                    .items_center()
                                                    .justify_between()
                                                    // .child(
                                                    //     h_flex()
                                                    //         .items_center()
                                                    //         .gap_2()
                                                    //         .child(
                                                    //             Icon::new(IconName::Folder)
                                                    //                 .size(px(18.))
                                                    //                 .text_color(cx.theme().accent)
                                                    //         )
                                                    //         .child(
                                                    //             div()
                                                    //                 .text_sm()
                                                    //                 .font_semibold()
                                                    //                 .text_color(cx.theme().foreground)
                                                    //                 .child("File Manager")
                                                    //         )
                                                    // )
                                                )
                                                // Combined breadcrumbs and controls bar
                                                .child(
                                                    h_flex()
                                                    .w_full()
                                                    .items_center()
                                                    .justify_between()
                                                    .gap_2()
                                                    .child(
                                                        // Breadcrumb navigation
                                                        h_flex()
                                                        .flex_1()
                                                        .items_center()
                                                        .gap_1()
                                                            .overflow_hidden()
                                                            .children(
                                                                breadcrumbs.iter().enumerate().flat_map(|(i, (name, path))| {
                                                                    let path_clone = path.clone();
                                                                    let is_last = i == breadcrumbs.len() - 1;
                                                                    
                                                                    let mut elements: Vec<gpui::AnyElement> = vec![];
                                                                    
                                                                    if i > 0 {
                                                                        elements.push(
                                                                            Icon::new(IconName::ChevronRight)
                                                                                .size(px(12.))
                                                                                .text_color(cx.theme().muted_foreground.opacity(0.4))
                                                                                .into_any_element()
                                                                            );
                                                                        }
                                                                        
                                                                        elements.push(
                                                                            Button::new(SharedString::from(format!("breadcrumb-{}", i)))
                                                                            .ghost()
                                                                            .compact()
                                                                            .label(name.clone())
                                                                            .when(is_last, |btn| btn.primary())
                                                                            .on_click(cx.listener(move |drawer, _, _, cx| {
                                                                                drawer.select_folder(path_clone.clone(), cx);
                                                                            }))
                                                                            .into_any_element()
                                                                    );
                                                                    
                                                                    elements
                                                                })
                                                            )
                                                        )
                                                        .child(
                                                            // Item count and controls
                                                        h_flex()
                                                            .items_center()
                                                            .gap_2()
                                                            .child(
                                                                h_flex()
                                                                    .items_center()
                                                                    .gap_1p5()
                                                                    .child(
                                                                        Icon::new(IconName::Page)
                                                                            .size(px(14.))
                                                                            .text_color(cx.theme().primary)
                                                                    )
                                                                    .child(
                                                                        div()
                                                                            .px_1p5()
                                                                            .py_0p5()
                                                                            .rounded(px(3.))
                                                                            .bg(cx.theme().muted.opacity(0.3))
                                                                            .child(
                                                                                div()
                                                                                    .text_xs()
                                                                                    .text_color(cx.theme().muted_foreground)
                                                                                    .child(format!("{} items", contents.len()))
                                                                                )
                                                                    )
                                                                )
                                                                .child(
                                                                h_flex()
                                                                .gap_0p5()
                                                                    .when(self.selected_folder.is_some(), |this| {
                                                                        let folder = self.selected_folder.clone().unwrap();
                                                                        let folder_str = folder.to_string_lossy().to_string();
                                                                        let folder_str1 = folder_str.clone();
                                                                        let folder_str2 = folder_str.clone();
                                                                        let folder_str3 = folder_str.clone();
                                                                        this.child(
                                                                            Button::new("new-folder-quick")
                                                                                .ghost()
                                                                                .compact()
                                                                                .icon(IconName::FolderPlus)
                                                                                .tooltip("New Folder")
                                                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                                                    cx.dispatch_action(&NewFolder {
                                                                                        folder_path: folder_str1.clone(),
                                                                                    });
                                                                                }))
                                                                        )
                                                                        .child(
                                                                            Button::new("new-class-quick")
                                                                                .ghost()
                                                                                .compact()
                                                                                .icon(IconName::Component)
                                                                                .tooltip("New Class")
                                                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                                                    cx.dispatch_action(&NewClass {
                                                                                        folder_path: folder_str2.clone(),
                                                                                    });
                                                                                }))
                                                                        )
                                                                        .child(
                                                                            Button::new("new-file-quick")
                                                                                .ghost()
                                                                                .compact()
                                                                                .icon(IconName::Page)
                                                                                .tooltip("New File")
                                                                                .on_click(cx.listener(move |_, _, _, cx| {
                                                                                    cx.dispatch_action(&NewFile {
                                                                                        folder_path: folder_str3.clone(),
                                                                                    });
                                                                                }))
                                                                        )
                                                                    })
                                                                    .child(
                                                                        Button::new("refresh")
                                                                            .ghost()
                                                                            .compact()
                                                                            .icon(IconName::Refresh)
                                                                            .tooltip("Refresh")
                                                                            .on_click(cx.listener(|_, _, _, cx| {
                                                                                cx.dispatch_action(&RefreshFileManager);
                                                                            }))
                                                                    )
                                                                    .child(
                                                                        Button::new("view-grid")
                                                                            .ghost()
                                                                            .compact()
                                                                            .icon(IconName::LayoutDashboard)
                                                                            .tooltip("Grid View")
                                                                            .when(self.view_mode == ViewMode::Grid, |btn| btn.primary())
                                                                            .on_click(cx.listener(|drawer, _, _, cx| {
                                                                                drawer.toggle_view_mode(ViewMode::Grid, cx);
                                                                            }))
                                                                    )
                                                                    .child(
                                                                        Button::new("view-list")
                                                                            .ghost()
                                                                            .compact()
                                                                            .icon(IconName::List)
                                                                            .tooltip("List View")
                                                                            .when(self.view_mode == ViewMode::List, |btn| btn.primary())
                                                                            .on_click(cx.listener(|drawer, _, _, cx| {
                                                                                drawer.toggle_view_mode(ViewMode::List, cx);
                                                                            }))
                                                                    )
                                                                    .child(
                                                                        Button::new("sort-button")
                                                                            .ghost()
                                                                            .compact()
                                                                            .icon(IconName::ChevronsUpDown)
                                                                            .tooltip("Sort By")
                                                                            .on_click(cx.listener(|drawer, _, _, cx| {
                                                                                let next_sort = match drawer.sort_by {
                                                                                    SortBy::Name => SortBy::DateModified,
                                                                                    SortBy::DateModified => SortBy::Size,
                                                                                    SortBy::Size => SortBy::Type,
                                                                                    SortBy::Type => SortBy::Name,
                                                                                };
                                                                                drawer.set_sort_by(next_sort, cx);
                                                                            }))
                                                                    )
                                                                    .child(
                                                                        Button::new("filter-button")
                                                                            .ghost()
                                                                            .compact()
                                                                            .icon(IconName::Filter)
                                                                            .tooltip("Filter")
                                                                            .when(!self.selected_file_types.is_empty(), |btn| btn.primary())
                                                                        )
                                                                        .child(
                                                                            Button::new("toggle-hidden")
                                                                            .ghost()
                                                                            .compact()
                                                                            .icon(IconName::Eye)
                                                                            .tooltip(if self.show_hidden_files { "Hide Hidden Files" } else { "Show Hidden Files" })
                                                                            .when(self.show_hidden_files, |btn| btn.primary())
                                                                            .on_click(cx.listener(|_, _, _, cx| {
                                                                                cx.dispatch_action(&ToggleHiddenFiles);
                                                                            }))
                                                                        )
                                                                    )
                                                                    .when(!self.is_in_window, |this| {
                                                                        this.child(
                                                                            // Popout button - only show when in drawer
                                                                            Button::new("popout-drawer")
                                                                                .ghost()
                                                                                .compact()
                                                                                .icon(IconName::ExternalLink)
                                                                                .tooltip("Open in Separate Window")
                                                                                .on_click(cx.listener(|drawer, _, _, cx| {
                                                                                    let project_path = drawer.project_path.clone();
                                                                                    cx.emit(PopoutFileManagerEvent { project_path });
                                                                                }))
                                                                        )
                                                                    })
                                                    )
                                            )
                                    )
                                    .child(
                                        // Content split: tree and grid
                                        div()
                                        .flex_1()
                                            .overflow_hidden()
                                            .child(
                                                h_resizable("file-manager-split", self.resizable_state.clone())
                                                    .child(
                                                        resizable_panel()
                                                            .size(px(260.))
                                                            .size_range(px(180.)..px(450.))
                                                            .child(
                                                                v_flex()
                                                                    .size_full()
                                                                    .bg(cx.theme().secondary.opacity(0.2))
                                                                    .border_r_1()
                                                                    .border_color(cx.theme().border.opacity(0.5))
                                                                    .child(
                                                                        // COMPACT TREE HEADER
                                                                        v_flex()
                                                                            .w_full()
                                                                            .gap_1p5()
                                                                            .px_2p5()
                                                                            .py_2()
                                                                            .border_b_1()
                                                                            .border_color(cx.theme().border.opacity(0.3))
                                                                            .child(
                                                                                // Header with actions
                                                                                h_flex()
                                                                                    .w_full()
                                                                                    .items_center()
                                                                                    .justify_between()
                                                                                    .child(
                                                                                        div()
                                                                                            .flex_shrink_0()
                                                                                            .text_xs()
                                                                                            .font_semibold()
                                                                                            .text_color(cx.theme().muted_foreground)
                                                                                            .child("FOLDERS")
                                                                                    )
                                                                                    .child(
                                                                                        h_flex()
                                                                                            .gap_0p5()
                                                                                            .child(
                                                                                                Button::new("collapse-all-folders")
                                                                                                    .ghost()
                                                                                                    .compact()
                                                                                                    .icon(IconName::Minimize)
                                                                                                    .tooltip("Collapse All")
                                                                                                    .on_click(cx.listener(|_, _, _, cx| {
                                                                                                        cx.dispatch_action(&CollapseAllFolders);
                                                                                                    }))
                                                                                            )
                                                                                    )
                                                                            )
                                                                            .child(
                                                                                // Functional search
                                                                                div()
                                                                                    .w_full()
                                                                                    .child(
                                                                                        TextInput::new(&self.folder_search_state)
                                                                                            .prefix(Icon::new(IconName::Search).size(px(12.)))
                                                                                            .w_full()
                                                                                    )
                                                                            )
                                                                    )
                                                                    .child(
                                                                        // TREE CONTENT with proper sizing
                                                                        div()
                                                                            .flex_1()
                                                                            .w_full()
                                                                            .child(
                                                                                div()
                                                                                    .p_1p5()
                                                                                    .scrollable(Axis::Vertical)
                                                                                    .child(
                                                                                if let Some(tree) = &self.folder_tree {
                                                                                    v_flex()
                                                                                        .w_full()
                                                                                        .gap_0p5()
                                                                                        .children(tree.children.iter().map(
                                                                                            |child| {
                                                                                                self.render_folder_tree_node(
                                                                                                    child, 0, cx,
                                                                                                )
                                                                                            },
                                                                                        ))
                                                                                        .into_any_element()
                                                                                } else {
                                                                                    // Compact empty state
                                                                                    v_flex()
                                                                                        .w_full()
                                                                                        .p_4()
                                                                                        .gap_2()
                                                                                        .items_center()
                                                                                        .justify_center()
                                                                                        .child(
                                                                                            Icon::new(IconName::Folder)
                                                                                                .size(px(40.))
                                                                                                .text_color(cx.theme().muted_foreground.opacity(0.25))
                                                                                        )
                                                                                        .child(
                                                                                            div()
                                                                                                .text_xs()
                                                                                                .font_medium()
                                                                                                .text_color(cx.theme().muted_foreground.opacity(0.6))
                                                                                                .child("No Project")
                                                                                        )
                                                                                        .into_any_element()
                                                                                },
                                                                            )
                                                                        ),
                                                                    ),
                                                            ),
                                                    )
                                                    .child(
                                                        resizable_panel().child(
                                                            v_flex()
                                                                .size_full()
                                                                .bg(cx.theme().background)
                                                                .child(
                                                                    // COMPACT CONTENT HEADER
                                                                    v_flex()
                                                                        .w_full()
                                                                        .gap_1p5()
                                                                        .px_2p5()
                                                                        .py_2()
                                                                        .border_b_1()
                                                                        .border_color(cx.theme().border.opacity(0.3))
                                                                        .child(
                                                                            // File filter search
                                                                            div()
                                                                                .w_full()
                                                                                .child(
                                                                                    TextInput::new(&self.file_filter_state)
                                                                                        .prefix(Icon::new(IconName::Search).size(px(12.)))
                                                                                        .w_full()
                                                                                )
                                                                        )
                                                                )
                                                                .child(
                                                                    // CONTENT GRID/LIST with proper sizing - wrap the child in scrollable
                                                                    div()
                                                                        .id("file-manager-content-area")
                                                                        .flex_1()
                                                                        .w_full()
                                                                        .context_menu({
                                                                            let selected_folder_for_menu = self.selected_folder.clone();
                                                                            move |menu, _window, _cx| {
                                                                                if let Some(folder) = &selected_folder_for_menu {
                                                                                    let folder_str =
                                                                                        folder.to_string_lossy().to_string();
                                                                                    menu.menu(
                                                                                        "New Folder",
                                                                                        Box::new(NewFolder {
                                                                                            folder_path: folder_str.clone(),
                                                                                        }),
                                                                                    )
                                                                                    .menu(
                                                                                        "New Class",
                                                                                        Box::new(NewClass {
                                                                                            folder_path: folder_str.clone(),
                                                                                        }),
                                                                                    )
                                                                                    .menu(
                                                                                        "New File",
                                                                                        Box::new(NewFile {
                                                                                            folder_path: folder_str.clone(),
                                                                                        }),
                                                                                    )
                                                                                } else {
                                                                                    menu
                                                                                }
                                                                            }
                                                                        })
                                                                        .child(
                                                                            div()
                                                                                .p_2()
                                                                                .scrollable(Axis::Vertical)
                                                                                .child(
                                                                            match self.view_mode {
                                                                                ViewMode::Grid => {
                                                                                    h_flex().w_full().flex_wrap().gap_1p5().children(
                                                                                        contents.iter().map(|item| {
                                                                                            self.render_content_item(item, cx)
                                                                                        }),
                                                                                    )
                                                                                    .into_any_element()
                                                                                }
                                                                                ViewMode::List => {
                                                                                    v_flex().w_full().gap_1().children(
                                                                                        contents.iter().map(|item| {
                                                                                            self.render_content_item(item, cx)
                                                                                        }),
                                                                                    )
                                                                                    .into_any_element()
                                                                                }
                                                                            }
                                                                        )
                                                                    )
                                                                )
                                                        ),
                                                    )
                                            )
                                    )
                            )
                    )
            )
    }
}
