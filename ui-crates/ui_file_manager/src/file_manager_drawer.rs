use gpui::prelude::*;
use gpui::*;
use rust_i18n::t;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use ui::{
    button::{Button, ButtonGroup, ButtonVariants as _},
    h_flex, v_flex,
    input::{InputState, TextInput},
    resizable::{h_resizable, resizable_panel, ResizableState},
    menu::context_menu::ContextMenuExt,
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt, Selectable as _,
};

// Import from our modular structure
use crate::drawer::{
    actions::*,
    types::*,
    tree::FolderNode,
    operations::FileOperations,
    utils::*,
    context_menus,
};

// ============================================================================
// FILE MANAGER DRAWER
// ============================================================================

pub struct FileManagerDrawer {
    pub project_path: Option<PathBuf>,
    folder_tree: Option<FolderNode>,
    selected_folder: Option<PathBuf>,
    selected_items: HashSet<PathBuf>,

    // File operations
    operations: FileOperations,

    // Drag and drop
    drag_state: DragState,

    // UI state
    resizable_state: Entity<ResizableState>,
    view_mode: ViewMode,
    sort_by: SortBy,
    sort_order: SortOrder,
    show_hidden_files: bool,

    // Rename state
    renaming_item: Option<PathBuf>,
    rename_input_state: Entity<InputState>,

    // Cached registered file types from plugin system
    registered_file_types: Vec<plugin_editor_api::FileTypeDefinition>,

    // Search
    search_query: String,
    folder_search_state: Entity<InputState>,
    file_filter_query: String,
    file_filter_state: Entity<InputState>,

    // Clipboard
    clipboard: Option<(Vec<PathBuf>, bool)>, // (paths, is_cut)
}

impl FileManagerDrawer {
    pub fn new(project_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let resizable_state = ResizableState::new(cx);
        let rename_input_state = cx.new(|cx| InputState::new(window, cx));
        let folder_search_state = cx.new(|cx| InputState::new(window, cx));
        let file_filter_state = cx.new(|cx| InputState::new(window, cx));

        // Simple rename subscription - only handle Enter
        cx.subscribe(
            &rename_input_state,
            |drawer, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::PressEnter { .. } = event {
                    drawer.commit_rename(cx);
                }
            },
        )
        .detach();

