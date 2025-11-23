use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use gpui::{*, prelude::FluentBuilder, actions};
use ui::{
    button::{Button, ButtonVariants as _},
    context_menu::ContextMenuExt,
    h_flex,
    scroll::Scrollbar,
    ActiveTheme as _, StyledExt, Sizable as _,
    IconName, Icon,
};
use serde::Deserialize;
use schemars::JsonSchema;

// Define actions for context menu
actions!(
    file_explorer,
    [
        CutFile,
        CopyFile,
        PasteFile,
        DeleteFile,
        RenameFile,
        CopyFilePath,
        CopyRelativePath,
        RevealInFileManager,
    ]
);

#[derive(Action, Clone, PartialEq, Deserialize, JsonSchema)]
#[action(namespace = file_explorer)]
struct NewFileHere {
    path: String,
}

#[derive(Action, Clone, PartialEq, Deserialize, JsonSchema)]
#[action(namespace = file_explorer)]
struct NewFolderHere {
    path: String,
}

// Clipboard operations
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClipboardOperation {
    Cut,
    Copy,
    Paste,
}

#[derive(Clone)]
pub struct FileEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_directory: bool,
    pub is_expanded: bool,
    pub depth: usize,
}

pub struct FileExplorer {
    focus_handle: FocusHandle,
    project_root: Option<PathBuf>,
    file_tree: Vec<FileEntry>,
    /// Flattened list of visible entries (after applying expansion states)
    visible_entries: Vec<usize>, // Indices into file_tree
    expanded_folders: HashMap<PathBuf, bool>,
    selected_file: Option<PathBuf>,
    last_opened_file: Option<PathBuf>,
    /// Scroll state for virtualization
    scroll_handle: ScrollHandle,
    scroll_state: ui::scroll::ScrollbarState,
    /// Item height in pixels (fixed for all items)
    item_height: Pixels,
    /// Last measured viewport bounds for accurate calculations
    last_viewport_bounds: Option<Bounds<Pixels>>,
    /// Last window size to detect resizes
    last_window_size: Option<gpui::Size<Pixels>>,
    /// Dirty flag to trigger re-clamping on next render
    needs_scroll_update: bool,
    /// Clipboard state for cut/copy/paste
    clipboard_path: Option<PathBuf>,
    clipboard_operation: Option<ClipboardOperation>,
    /// Path being renamed (for inline rename)
    renaming_path: Option<PathBuf>,
}

impl FileExplorer {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            project_root: None,
            file_tree: Vec::new(),
            visible_entries: Vec::new(),
            expanded_folders: HashMap::new(),
            selected_file: None,
            last_opened_file: None,
            scroll_handle: ScrollHandle::new(),
            scroll_state: ui::scroll::ScrollbarState::default(),
            item_height: px(28.0), // Fixed height for each item
            last_viewport_bounds: None,
            last_window_size: None,
            needs_scroll_update: false,
            clipboard_path: None,
            clipboard_operation: None,
            renaming_path: None,
        }
    }

    // Action handlers
    fn on_cut_file(&mut self, _: &CutFile, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.selected_file.clone() {
            self.cut_file(path, window, cx);
        }
    }

    fn on_copy_file(&mut self, _: &CopyFile, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.selected_file.clone() {
            self.copy_file(path, window, cx);
        }
    }

    fn on_paste_file(&mut self, _: &PasteFile, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(selected) = self.selected_file.clone() {
            let target_dir = if selected.is_dir() {
                selected
            } else {
                selected.parent().unwrap_or(&selected).to_path_buf()
            };
            self.paste_file(target_dir, window, cx);
        }
    }

    fn on_delete_file(&mut self, _: &DeleteFile, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.selected_file.clone() {
            self.delete_file(path, window, cx);
        }
    }

    fn on_rename_file(&mut self, _: &RenameFile, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = self.selected_file.clone() {
            self.start_rename(path, window, cx);
        }
    }

    fn on_copy_file_path(&mut self, _: &CopyFilePath, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = &self.selected_file {
            self.copy_path_to_clipboard(path, cx);
        }
    }

    fn on_copy_relative_path(&mut self, _: &CopyRelativePath, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(path) = &self.selected_file {
            self.copy_relative_path_to_clipboard(path, cx);
        }
    }

    fn on_reveal_in_file_manager(&mut self, _: &RevealInFileManager, _window: &mut Window, _cx: &mut Context<Self>) {
        if let Some(path) = &self.selected_file {
            self.reveal_in_file_manager(path);
        }
    }

    fn on_new_file_here(&mut self, action: &NewFileHere, window: &mut Window, cx: &mut Context<Self>) {
        let path = PathBuf::from(&action.path);
        self.create_file_in_directory(path, window, cx);
    }

    fn on_new_folder_here(&mut self, action: &NewFolderHere, window: &mut Window, cx: &mut Context<Self>) {
        let path = PathBuf::from(&action.path);
        self.create_folder_in_directory(path, window, cx);
    }

    pub fn open_project(&mut self, path: PathBuf, _window: &mut Window, cx: &mut Context<Self>) {
        if path.is_dir() {
            self.project_root = Some(path.clone());
            self.refresh_file_tree(cx);
            cx.notify();
        }
    }

    fn refresh_file_tree(&mut self, _cx: &mut Context<Self>) {
        self.file_tree.clear();
        self.visible_entries.clear();
        
        if let Some(root) = self.project_root.clone() {
            self.scan_directory(&root, 0);
            self.rebuild_visible_entries();
        }
        
        // Mark that scroll needs updating
        self.needs_scroll_update = true;
    }
    
    /// Rebuild the flat list of visible entries based on expansion states
    fn rebuild_visible_entries(&mut self) {
        self.visible_entries.clear();
        
        for (idx, entry) in self.file_tree.iter().enumerate() {
            // Check if this entry should be visible based on parent expansion
            if self.is_entry_visible(idx) {
                self.visible_entries.push(idx);
            }
        }
        
        // Mark that scroll needs updating after tree structure change
        self.needs_scroll_update = true;
    }
    
    /// Check if an entry is visible (all parents are expanded)
    fn is_entry_visible(&self, entry_idx: usize) -> bool {
        let entry = &self.file_tree[entry_idx];
        
        // Root level is always visible
        if entry.depth == 0 {
            return true;
        }
        
        // Check if parent is expanded
        if let Some(parent_path) = entry.path.parent() {
            // Find parent in file tree
            for (idx, potential_parent) in self.file_tree.iter().enumerate() {
                if idx >= entry_idx {
                    break; // Parent must come before child
                }
                
                if potential_parent.path == parent_path {
                    return potential_parent.is_expanded && self.is_entry_visible(idx);
                }
            }
        }
        
        false
    }

    fn scan_directory(&mut self, dir: &Path, depth: usize) {
        if depth > 10 { return; } // Prevent infinite recursion

        if let Ok(entries) = fs::read_dir(dir) {
            let mut dirs = Vec::new();
            let mut files = Vec::new();

            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();

                // Skip hidden files and common ignore patterns
                if name.starts_with('.') || name == "target" || name == "node_modules" {
                    continue;
                }

                let file_entry = FileEntry {
                    name,
                    path: path.clone(),
                    is_directory: path.is_dir(),
                    is_expanded: self.expanded_folders.get(&path).copied().unwrap_or(false),
                    depth,
                };

                if path.is_dir() {
                    dirs.push(file_entry);
                } else {
                    files.push(file_entry);
                }
            }

            // Sort directories and files alphabetically
            dirs.sort_by(|a, b| a.name.cmp(&b.name));
            files.sort_by(|a, b| a.name.cmp(&b.name));

            // Add directories first, then files
            for dir_entry in dirs {
                let is_expanded = dir_entry.is_expanded;
                let path = dir_entry.path.clone();
                self.file_tree.push(dir_entry);

                if is_expanded {
                    self.scan_directory(&path, depth + 1);
                }
            }

            for file_entry in files {
                self.file_tree.push(file_entry);
            }
        }
    }

    fn toggle_folder(&mut self, path: &Path, _window: &mut Window, cx: &mut Context<Self>) {
        let is_expanded = self.expanded_folders.get(path).copied().unwrap_or(false);
        self.expanded_folders.insert(path.to_path_buf(), !is_expanded);
        
        // Update the file tree entry
        for entry in &mut self.file_tree {
            if entry.path == path {
                entry.is_expanded = !is_expanded;
                break;
            }
        }
        
        // Rebuild the file tree to reflect expansion changes
        self.refresh_file_tree(cx);
        
        cx.notify();
    }

    fn select_file(&mut self, path: PathBuf, _window: &mut Window, cx: &mut Context<Self>) {
        self.selected_file = Some(path);
        cx.notify();
    }

    fn open_file_in_editor(&mut self, path: PathBuf, _window: &mut Window, cx: &mut Context<Self>) {
        println!("Opening file in editor: {:?}", path);
        self.selected_file = Some(path.clone());
        self.last_opened_file = Some(path);
        cx.notify();
    }

    pub fn get_last_opened_file(&mut self) -> Option<PathBuf> {
        self.last_opened_file.take()
    }

    fn create_new_file(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(root) = &self.project_root {
            let new_path = root.join("new_file.rs");

            // Create the file
            if let Ok(_) = fs::write(&new_path, "") {
                self.refresh_file_tree(cx);
                self.selected_file = Some(new_path);
                cx.notify();
            }
        }
    }

    fn create_new_folder(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(root) = &self.project_root {
            let new_path = root.join("new_folder");

            // Create the directory
            if let Ok(_) = fs::create_dir(&new_path) {
                self.refresh_file_tree(cx);
                cx.notify();
            }
        }
    }

    fn get_file_icon(&self, entry: &FileEntry) -> IconName {
        if entry.is_directory {
            if entry.is_expanded {
                IconName::FolderOpen
            } else {
                IconName::Folder
            }
        } else {
            match entry.path.extension().and_then(|ext| ext.to_str()) {
                Some("rs") => IconName::Code,
                Some("js") | Some("ts") => IconName::Code,
                Some("py") => IconName::Code,
                Some("toml") | Some("json") => IconName::Settings,
                Some("md") => IconName::Notes,
                Some("txt") => IconName::Page,
                Some("html") => IconName::HTML5,
                Some("css") => IconName::CSS3,
                Some("png") | Some("jpg") | Some("jpeg") | Some("gif") => IconName::MediaImage,
                _ => IconName::Page,
            }
        }
    }

    // Clipboard operations
    fn cut_file(&mut self, path: PathBuf, _window: &mut Window, cx: &mut Context<Self>) {
        self.clipboard_path = Some(path);
        self.clipboard_operation = Some(ClipboardOperation::Cut);
        println!("📋 Cut: {:?}", self.clipboard_path);
        cx.notify();
    }

    fn copy_file(&mut self, path: PathBuf, _window: &mut Window, cx: &mut Context<Self>) {
        self.clipboard_path = Some(path);
        self.clipboard_operation = Some(ClipboardOperation::Copy);
        println!("📋 Copy: {:?}", self.clipboard_path);
        cx.notify();
    }

    fn paste_file(&mut self, target_dir: PathBuf, _window: &mut Window, cx: &mut Context<Self>) {
        if let (Some(source_path), Some(operation)) = (&self.clipboard_path, &self.clipboard_operation) {
            let file_name = source_path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
            let dest_path = target_dir.join(file_name);

            match operation {
                ClipboardOperation::Cut => {
                    if let Err(e) = fs::rename(source_path, &dest_path) {
                        eprintln!("Failed to move file: {}", e);
                    } else {
                        println!("✓ Moved: {:?} -> {:?}", source_path, dest_path);
                        self.clipboard_path = None;
                        self.clipboard_operation = None;
                        self.refresh_file_tree(cx);
                    }
                }
                ClipboardOperation::Copy => {
                    if source_path.is_dir() {
                        // Recursive directory copy
                        if let Err(e) = self.copy_dir_recursive(source_path, &dest_path) {
                            eprintln!("Failed to copy directory: {}", e);
                        } else {
                            println!("✓ Copied directory: {:?} -> {:?}", source_path, dest_path);
                            self.refresh_file_tree(cx);
                        }
                    } else {
                        if let Err(e) = fs::copy(source_path, &dest_path) {
                            eprintln!("Failed to copy file: {}", e);
                        } else {
                            println!("✓ Copied: {:?} -> {:?}", source_path, dest_path);
                            self.refresh_file_tree(cx);
                        }
                    }
                }
                ClipboardOperation::Paste => {}
            }
        }
        cx.notify();
    }

    fn copy_dir_recursive(&self, src: &Path, dst: &Path) -> std::io::Result<()> {
        fs::create_dir_all(dst)?;
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let path = entry.path();
            let file_name = entry.file_name();
            let dest_path = dst.join(file_name);

            if path.is_dir() {
                self.copy_dir_recursive(&path, &dest_path)?;
            } else {
                fs::copy(&path, &dest_path)?;
            }
        }
        Ok(())
    }

    fn delete_file(&mut self, path: PathBuf, _window: &mut Window, cx: &mut Context<Self>) {
        let result = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };

        match result {
            Ok(_) => {
                println!("✓ Deleted: {:?}", path);
                self.refresh_file_tree(cx);
            }
            Err(e) => {
                eprintln!("Failed to delete: {}", e);
            }
        }
        cx.notify();
    }

    fn start_rename(&mut self, path: PathBuf, _window: &mut Window, cx: &mut Context<Self>) {
        self.renaming_path = Some(path);
        cx.notify();
    }

    fn copy_path_to_clipboard(&self, path: &Path, cx: &mut App) {
        let path_str = path.to_string_lossy().to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(path_str.clone()));
        println!("📋 Copied path to clipboard: {}", path_str);
    }

    fn copy_relative_path_to_clipboard(&self, path: &Path, cx: &mut App) {
        if let Some(root) = &self.project_root {
            if let Ok(relative) = path.strip_prefix(root) {
                let path_str = relative.to_string_lossy().to_string();
                cx.write_to_clipboard(ClipboardItem::new_string(path_str.clone()));
                println!("📋 Copied relative path to clipboard: {}", path_str);
                return;
            }
        }
        // Fallback to absolute path
        self.copy_path_to_clipboard(path, cx);
    }

    fn reveal_in_file_manager(&self, path: &Path) {
        #[cfg(target_os = "windows")]
        {
            let _ = std::process::Command::new("explorer")
                .arg("/select,")
                .arg(path)
                .spawn();
        }
        #[cfg(target_os = "macos")]
        {
            let _ = std::process::Command::new("open")
                .arg("-R")
                .arg(path)
                .spawn();
        }
        #[cfg(target_os = "linux")]
        {
            if let Some(parent) = path.parent() {
                let _ = std::process::Command::new("xdg-open")
                    .arg(parent)
                    .spawn();
            }
        }
        println!("📂 Revealed in file manager: {:?}", path);
    }

    fn create_file_in_directory(&mut self, dir_path: PathBuf, _window: &mut Window, cx: &mut Context<Self>) {
        let new_path = dir_path.join("new_file.txt");
        
        // Create the file
        if let Ok(_) = fs::write(&new_path, "") {
            self.refresh_file_tree(cx);
            self.selected_file = Some(new_path.clone());
            self.start_rename(new_path, _window, cx);
        }
    }

    fn create_folder_in_directory(&mut self, dir_path: PathBuf, _window: &mut Window, cx: &mut Context<Self>) {
        let new_path = dir_path.join("new_folder");
        
        // Create the directory
        if let Ok(_) = fs::create_dir(&new_path) {
            self.refresh_file_tree(cx);
            self.selected_file = Some(new_path.clone());
            self.start_rename(new_path, _window, cx);
        }
    }
    
    /// Calculate which entries are visible in the viewport (virtualization)
    fn calculate_visible_range(&self, scroll_offset: Pixels, viewport_height: Pixels) -> (usize, usize) {
        // Ensure we have a minimum viewport height
        let safe_viewport_height = viewport_height.max(px(100.0));
        
        // Convert to float for division
        let scroll_f = -scroll_offset;
        let item_height_f = self.item_height;
        
        let start_index = ((scroll_f / item_height_f).floor().max(0.0)) as usize;
        let visible_count = ((safe_viewport_height / item_height_f).ceil() as usize) + 4; // +4 for buffer
        let end_index = (start_index + visible_count).min(self.visible_entries.len());
        
        (start_index, end_index)
    }
    
    /// Get viewport height from last measured bounds, or use fallback
    fn get_viewport_height(&self) -> Pixels {
        let height = self.last_viewport_bounds
            .map(|bounds| bounds.size.height)
            .unwrap_or(px(600.0)); // Fallback for first render
        
        // Debug: print when using fallback
        if self.last_viewport_bounds.is_none() {
            println!("⚠️  Using fallback viewport height: 600px");
        }
        
        height
    }
    
    /// Get viewport width from last measured bounds, or use fallback
    fn get_viewport_width(&self) -> Pixels {
        self.last_viewport_bounds
            .map(|bounds| bounds.size.width)
            .unwrap_or(px(250.0)) // Fallback for first render
    }

    fn render_file_tree_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Check for window size changes
        let window_size = window.viewport_size();
        let size_changed = self.last_window_size
            .map(|last_size| last_size != window_size)
            .unwrap_or(true);
        
        if size_changed {
            self.last_window_size = Some(window_size);
            // Estimate viewport bounds based on window size
            // Account for header (48px) and footer (36px) = 84px total chrome
            let estimated_height = (window_size.height - px(84.0)).max(px(200.0));
            let estimated_width = px(250.0); // Typical sidebar width
            
            self.last_viewport_bounds = Some(Bounds {
                origin: gpui::point(px(0.0), px(0.0)),
                size: gpui::size(estimated_width, estimated_height),
            });
            
            // Mark scroll for update with new bounds
            self.needs_scroll_update = true;
        }
        
        // Use last known viewport bounds or fallback
        let viewport_height = self.get_viewport_height();
        let viewport_width = self.get_viewport_width();
        
        // Create bounds from stored values
        let bounds = self.last_viewport_bounds.unwrap_or(Bounds {
            origin: gpui::point(px(0.0), px(0.0)),
            size: gpui::size(viewport_width, viewport_height),
        });
        
        // Apply any pending scroll updates
        if self.needs_scroll_update {
            let current_offset = self.scroll_handle.offset();
            self.set_scroll_offset_clamped(current_offset, bounds.size.height);
            self.needs_scroll_update = false;
        }
        
        let scroll_offset = self.scroll_handle.offset();
        let (start_idx, end_idx) = self.calculate_visible_range(scroll_offset.y, bounds.size.height);
        
        // Total height of all items
        let total_height = self.item_height * self.visible_entries.len() as f32;
        
        // Offset for virtualization
        let offset_y = self.item_height * start_idx as f32;
        
        div()
            .id("file_tree_viewport")
            .relative()
            .size_full()
            .overflow_hidden()
            .font_family("monospace")
            .font(gpui::Font {
                family: "Jetbrains Mono".to_string().into(),
                weight: gpui::FontWeight::NORMAL,
                style: gpui::FontStyle::Normal,
                features: gpui::FontFeatures::default(),
                fallbacks: Some(gpui::FontFallbacks::from_fonts(vec!["monospace".to_string()])),
            })
            .child(
                // Visible items container with absolute positioning for virtualization
                div()
                    .absolute()
                    .top(offset_y + scroll_offset.y)
                    .left(scroll_offset.x)
                    .w_full()
                    .flex()
                    .flex_col()
                    .children(
                        (start_idx..end_idx)
                            .filter_map(|visible_idx| {
                                self.visible_entries.get(visible_idx)
                                    .and_then(|&tree_idx| self.file_tree.get(tree_idx))
                                    .map(|entry| self.render_file_item(entry, cx))
                            })
                    )
            )
            .child(
                // Scrollbar overlay
                div()
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(px(12.0))
                    .child(
                        Scrollbar::vertical(&self.scroll_state, &self.scroll_handle)
                            .scroll_size(gpui::Size {
                                width: bounds.size.width,
                                height: total_height,
                            })
                    )
            )
    }
    
    /// Update viewport bounds when window is resized or layout changes
    pub fn update_viewport_bounds(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        let bounds_changed = self.last_viewport_bounds
            .map(|old_bounds| old_bounds.size != bounds.size)
            .unwrap_or(true);
        
        if bounds_changed {
            self.last_viewport_bounds = Some(bounds);
            // Re-clamp scroll with new viewport size
            let current_offset = self.scroll_handle.offset();
            self.set_scroll_offset_clamped(current_offset, bounds.size.height);
            cx.notify();
        }
    }

    fn render_file_item(&self, entry: &FileEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let is_selected = self.selected_file.as_ref() == Some(&entry.path);
        let path = entry.path.clone();
        let is_directory = entry.is_directory;
        let icon = self.get_file_icon(entry);
        let indent = px(entry.depth as f32 * 16.0); // 16px per depth level
        let has_clipboard = self.clipboard_path.is_some();
        // Create a valid ID by hashing the path
        let item_id = SharedString::from(format!("file-{:x}", {
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};
            let mut hasher = DefaultHasher::new();
            entry.path.hash(&mut hasher);
            hasher.finish()
        }));

        div()
            .id(item_id)
            .flex()
            .items_center()
            .gap_2()
            .h(self.item_height)
            .pl(indent + px(12.0))
            .pr_3()
            .rounded_md()
            .when(is_selected, |style| style.bg(cx.theme().accent))
            .when(!is_selected, |style| {
                style.hover(|style| style.bg(cx.theme().accent.opacity(0.1)))
            })
            .cursor_pointer()
            .child(Icon::new(icon).size_4())
            .child(
                div()
                    .text_sm()
                    .when(is_selected, |style| style.text_color(cx.theme().accent_foreground))
                    .when(!is_selected, |style| style.text_color(cx.theme().foreground))
                    .child(entry.name.clone())
            )
            .on_mouse_down(gpui::MouseButton::Left, {
                let path = path.clone();
                cx.listener(move |this, _, window, cx| {
                    if is_directory {
                        this.toggle_folder(&path, window, cx);
                    } else {
                        this.select_file(path.clone(), window, cx);
                        this.open_file_in_editor(path.clone(), window, cx);
                    }
                })
            })
            // Select the item on right-click to ensure context menu actions work on it
            .on_mouse_down(gpui::MouseButton::Right, {
                let path = path.clone();
                cx.listener(move |this, _, window, cx| {
                    this.select_file(path.clone(), window, cx);
                })
            })
            .context_menu({
                let path_for_menu = path.clone();
                let path_str = path_for_menu.to_string_lossy().to_string();
                move |menu, _window, _cx| {
                    let mut menu = menu;

                    // File/Folder specific actions
                    if is_directory {
                        menu = menu
                            .menu("New File Here", Box::new(NewFileHere { path: path_str.clone() }))
                            .menu("New Folder Here", Box::new(NewFolderHere { path: path_str.clone() }))
                            .separator();
                    }

                    // Common actions
                    menu = menu
                        .menu("Cut", Box::new(CutFile))
                        .menu("Copy", Box::new(CopyFile));

                    // Paste (only if we have something in clipboard)
                    if has_clipboard {
                        menu = menu.menu("Paste", Box::new(PasteFile));
                    }

                    menu = menu
                        .separator()
                        .menu("Rename", Box::new(RenameFile))
                        .menu("Delete", Box::new(DeleteFile))
                        .separator()
                        .menu("Copy Path", Box::new(CopyFilePath))
                        .menu("Copy Relative Path", Box::new(CopyRelativePath))
                        .separator()
                        .menu("Reveal in File Manager", Box::new(RevealInFileManager));

                    menu
                }
            })
    }
    
    fn on_scroll_wheel(&mut self, event: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(20.0));
        let current_offset = self.scroll_handle.offset();
        let new_offset = current_offset + delta;
        
        let viewport_height = self.get_viewport_height();
        self.set_scroll_offset_clamped(new_offset, viewport_height);
        cx.notify();
    }
    
    /// Set scroll offset with bounds checking to prevent out-of-bounds scrolling
    /// Uses actual measured viewport height for accurate bounds
    fn set_scroll_offset_clamped(&mut self, offset: gpui::Point<Pixels>, viewport_height: Pixels) {
        // Calculate bounds for scrolling
        let total_height = self.item_height * self.visible_entries.len() as f32;
        
        // Clamp scroll offset to valid range
        // Y: Can scroll from 0 (top) to -(total_height - viewport_height) (bottom)
        let max_scroll_y = px(0.0);
        let min_scroll_y = -(total_height - viewport_height).max(px(0.0));
        
        // X: No horizontal scrolling needed, keep at 0
        let clamped_offset = gpui::point(
            px(0.0), // No horizontal scroll
            offset.y.max(min_scroll_y).min(max_scroll_y)
        );
        
        self.scroll_handle.set_offset(clamped_offset);
    }
    
    /// Scroll to ensure a specific entry is visible
    pub fn scroll_to_entry(&mut self, entry_path: &Path, cx: &mut Context<Self>) {
        // Find the entry index in visible_entries
        let entry_index = self.visible_entries.iter()
            .position(|&idx| {
                self.file_tree.get(idx)
                    .map(|e| e.path == entry_path)
                    .unwrap_or(false)
            });
        
        if let Some(visible_idx) = entry_index {
            let viewport_height = self.get_viewport_height();
            let item_position = self.item_height * visible_idx as f32;
            let current_offset = self.scroll_handle.offset();
            
            // Check if item is already visible
            let scroll_top = -current_offset.y;
            let scroll_bottom = scroll_top + viewport_height;
            let item_bottom = item_position + self.item_height;
            
            if item_position < scroll_top {
                // Item is above viewport, scroll to show it at top
                self.set_scroll_offset_clamped(gpui::point(px(0.0), -item_position), viewport_height);
                cx.notify();
            } else if item_bottom > scroll_bottom {
                // Item is below viewport, scroll to show it at bottom
                let target_scroll = -(item_bottom - viewport_height);
                self.set_scroll_offset_clamped(gpui::point(px(0.0), target_scroll), viewport_height);
                cx.notify();
            }
        }
    }
}