        // Subscribe to search inputs
        cx.subscribe(
            &folder_search_state,
            |drawer, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::Change { .. } = event {
                    drawer.search_query = drawer.folder_search_state.read(cx).text().to_string();
                    cx.notify();
                }
            },
        )
        .detach();

        cx.subscribe(
            &file_filter_state,
            |drawer, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::Change { .. } = event {
                    drawer.file_filter_query = drawer.file_filter_state.read(cx).text().to_string();
                    cx.notify();
                }
            },
        )
        .detach();

        let operations = FileOperations::new(project_path.clone());

        Self {
            folder_tree: project_path.as_ref().and_then(|p| FolderNode::from_path(p)),
            project_path: project_path.clone(),
            selected_folder: project_path,
            selected_items: HashSet::new(),
            operations,
            drag_state: DragState::None,
            resizable_state,
            renaming_item: None,
            rename_input_state,
            view_mode: ViewMode::Grid,
            sort_by: SortBy::Name,
            sort_order: SortOrder::Ascending,
            search_query: String::new(),
            folder_search_state,
            file_filter_query: String::new(),
            file_filter_state,
            show_hidden_files: false,
            clipboard: None,
            registered_file_types: Vec::new(), // Will be populated from plugin manager
        }
    }

    pub fn new_in_window(project_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new(project_path, window, cx)
    }

    /// Update registered file types from the plugin manager
    pub fn update_file_types(&mut self, file_types: Vec<plugin_editor_api::FileTypeDefinition>) {
        self.registered_file_types = file_types;
    }

    // ========================================================================
    // ACTION HANDLERS
    // ========================================================================

    fn handle_create_asset(&mut self, action: &CreateAsset, cx: &mut Context<Self>) {
        if let Some(folder) = &self.selected_folder {
            // Create file name with extension
            let file_name = format!("New{}.{}", action.display_name.replace(" ", ""), action.extension);
            let file_path = folder.join(&file_name);

            // Check if file already exists and generate unique name
            let mut counter = 1;
            let mut final_path = file_path.clone();
            while final_path.exists() {
                let file_name = format!("New{}_{}.{}", action.display_name.replace(" ", ""), counter, action.extension);
                final_path = folder.join(&file_name);
                counter += 1;
            }

            // Create the file with default content from the file type definition
            let content = if action.default_content.is_null() {
                // For SQLite databases, create an empty database file
                if action.extension == "db" || action.extension == "sqlite" || action.extension == "sqlite3" {
                    // SQLite databases need proper initialization, which will be handled by the editor
                    // For now, create an empty file that the editor will initialize
                    vec![]
                } else {
                    // For other files, use empty content
                    vec![]
                }
            } else {
                // Use the default content from the file type definition
                action.default_content.to_string().into_bytes()
            };

            if let Err(e) = std::fs::write(&final_path, content) {
                tracing::error!("Failed to create file {:?}: {}", final_path, e);
            } else {
                // Refresh the folder tree
                if let Some(ref path) = self.project_path {
                    self.folder_tree = FolderNode::from_path(path);
                }
                cx.notify();
            }
        }
    }

    fn handle_new_folder(&mut self, _action: &NewFolder, cx: &mut Context<Self>) {
        if let Some(folder) = &self.selected_folder {
            // Create folder with unique name
            let mut counter = 1;
            let mut folder_name = "NewFolder".to_string();
            let mut folder_path = folder.join(&folder_name);

            while folder_path.exists() {
                folder_name = format!("NewFolder_{}", counter);
                folder_path = folder.join(&folder_name);
                counter += 1;
            }

            // Create the folder
            if let Err(e) = std::fs::create_dir(&folder_path) {
                tracing::error!("Failed to create folder {:?}: {}", folder_path, e);
            } else {
                // Refresh the folder tree
                if let Some(ref path) = self.project_path {
                    self.folder_tree = FolderNode::from_path(path);
                }
                cx.notify();
            }
        }
    }

    fn handle_delete_item(&mut self, cx: &mut Context<Self>) {
        let items_to_delete: Vec<PathBuf> = self.selected_items.iter().cloned().collect();

        for item in items_to_delete {
            if item.is_dir() {
                if let Err(e) = std::fs::remove_dir_all(&item) {
                    tracing::error!("Failed to delete folder {:?}: {}", item, e);
                }
            } else {
                if let Err(e) = std::fs::remove_file(&item) {
                    tracing::error!("Failed to delete file {:?}: {}", item, e);
                }
            }
        }

        self.selected_items.clear();

        // Refresh the folder tree
        if let Some(ref path) = self.project_path {
            self.folder_tree = FolderNode::from_path(path);
        }

        cx.notify();
    }

    fn handle_rename_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(item) = self.selected_items.iter().next().cloned() {
            self.start_rename(item, window, cx);
        }
    }

    fn handle_duplicate_item(&mut self, cx: &mut Context<Self>) {
        let items_to_duplicate: Vec<PathBuf> = self.selected_items.iter().cloned().collect();

        for item in items_to_duplicate {
            if let Some(parent) = item.parent() {
                if let Some(name) = item.file_name() {
                    let name_str = name.to_string_lossy();

                    // Generate unique name
                    let mut counter = 1;
                    let mut new_name = format!("{}_copy", name_str);
                    let mut new_path = parent.join(&new_name);

                    while new_path.exists() {
                        new_name = format!("{}_copy_{}", name_str, counter);
                        new_path = parent.join(&new_name);
                        counter += 1;
                    }

                    // Copy the item
                    if item.is_dir() {
                        if let Err(e) = Self::copy_dir_recursive(&item, &new_path) {
                            tracing::error!("Failed to duplicate folder {:?}: {}", item, e);
                        }
                    } else {
                        if let Err(e) = std::fs::copy(&item, &new_path) {
                            tracing::error!("Failed to duplicate file {:?}: {}", item, e);
                        }
                    }
                }
            }
        }

        // Refresh the folder tree
        if let Some(ref path) = self.project_path {
            self.folder_tree = FolderNode::from_path(path);
        }

        cx.notify();
    }

    fn handle_copy(&mut self, _cx: &mut Context<Self>) {
        let items: Vec<PathBuf> = self.selected_items.iter().cloned().collect();
        self.clipboard = Some((items, false));
    }

    fn handle_cut(&mut self, _cx: &mut Context<Self>) {
        let items: Vec<PathBuf> = self.selected_items.iter().cloned().collect();
        self.clipboard = Some((items, true));
    }

    fn handle_paste(&mut self, cx: &mut Context<Self>) {
        if let Some((items, is_cut)) = &self.clipboard {
            if let Some(target_folder) = &self.selected_folder {
                for item in items.iter() {
                    if let Some(name) = item.file_name() {
                        let target_path = target_folder.join(name);

                        if *is_cut {
                            // Move operation
                            if let Err(e) = std::fs::rename(item, &target_path) {
                                tracing::error!("Failed to move {:?} to {:?}: {}", item, target_path, e);
                            }
                        } else {
                            // Copy operation
                            if item.is_dir() {
                                if let Err(e) = Self::copy_dir_recursive(item, &target_path) {
                                    tracing::error!("Failed to copy folder {:?}: {}", item, e);
                                }
                            } else {
                                if let Err(e) = std::fs::copy(item, &target_path) {
                                    tracing::error!("Failed to copy file {:?}: {}", item, e);
                                }
                            }
                        }
                    }
                }

                // Clear clipboard if it was a cut operation
                if *is_cut {
                    self.clipboard = None;
                }

                // Refresh the folder tree
                if let Some(ref path) = self.project_path {
                    self.folder_tree = FolderNode::from_path(path);
                }

                cx.notify();
            }
        }
    }

    fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;

        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let src_path = entry.path();
            let dst_path = dst.join(entry.file_name());

            if src_path.is_dir() {
                Self::copy_dir_recursive(&src_path, &dst_path)?;
            } else {
                std::fs::copy(&src_path, &dst_path)?;
            }
        }

        Ok(())
    }

    // ========================================================================
    // NEW CONTEXT MENU ACTION HANDLERS
    // ========================================================================

    fn handle_open_in_file_manager(&mut self, _action: &OpenInFileManager, _cx: &mut Context<Self>) {
        // Get the path to open - either the selected folder or first selected item
        let path_to_open = if let Some(folder) = &self.selected_folder {
            Some(folder.clone())
        } else {
            self.selected_items.iter().next().cloned()
        };

        if let Some(path) = path_to_open {
            #[cfg(target_os = "windows")]
            {
                let _ = std::process::Command::new("explorer")
                    .arg(path.to_string_lossy().to_string())
                    .spawn();
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("open")
                    .arg(path.to_string_lossy().to_string())
                    .spawn();
            }
            #[cfg(target_os = "linux")]
            {
                let _ = std::process::Command::new("xdg-open")
                    .arg(path.to_string_lossy().to_string())
                    .spawn();
            }
        }
    }

    fn handle_open_terminal_here(&mut self, _action: &OpenTerminalHere, _cx: &mut Context<Self>) {
        let folder = self.selected_folder.clone().or_else(|| {
            self.selected_items.iter().next().and_then(|p| {
                if p.is_dir() {
                    Some(p.clone())
                } else {
                    p.parent().map(|p| p.to_path_buf())
                }
            })
        });

        if let Some(folder) = folder {
            #[cfg(target_os = "windows")]
            {
                let _ = std::process::Command::new("cmd")
                    .args(&["/c", "start", "cmd"])
                    .current_dir(folder)
                    .spawn();
            }
            #[cfg(target_os = "macos")]
            {
                let _ = std::process::Command::new("open")
                    .args(&["-a", "Terminal"])
                    .arg(folder.to_string_lossy().to_string())
                    .spawn();
            }
            #[cfg(target_os = "linux")]
            {
                // Try common terminal emulators
                for term in &["gnome-terminal", "konsole", "xterm"] {
                    if std::process::Command::new(term)
                        .current_dir(&folder)
                        .spawn()
                        .is_ok()
                    {
                        break;
                    }
                }
            }
        }
    }

    fn handle_validate_asset(&mut self, _action: &ValidateAsset, _cx: &mut Context<Self>) {
        // TODO: Implement asset validation
        // This would check if the asset file is valid according to its type
        tracing::info!("Validate asset action triggered - not yet implemented");
    }

    fn handle_toggle_favorite(&mut self, _action: &ToggleFavorite, _cx: &mut Context<Self>) {
        // TODO: Implement favorite toggling
        // This would mark/unmark files as favorites (stored in project settings)
        tracing::info!("Toggle favorite action triggered - not yet implemented");
    }

    fn handle_toggle_gitignore(&mut self, _action: &ToggleGitignore, cx: &mut Context<Self>) {
        if let Some(item) = self.selected_items.iter().next() {
            if let Some(project_path) = &self.project_path {
                let gitignore_path = project_path.join(".gitignore");
                
                // Read existing .gitignore or create empty string
                let content = std::fs::read_to_string(&gitignore_path).unwrap_or_default();
                
                // Get relative path from project root
                if let Ok(relative_path) = item.strip_prefix(project_path) {
                    let pattern = relative_path.to_string_lossy().replace('\\', "/");
                    
                    if content.lines().any(|line| line.trim() == pattern) {
                        // Remove from .gitignore
                        let new_content: String = content
                            .lines()
                            .filter(|line| line.trim() != pattern)
                            .collect::<Vec<_>>()
                            .join("\n");
                        let _ = std::fs::write(&gitignore_path, new_content);
                        tracing::info!("Removed {} from .gitignore", pattern);
                    } else {
                        // Add to .gitignore
                        let new_content = if content.is_empty() {
                            pattern
                        } else {
                            format!("{}\n{}", content.trim_end(), pattern)
                        };
                        let _ = std::fs::write(&gitignore_path, new_content);
                        tracing::info!("Added pattern to .gitignore");
                    }
                    
                    
                    cx.notify();
                }
            }
        }
    }

    fn handle_toggle_hidden(&mut self, _action: &ToggleHidden, _cx: &mut Context<Self>) {
        // TODO: Implement hidden file toggling
        // This would mark files as hidden (on Windows, set hidden attribute; on Unix, rename with dot prefix)
        tracing::info!("Toggle hidden action triggered - not yet implemented");
    }

    fn handle_show_history(&mut self, _action: &ShowHistory, _cx: &mut Context<Self>) {
        // TODO: Implement file history viewer
        // This would show git history or file modification history
        tracing::info!("Show history action triggered - not yet implemented");
    }

    fn handle_check_multiuser_sync(&mut self, _action: &CheckMultiuserSync, _cx: &mut Context<Self>) {
        // TODO: Implement multiuser sync check
        // This would check if all connected peers have this file synced
        tracing::info!("Check multiuser sync action triggered - not yet implemented");
    }

    pub fn set_project_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        tracing::debug!("[FILE_MANAGER] set_project_path called with: {:?}", path);
        tracing::debug!("[FILE_MANAGER] Path exists: {}", path.exists());
        tracing::debug!("[FILE_MANAGER] Path is_dir: {}", path.is_dir());

        self.project_path = Some(path.clone());
        self.folder_tree = FolderNode::from_path(&path);
        self.selected_folder = Some(path.clone());

        tracing::debug!("[FILE_MANAGER] folder_tree is_some: {}", self.folder_tree.is_some());
        tracing::debug!("[FILE_MANAGER] selected_folder: {:?}", self.selected_folder);
        
        cx.notify();
    }

    // ========================================================================
    // RENDERING
    // ========================================================================

    fn render_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_resizable("file-manager-resizable", self.resizable_state.clone())
            .child(
                resizable_panel()
                    .child(self.render_folder_tree(window, cx))
                    .size(px(250.))
            )
            .child(
                resizable_panel()
                    .child(self.render_file_content(window, cx))
            )
    }

    fn render_folder_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                // Folder tree header with improved styling
                v_flex()
                    .w_full()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().sidebar)
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Icon::new(IconName::Folder)
                                            .size_4()
                                            .text_color(cx.theme().foreground)
                                    )
                                    .child(
                                        div()
                                            .text_base()
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(cx.theme().foreground)
                                            .child(t!("FileManager.ProjectFiles").to_string())
                                    )
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Button::new("expand-all")
                                            .icon(IconName::ChevronDown)
                                            .ghost()
                                            .xsmall()
                                            .tooltip(t!("FileManager.ExpandAll").to_string())
                                            .on_click(cx.listener(|drawer, _event, _window, cx| {
                                                if let Some(ref mut tree) = drawer.folder_tree {
                                                    tree.expand_all();
                                                    cx.notify();
                                                }
                                            }))
                                    )
                                    .child(
                                        Button::new("collapse-all")
                                            .icon(IconName::ChevronUp)
                                            .ghost()
                                            .xsmall()
                                            .tooltip(t!("FileManager.CollapseAll").to_string())
                                            .on_click(cx.listener(|drawer, _event, _window, cx| {
                                                if let Some(ref mut tree) = drawer.folder_tree {
                                                    tree.collapse_all();
                                                    cx.notify();
                                                }
                                            }))
                                    )
                            )
                    )
                    // Search box for folder tree
                    .child(
                        div()
                            .w_full()
                            .child(
                                TextInput::new(&self.folder_search_state)
                                    .w_full()
                                    .prefix(
                                        Icon::new(IconName::Search)
                                            .size_3()
                                            .text_color(cx.theme().muted_foreground)
                                    )
                            )
                    )
            )
            .child(
                // Folder tree content - SCROLLABLE with enhanced empty state
                div()
                    .flex_1()
                    .overflow_hidden()
                    .when_some(self.folder_tree.clone(), |this, tree| {
                        this.child(
                            v_flex()
                                .size_full()
                                .p_2()
                                .gap_px()
                                .scrollable(gpui::Axis::Vertical)
                                .child(self.render_folder_node(&tree, 0, window, cx))
                        )
                    })
                    .when(self.folder_tree.is_none(), |this| {
                        this.child(
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .p_4()
                                .child(
                                    v_flex()
                                        .gap_3()
                                        .items_center()
                                        .max_w(px(200.0))
                                        .px_4()
                                        .py_6()
                                        .rounded_lg()
                                        .bg(cx.theme().secondary.opacity(0.2))
                                        .border_1()
                                        .border_color(cx.theme().border.opacity(0.3))
                                        .child(
                                            div()
                                                .w(px(48.0))
                                                .h(px(48.0))
                                                .rounded_full()
                                                .bg(cx.theme().muted.opacity(0.3))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(
                                                    Icon::new(IconName::FolderOpen)
                                                        .size(px(24.0))
                                                        .text_color(cx.theme().muted_foreground)
                                                )
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(cx.theme().foreground)
                                                .child("No Project")
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_center()
                                                .text_color(cx.theme().muted_foreground)
                                                .line_height(rems(1.4))
                                                .child("Open a project folder to see files")
                                        )
                                )
                        )
                    })
            )
    }

    fn render_folder_node(&mut self, node: &FolderNode, depth: usize, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_selected = self.selected_folder.as_ref() == Some(&node.path);
        let path = node.path.clone();
        let path_for_expand = path.clone();
        let expanded = node.expanded;
        let has_children = !node.children.is_empty();
        let folder_id = format!("folder-{}", path.display());
        let indent = px(depth as f32 * 20.0 + 4.0);
        let icon = if expanded { IconName::FolderOpen } else { IconName::Folder };
        let icon_color = ui::hierarchical_tree::tree_colors::FOLDER;

        let text_color = if is_selected {
            cx.theme().accent_foreground
        } else {
            cx.theme().foreground
        };

        let muted_color = if is_selected {
            cx.theme().accent_foreground.opacity(0.7)
        } else {
            cx.theme().muted_foreground
        };

        let mut item_div = h_flex()
            .id(SharedString::from(folder_id))
            .w_full()
            .items_center()
            .gap_1()
            .h_7()
            .pl(indent)
            .pr_2()
            .rounded(px(4.0))
            .cursor_pointer();

        if is_selected {
            item_div = item_div
                .bg(cx.theme().accent)
                .shadow_sm();
        } else {
            item_div = item_div.hover(|style| style.bg(cx.theme().muted.opacity(0.3)));
        }

        v_flex()
            .w_full()
            .child(
                item_div
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |drawer, _event: &MouseDownEvent, _window, cx| {
                        drawer.handle_folder_select(path.clone(), cx);
                    }))
                    .child(
                        if has_children {
                            let path_clone = path_for_expand.clone();
                            div()
                                .w_4()
                                .h_4()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(2.0))
                                .cursor_pointer()
                                .hover(|s| s.bg(cx.theme().muted.opacity(0.5)))
                                .child(
                                    Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                                        .size(px(12.0))
                                        .text_color(muted_color)
                                )
                                .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |drawer, _event: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    if let Some(ref mut tree) = drawer.folder_tree {
                                        tree.toggle_expanded(&path_clone);
                                    }
                                    cx.notify();
                                }))
                                .into_any_element()
                        } else {
                            div()
                                .w_4()
                                .into_any_element()
                        }
                    )
                    .child(
                        div()
                            .w_5()
                            .h_5()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .bg(icon_color.opacity(0.15))
                            .child(
                                Icon::new(icon)
                                    .size(px(14.0))
                                    .text_color(if is_selected { text_color } else { icon_color })
                            )
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(text_color)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(node.name.clone())
                    )
            )
            .children(
                if expanded {
                    node.children.iter().map(|child| {
                        self.render_folder_node(child, depth + 1, window, cx).into_any_element()
                    }).collect::<Vec<_>>()
                } else {
                    Vec::new()
                }
            )
    }

    fn render_file_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.get_filtered_items();
        let has_clipboard = self.clipboard.is_some();
        let selected_folder = self.selected_folder.clone();
        let file_types = self.registered_file_types.clone();

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                // Combined toolbar with path and buttons
                self.render_combined_toolbar(&items, window, cx)
            )
            .child(
                // File content - SCROLLABLE
                div()
                    .id("file-content-scroll")
                    .flex_1()
                    .p_4()
                    .overflow_y_scroll()
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(|drawer, _event, _window, cx| {
                        // Commit rename if clicking on blank area
                        if drawer.renaming_item.is_some() {
                            drawer.commit_rename(cx);
                        }
                    }))
                    .context_menu(move |menu, _window, _cx| {
                        // Show folder context menu for blank area
                        if let Some(path) = selected_folder.clone() {
                            context_menus::folder_context_menu(path, has_clipboard, file_types.clone())(menu, _window, _cx)
                        } else {
                            menu
                        }
                    })
                    .child(
                        match self.view_mode {
                            ViewMode::Grid => self.render_grid_view(&items, window, cx).into_any_element(),
                            ViewMode::List => self.render_list_view(&items, window, cx).into_any_element(),
                        }
                    )
            )
    }

    fn render_list_view(&mut self, items: &[FileItem], window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_1()
            .children(items.iter().map(|item| {
                self.render_list_item(item, window, cx)
            }))
    }

    fn render_list_item(&mut self, item: &FileItem, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_selected = self.selected_items.contains(&item.path);
        let is_renaming = self.renaming_item.as_ref() == Some(&item.path);
        let icon = get_icon_for_file_type(&item);
        let icon_color = get_icon_color_for_file_type(&item, cx.theme());
        let item_clone = item.clone();
        let item_clone2 = item.clone();
        let item_clone3 = item.clone(); // For double-click
        let item_path = item.path.clone();
        let has_clipboard = self.clipboard.is_some();
        let is_class = item.is_class();
        let _is_folder = item.is_folder;

        h_flex()
            .id(SharedString::from(format!("list-item-{}", item.name)))
            .w_full()
            .h(px(36.))
            .px_3()
            .py_1p5()
            .gap_3()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(gpui::transparent_black())
            .when(is_selected, |this| {
                this.bg(cx.theme().accent.opacity(0.1))
                    .border_color(cx.theme().accent.opacity(0.3))
                    .border_l_2()
                    .border_color(cx.theme().accent)
            })
            .hover(|this| {
                this.bg(cx.theme().secondary.opacity(0.5))
                    .border_color(cx.theme().accent.opacity(0.2))
            })
            .cursor_pointer()
            .child(
                div()
                    .w(px(24.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .bg(icon_color.opacity(0.15))
                    .child(
                        Icon::new(icon)
                            .size_4()
                            .text_color(icon_color)
                    )
            )
            .child(
                if is_renaming {
                    div()
                        .flex_1()
                        .child(
                            TextInput::new(&self.rename_input_state)
                                .w_full()
                        )
                        .into_any_element()
                } else {
                    div()
                        .flex_1()
                        .text_sm()
                        .font_weight(if is_selected {
                            gpui::FontWeight::SEMIBOLD
                        } else {
                            gpui::FontWeight::NORMAL
                        })
                        .text_color(cx.theme().foreground)
                        .child(item.name.clone())
                        .into_any_element()
                }
            )
            .when(!item.is_folder, |this| {
                this.child(
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_sm()
                        .bg(cx.theme().muted.opacity(0.2))
                        .text_xs()
                        .font_family("monospace")
                        .text_color(cx.theme().muted_foreground)
                        .child(format_file_size(item.size))
                )
            })
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |drawer, event: &MouseDownEvent, _window: &mut Window, cx| {
                if is_renaming {
                    // Stop propagation when clicking the item being renamed
                    cx.stop_propagation();
                } else {
                    // Commit any active rename before handling this item
                    if drawer.renaming_item.is_some() {
                        drawer.commit_rename(cx);
                    }
                    
                    if event.click_count == 2 {
                        drawer.handle_item_double_click(&item_clone3, cx);
                    } else {
                        drawer.handle_item_click(&item_clone, &event.modifiers, cx);
                    }
                }
            }))
            .on_mouse_down(gpui::MouseButton::Right, cx.listener(move |drawer, event: &MouseDownEvent, _window: &mut Window, cx| {
                // Select item on right-click if not already selected (without changing folder view)
                if !drawer.selected_items.contains(&item_clone2.path) {
                    drawer.selected_items.clear();
                    drawer.selected_items.insert(item_clone2.path.clone());
                    // Don't change selected_folder on right-click to avoid navigating
                    cx.notify();
                }
                // Stop propagation so parent container's context menu doesn't show
                cx.stop_propagation();
            }))
            .context_menu(move |menu, _window, _cx| {
                // All items (files and folders) use item_context_menu
                // Only blank area uses folder_context_menu with "New" options
                context_menus::item_context_menu(item_path.clone(), has_clipboard, is_class)(menu, _window, _cx)
            })
    }

    fn render_combined_toolbar(&mut self, items: &[FileItem], window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_view_mode = self.view_mode;
        
        h_flex()
            .w_full()
            .h(px(56.))
            .px_4()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(
                // Clickable breadcrumb path - takes remaining space with accent styling
                self.render_clickable_breadcrumb(&items, window, cx)
            )
            .child(
                // Item count badge
                div()
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .bg(cx.theme().accent.opacity(0.1))
                    .border_1()
                    .border_color(cx.theme().accent.opacity(0.3))
                    .text_xs()
                    .font_medium()
                    .text_color(cx.theme().accent)
                    .child(t!("FileManager.Items", count => items.len()).to_string())
            )
            // Divider
            .child(ui::divider::Divider::vertical().h(px(24.)))
            // View mode toggle group
            .child(
                ButtonGroup::new("view-mode-group")
                    .child(
                        Button::new("toggle-view")
                            .icon(IconName::LayoutDashboard)
                            .tooltip(t!("FileManager.GridView").to_string())
                            .selected(current_view_mode == ViewMode::Grid)
                    )
                    .child(
                        Button::new("toggle-list")
                            .icon(IconName::List)
                            .tooltip(t!("FileManager.ListView").to_string())
                            .selected(current_view_mode == ViewMode::List)
                    )
                    .ghost()
                    .on_click(cx.listener(|drawer, selected: &Vec<usize>, _window, cx| {
                        if selected.contains(&0) {
                            drawer.view_mode = ViewMode::Grid;
                        } else if selected.contains(&1) {
                            drawer.view_mode = ViewMode::List;
                        }
                        cx.notify();
                    }))
            )
            // Divider
            .child(ui::divider::Divider::vertical().h(px(24.)))
            // File operations group
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("new-file")
                            .icon(IconName::PagePlus)
                            .ghost()
                            .tooltip(t!("FileManager.NewFile").to_string())
                            .on_click(cx.listener(|drawer, _event, _window, cx| {
                                drawer.start_new_file(cx);
                            }))
                    )
                    .child(
                        Button::new("new-folder")
                            .icon(IconName::FolderPlus)
                            .ghost()
                            .tooltip(t!("FileManager.NewFolder").to_string())
                            .on_click(cx.listener(|drawer, _event, _window, cx| {
                                drawer.start_new_folder(cx);
                            }))
                    )
            )
            // Divider
            .child(ui::divider::Divider::vertical().h(px(24.)))
            // View options group
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("toggle-hidden")
                            .icon(if self.show_hidden_files { IconName::EyeOff } else { IconName::Eye })
                            .ghost()
                            .tooltip(if self.show_hidden_files { 
                                t!("FileManager.HideHidden").to_string() 
                            } else { 
                                t!("FileManager.ShowHidden").to_string() 
                            })
                            .on_click(cx.listener(|drawer, _event, _window, cx| {
                                drawer.show_hidden_files = !drawer.show_hidden_files;
                                cx.notify();
                            }))
                    )
                    .child(
                        Button::new("refresh")
                            .icon(IconName::Refresh)
                            .ghost()
                            .tooltip(t!("FileManager.Refresh").to_string())
                            .on_click(cx.listener(|drawer, _event, _window, cx| {
                                if let Some(ref path) = drawer.project_path {
                                    drawer.folder_tree = FolderNode::from_path(path);
                                }
                                cx.notify();
                            }))
                    )
            )
            // Divider
            .child(ui::divider::Divider::vertical().h(px(24.)))
            // Actions group
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("external")
                            .icon(IconName::ExternalLink)
                            .ghost()
                            .tooltip(t!("FileManager.OpenInFileManager").to_string())
                            .on_click(cx.listener(|drawer, _event, _window, _cx| {
                                if let Some(ref folder) = drawer.selected_folder {
                                    #[cfg(target_os = "windows")]
                                    let _ = std::process::Command::new("explorer")
                                        .arg(folder)
                                        .spawn();
                                    #[cfg(target_os = "macos")]
                                    let _ = std::process::Command::new("open")
                                        .arg(folder)
                                        .spawn();
                                    #[cfg(target_os = "linux")]
                                    let _ = std::process::Command::new("xdg-open")
                                        .arg(folder)
                                        .spawn();
                                }
                            }))
                    )
                    .child(
                        Button::new("popout")
                            .icon(IconName::ARrowUpRightSquare)
                            .ghost()
                            .tooltip("Pop Out to New Window")
                            .on_click(cx.listener(|drawer, _event, window: &mut Window, cx| {
                                let mouse_pos = window.mouse_position();
                                cx.emit(PopoutFileManagerEvent { position: mouse_pos });
                            }))
                    )
            )
    }

    fn render_clickable_breadcrumb(&mut self, _items: &[FileItem], _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut path_parts = Vec::new();
        
        // Get path components
        if let Some(ref selected) = self.selected_folder {
            if let Some(ref project) = self.project_path {
                if let Ok(relative) = selected.strip_prefix(project) {
                    let mut current = project.clone();
                    path_parts.push(("Project".to_string(), current.clone()));
                    
                    for component in relative.components() {
                        if let Some(name) = component.as_os_str().to_str() {
                            current = current.join(name);
                            path_parts.push((name.to_string(), current.clone()));
                        }
                    }
                }
            }
        }
        
        if path_parts.is_empty() {
            path_parts.push(("Project".to_string(), self.project_path.clone().unwrap_or_default()));
        }

        h_flex()
            .flex_1()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .rounded(px(8.))
            .bg(cx.theme().muted.opacity(0.3))
            .border_1()
            .border_color(cx.theme().border)
            .child(
                Icon::new(IconName::Folder)
                    .size_4()
                    .text_color(cx.theme().accent)
            )
            .children(
                path_parts.into_iter().enumerate().flat_map(|(i, (name, path))| {
                    let mut elements: Vec<gpui::AnyElement> = Vec::new();
                    
                    if i > 0 {
                        elements.push(
                            Icon::new(IconName::ChevronRight)
                                .size_3()
                                .text_color(cx.theme().muted_foreground)
                                .into_any_element()
                        );
                    }
                    
                    let path_clone = path.clone();
                    elements.push(
                        div()
                            .text_sm()
                            .px_1()
                            .py_px()
                            .rounded(px(4.))
                            .text_color(cx.theme().foreground)
                            .font_medium()
                            .cursor_pointer()
                            .hover(|style| style.bg(cx.theme().accent.opacity(0.15)))
                            .child(name)
                            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |drawer, _event: &MouseDownEvent, _window: &mut Window, cx| {
                                drawer.selected_folder = Some(path_clone.clone());
                                cx.notify();
                            }))
                            .into_any_element()
                    );
                    
                    elements
                })
            )
    }

    fn render_grid_view(&mut self, items: &[FileItem], window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .flex()
            .flex_wrap()
            .gap_3()
            .children(
                items.iter().map(|item| {
                    self.render_grid_item(item, window, cx)
                })
            )
    }

    fn render_grid_item(&mut self, item: &FileItem, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_selected = self.selected_items.contains(&item.path);
        let is_renaming = self.renaming_item.as_ref() == Some(&item.path);
        let path = item.path.clone();
        let icon = get_icon_for_file_type(&item);
        let icon_color = get_icon_color_for_file_type(&item, cx.theme());
        let item_clone = item.clone();
        let item_clone2 = item.clone();
        let item_clone3 = item.clone(); // For double-click
        let item_path = item.path.clone();
        let has_clipboard = self.clipboard.is_some();
        let is_class = item.is_class();
        let is_folder = item.is_folder;

        div()
            .w(px(100.0))
            .h(px(110.0))
            .rounded_lg()
            .border_1()
            .when(is_selected, |this| {
                this.border_color(cx.theme().accent)
                    .bg(cx.theme().accent.opacity(0.1))
                    .shadow_md()
            })
            .when(!is_selected, |this| {
                this.border_color(cx.theme().border.opacity(0.3))
                    .bg(cx.theme().sidebar.opacity(0.5))
            })
            .cursor_pointer()
            .hover(|style| {
                style
                    .bg(cx.theme().secondary.opacity(0.7))
                    .border_color(cx.theme().accent.opacity(0.7))
                    .shadow_lg()
            })
            .child(
                v_flex()
                    .id(SharedString::from(format!("grid-item-{}", item.name)))
                    .w_full()
                    .h_full()
                    .p_3()
                    .gap_2()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(48.0))
                            .rounded_lg()
                            .bg(icon_color.opacity(0.15))
                            .border_1()
                            .border_color(icon_color.opacity(0.3))
                            .flex()
                            .items_center()
                            .justify_center()
                            .shadow_sm()
                            .child(
                                Icon::new(icon)
                                    .size(px(24.0))
                                    .text_color(icon_color)
                            )
                    )
                    .child(
                        if is_renaming {
                            div()
                                .w_full()
                                .child(
                                    TextInput::new(&self.rename_input_state)
                                )
                                .into_any_element()
                        } else {
                            div()
                                .w_full()
                                .text_xs()
                                .text_center()
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .text_color(cx.theme().foreground)
                                .overflow_hidden()
                                .text_ellipsis()
                                .line_height(rems(1.3))
                                .child(item.name.clone())
                                .into_any_element()
                        }
                    )
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |drawer, event: &MouseDownEvent, _window: &mut Window, cx| {
                        if is_renaming {
                            // Stop propagation when clicking the item being renamed
                            cx.stop_propagation();
                        } else {
                            // Commit any active rename before handling this item
                            if drawer.renaming_item.is_some() {
                                drawer.commit_rename(cx);
                            }
                            
                            if event.click_count == 2 {
                                drawer.handle_item_double_click(&item_clone3, cx);
                            } else {
                                drawer.handle_item_click(&item_clone, &event.modifiers, cx);
                            }
                        }
                    }))
                    .on_mouse_down(gpui::MouseButton::Right, cx.listener(move |drawer, event: &MouseDownEvent, _window: &mut Window, cx| {
                        // Select item on right-click if not already selected (without changing folder view)
                        if !drawer.selected_items.contains(&item_clone2.path) {
                            drawer.selected_items.clear();
                            drawer.selected_items.insert(item_clone2.path.clone());
                            // Don't change selected_folder on right-click to avoid navigating
                            cx.notify();
                        }
                        // Stop propagation so parent container's context menu doesn't show
                        cx.stop_propagation();
                    }))
                    .context_menu(move |menu, _window, _cx| {
                        // All items (files and folders) use item_context_menu
                        // Only blank area uses folder_context_menu with "New" options
                        context_menus::item_context_menu(item_path.clone(), has_clipboard, is_class)(menu, _window, _cx)
                    })
            )
    }

    fn handle_item_click(&mut self, item: &FileItem, modifiers: &Modifiers, cx: &mut Context<Self>) {
        // Single click just selects items
        if modifiers.control || modifiers.platform {
            if self.selected_items.contains(&item.path) {
                self.selected_items.remove(&item.path);
            } else {
                self.selected_items.insert(item.path.clone());
            }
        } else {
            self.selected_items.clear();
            self.selected_items.insert(item.path.clone());
        }
        
        cx.notify();
    }

    fn handle_item_double_click(&mut self, item: &FileItem, cx: &mut Context<Self>) {
        // Double click opens folders or files
        if item.is_folder {
            self.selected_folder = Some(item.path.clone());
        } else {
            cx.emit(FileSelected {
                path: item.path.clone(),
                file_type_def: item.file_type_def.clone(),
            });
        }
        cx.notify();
    }

    // ========================================================================
    // ITEM MANAGEMENT
    // ========================================================================

    fn start_new_file(&mut self, cx: &mut Context<Self>) {
        if let Some(ref folder) = self.selected_folder {
            let new_path = folder.join("untitled.txt");
            if let Err(e) = std::fs::write(&new_path, "") {
                tracing::error!("Failed to create file: {}", e);
                return;
            }
            self.renaming_item = Some(new_path);
            cx.notify();
        }
    }

    fn start_new_folder(&mut self, cx: &mut Context<Self>) {
        if let Some(ref folder) = self.selected_folder {
            let new_path = folder.join("New Folder");
            if let Err(e) = std::fs::create_dir(&new_path) {
                tracing::error!("Failed to create folder: {}", e);
                return;
            }
            self.renaming_item = Some(new_path);
            cx.notify();
        }
    }

    fn get_filtered_items(&self) -> Vec<FileItem> {
        let Some(ref folder) = self.selected_folder else {
            return Vec::new();
        };

        let Ok(entries) = std::fs::read_dir(folder) else {
            return Vec::new();
        };

        let mut items: Vec<FileItem> = entries
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                let path = entry.path();

                // Filter hidden files
                if !self.show_hidden_files {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if name.starts_with('.') {
                            return None;
                        }
                    }
                }

                FileItem::from_path(&path, &self.registered_file_types)
            })
            .filter(|item| {
                // Apply search filter
                if !self.file_filter_query.is_empty() {
                    item.name
                        .to_lowercase()
                        .contains(&self.file_filter_query.to_lowercase())
                } else {
                    true
                }
            })
            .collect();

        // Sort items
        items.sort_by(|a, b| {
            let cmp = match self.sort_by {
                SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortBy::Modified => a.modified.cmp(&b.modified),
                SortBy::Size => a.size.cmp(&b.size),
                SortBy::Type => a.display_name().cmp(b.display_name()),
            };

            match self.sort_order {
                SortOrder::Ascending => cmp,
                SortOrder::Descending => cmp.reverse(),
            }
        });

        // Folders first
        items.sort_by_key(|item| !item.is_folder);

        items
    }

    // ========================================================================
    // EVENT HANDLERS
    // ========================================================================

    fn handle_folder_select(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.selected_folder = Some(path.clone());
        self.selected_items.clear();

        // Toggle expanded state in tree
        if let Some(ref mut tree) = self.folder_tree {
            tree.toggle_expanded(&path);
        }

        cx.notify();
    }

    // ========================================================================
    // FILE OPERATIONS
    // ========================================================================

    fn commit_rename(&mut self, cx: &mut Context<Self>) {
        let Some(old_path) = self.renaming_item.take() else {
            return;
        };

        let new_name = self.rename_input_state.read(cx).text().to_string().trim().to_string();
        
        // Validate
        if new_name.is_empty() {
            cx.notify();
            return;
        }
        
        let old_name = old_path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
            
        if new_name == old_name {
            cx.notify();
            return;
        }
        
        // Check for invalid characters
        if new_name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
            tracing::error!("Invalid filename: {}", new_name);
            cx.notify();
            return;
        }
        
        // Perform rename
        match self.operations.rename_item(&old_path, &new_name) {
            Ok(new_path) => {
                // Update selections
                if self.selected_folder.as_ref() == Some(&old_path) {
                    self.selected_folder = Some(new_path.clone());
                }
                if self.selected_items.remove(&old_path) {
                    self.selected_items.insert(new_path);
                }
                
                // Refresh tree
                if let Some(ref project_path) = self.project_path {
                    self.folder_tree = FolderNode::from_path(project_path);
                }
            }
            Err(e) => {
                tracing::error!("Rename failed: {}", e);
            }
        }
        
        cx.notify();
    }

    fn start_rename(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        // Get the current name
        let current_name = path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        
        // Set renaming state
        self.renaming_item = Some(path);
        
        // Clear and set the input text
        self.rename_input_state.update(cx, |state, cx| {
            // First clear everything
            let text_len = state.text().len();
            if text_len > 0 {
                state.replace_text_in_range(Some(0..text_len), "", window, cx);
            }
            // Then insert the current name
            state.replace_text_in_range(Some(0..0), &current_name, window, cx);
        });
        
        cx.notify();
    }

    fn cancel_rename(&mut self, cx: &mut Context<Self>) {
        self.renaming_item = None;
        cx.notify();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if let Some(ref project_path) = self.project_path {
            self.folder_tree = FolderNode::from_path(project_path);
        }
        cx.notify();
    }
}