impl Focusable for FileExplorer {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for FileExplorer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let file_tree_empty = self.file_tree.is_empty();
        let focus_handle = self.focus_handle.clone();
        
        div()
            .key_context("FileExplorer")
            .track_focus(&focus_handle)
            .on_action(cx.listener(Self::on_cut_file))
            .on_action(cx.listener(Self::on_copy_file))
            .on_action(cx.listener(Self::on_paste_file))
            .on_action(cx.listener(Self::on_delete_file))
            .on_action(cx.listener(Self::on_rename_file))
            .on_action(cx.listener(Self::on_copy_file_path))
            .on_action(cx.listener(Self::on_copy_relative_path))
            .on_action(cx.listener(Self::on_reveal_in_file_manager))
            .on_action(cx.listener(Self::on_new_file_here))
            .on_action(cx.listener(Self::on_new_folder_here))
            .size_full()
            .flex()
            .flex_col()
            .child(
                // Header
                div()
                    .w_full()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(cx.theme().foreground)
                                    .child("Explorer")
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Button::new("new_file")
                                            .icon(IconName::Plus)
                                            .tooltip("New File")
                                            .ghost()
                                            .xsmall()
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.create_new_file(window, cx);
                                            }))
                                    )
                                    .child(
                                        Button::new("new_folder")
                                            .icon(IconName::Folder)
                                            .tooltip("New Folder")
                                            .ghost()
                                            .xsmall()
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                this.create_new_folder(window, cx);
                                            }))
                                    )
                                    .child(
                                        Button::new("refresh")
                                            .icon(IconName::Asterisk)
                                            .tooltip("Refresh")
                                            .ghost()
                                            .xsmall()
                                            .on_click(cx.listener(|this, _, _window, cx| {
                                                this.refresh_file_tree(cx);
                                            }))
                                    )
                                    .child(
                                        Button::new("open_folder")
                                            .icon(IconName::FolderOpen)
                                            .tooltip("Open Folder")
                                            .ghost()
                                            .xsmall()
                                            .on_click(cx.listener(|this, _, window, cx| {
                                                // Open current working directory as fallback
                                                if let Ok(cwd) = std::env::current_dir() {
                                                    this.open_project(cwd, window, cx);
                                                }
                                            }))
                                    )
                            )
                    )
            )
            .child(
                // Scrollable content area with virtualization
                div()
                    .id("file-explorer-content")
                    .flex_1()
                    .overflow_hidden()
                    .on_scroll_wheel(cx.listener(Self::on_scroll_wheel))
                    .when(file_tree_empty, |content| {
                        content.child(
                            div()
                                .p_4()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_2()
                                        .px_3()
                                        .py_2()
                                        .rounded_md()
                                        .hover(|style| style.bg(cx.theme().accent.opacity(0.1)))
                                        .cursor_pointer()
                                        .child(Icon::new(IconName::FolderOpen).size_4().text_color(cx.theme().muted_foreground))
                                        .child(
                                            div()
                                                .text_sm()
                                                .text_color(cx.theme().muted_foreground)
                                                .child("No folder opened")
                                        )
                                        .on_mouse_down(gpui::MouseButton::Left, cx.listener(|this, _, window, cx| {
                                            if let Ok(cwd) = std::env::current_dir() {
                                                this.open_project(cwd, window, cx);
                                            }
                                        }))
                                )
                        )
                    })
                    .when(!file_tree_empty, |content| {
                        // Render virtualized content
                        content.child(self.render_file_tree_content(_window, cx))
                    })
            )
            .when_some(self.project_root.clone(), |container, root| {
                container.child(
                    // Footer
                    div()
                        .w_full()
                        .px_4()
                        .py_2()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    root.file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy()
                                        .to_string()
                                )
                        )
                )
            })
    }
}