// ============================================================================
// CUSTOM EVENTS FOR INTERNAL USE
// ============================================================================

#[derive(Clone)]
enum FolderTreeAction {
    SelectFolder { path: PathBuf },
}

impl EventEmitter<FileSelected> for FileManagerDrawer {}
impl EventEmitter<PopoutFileManagerEvent> for FileManagerDrawer {}
impl EventEmitter<FolderTreeAction> for FileManagerDrawer {}

impl Render for FileManagerDrawer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .key_context("FileManagerDrawer")
            .on_action(cx.listener(|this, _action: &RefreshFileManager, _window, cx| {
                if let Some(ref path) = this.project_path {
                    this.folder_tree = FolderNode::from_path(path);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, action: &CreateAsset, _window, cx| {
                this.handle_create_asset(action, cx);
            }))
            .on_action(cx.listener(|this, action: &NewFolder, _window, cx| {
                this.handle_new_folder(action, cx);
            }))
            .on_action(cx.listener(|this, _action: &DeleteItem, _window, cx| {
                this.handle_delete_item(cx);
            }))
            .on_action(cx.listener(|this, _action: &RenameItem, window, cx| {
                this.handle_rename_item(window, cx);
            }))
            .on_action(cx.listener(|this, _action: &ui::input::Escape, _window, cx| {
                // Cancel rename on Escape - clear the input and close
                if this.renaming_item.is_some() {
                    this.renaming_item = None;
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _action: &DuplicateItem, _window, cx| {
                this.handle_duplicate_item(cx);
            }))
            .on_action(cx.listener(|this, _action: &Copy, _window, cx| {
                this.handle_copy(cx);
            }))
            .on_action(cx.listener(|this, _action: &Cut, _window, cx| {
                this.handle_cut(cx);
            }))
            .on_action(cx.listener(|this, _action: &Paste, _window, cx| {
                this.handle_paste(cx);
            }))
            .on_action(cx.listener(|this, action: &OpenInFileManager, _window, cx| {
                this.handle_open_in_file_manager(action, cx);
            }))
            .on_action(cx.listener(|this, action: &OpenTerminalHere, _window, cx| {
                this.handle_open_terminal_here(action, cx);
            }))
            .on_action(cx.listener(|this, action: &ValidateAsset, _window, cx| {
                this.handle_validate_asset(action, cx);
            }))
            .on_action(cx.listener(|this, action: &ToggleFavorite, _window, cx| {
                this.handle_toggle_favorite(action, cx);
            }))
            .on_action(cx.listener(|this, action: &ToggleGitignore, _window, cx| {
                this.handle_toggle_gitignore(action, cx);
            }))
            .on_action(cx.listener(|this, action: &ToggleHidden, _window, cx| {
                this.handle_toggle_hidden(action, cx);
            }))
            .on_action(cx.listener(|this, action: &ShowHistory, _window, cx| {
                this.handle_show_history(action, cx);
            }))
            .on_action(cx.listener(|this, action: &CheckMultiuserSync, _window, cx| {
                this.handle_check_multiuser_sync(action, cx);
            }))
            .child(self.render_content(window, cx))
    }
}
