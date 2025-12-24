use gpui::prelude::*;
use gpui::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use ui::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
    input::{InputState, TextInput},
    resizable::{h_resizable, resizable_panel, ResizableState},
    menu::context_menu::ContextMenuExt,
    ActiveTheme as _, Icon, IconName, StyledExt,
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
    project_path: Option<PathBuf>,
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

        // Subscribe to rename input
        cx.subscribe(
            &rename_input_state,
            |drawer, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::PressEnter { .. } = event {
                    if drawer.renaming_item.is_some() {
                        drawer.commit_rename(cx);
                    }
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
        }
    }

    pub fn new_in_window(project_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self::new(project_path, window, cx)
    }

    pub fn set_project_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        println!("[FILE_MANAGER] set_project_path called with: {:?}", path);
        println!("[FILE_MANAGER] Path exists: {}", path.exists());
        println!("[FILE_MANAGER] Path is_dir: {}", path.is_dir());
        
        self.project_path = Some(path.clone());
        self.folder_tree = FolderNode::from_path(&path);
        self.selected_folder = Some(path.clone());
        
        println!("[FILE_MANAGER] folder_tree is_some: {}", self.folder_tree.is_some());
        println!("[FILE_MANAGER] selected_folder: {:?}", self.selected_folder);
        
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
            .bg(cx.theme().background)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                // Folder tree header
                h_flex()
                    .h(px(40.))
                    .px_4()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(cx.theme().foreground)
                            .child("Folders")
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("collapse-all")
                            .icon(IconName::ChevronsUpDown)
                            .ghost()
                            .tooltip("Collapse All")
                            .on_click(cx.listener(|drawer, _event, _window, cx| {
                                if let Some(ref mut tree) = drawer.folder_tree {
                                    tree.collapse_all();
                                    cx.notify();
                                }
                            }))
                    )
            )
            .child(
                // Folder tree content - SCROLLABLE
                div()
                    .id("folder-tree-scroll")
                    .flex_1()
                    .p_2()
                    .overflow_y_scroll()
                    .when_some(self.folder_tree.clone(), |this, tree| {
                        this.child(self.render_folder_node(&tree, 0, window, cx))
                    })
                    .when(self.folder_tree.is_none(), |this| {
                        this.child(
                            div()
                                .text_sm()
                                .text_color(cx.theme().muted_foreground)
                                .child("No project folder selected")
                        )
                    })
            )
    }

    fn render_folder_node(&mut self, node: &FolderNode, depth: usize, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_selected = self.selected_folder.as_ref() == Some(&node.path);
        let path = node.path.clone();
        let has_children = !node.children.is_empty();
        let expanded = node.expanded;

        v_flex()
            .child(
                h_flex()
                    .pl(px((depth * 16) as f32))
                    .pr_2()
                    .py_1()
                    .gap_1()
                    .items_center()
                    .rounded(px(4.0))
                    .when(is_selected, |this| {
                        this.bg(cx.theme().accent.opacity(0.1))
                    })
                    .hover(|this| {
                        this.bg(cx.theme().muted.opacity(0.2))
                    })
                    .cursor_pointer()
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |drawer, _event, _window, cx| {
                        drawer.handle_folder_select(path.clone(), cx);
                    }))
                    .child(
                        if has_children {
                            div()
                                .w(px(16.0))
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                                        .size_3()
                                        .text_color(cx.theme().muted_foreground)
                                )
                                .into_any_element()
                        } else {
                            div()
                                .w(px(16.0))
                                .into_any_element()
                        }
                    )
                    .child(
                        Icon::new(if expanded { IconName::FolderOpen } else { IconName::Folder })
                            .size_4()
                            .text_color(cx.theme().accent)
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().foreground)
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
                    .context_menu(move |menu, _window, _cx| {
                        // Show folder context menu for blank area
                        if let Some(path) = selected_folder.clone() {
                            context_menus::folder_context_menu(path, has_clipboard)(menu, _window, _cx)
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
            .gap_px()
            .children(items.iter().map(|item| {
                self.render_list_item(item, window, cx)
            }))
    }

    fn render_list_item(&mut self, item: &FileItem, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_selected = self.selected_items.contains(&item.path);
        let icon = get_icon_for_file_type(&item.file_type);
        let icon_color = get_icon_color_for_file_type(&item.file_type, cx.theme());
        let item_clone = item.clone();
        let item_clone2 = item.clone();
        let item_path = item.path.clone();
        let has_clipboard = self.clipboard.is_some();
        let is_class = item.file_type == FileType::Class;
        let is_folder = item.is_folder;

        h_flex()
            .id(SharedString::from(format!("list-item-{}", item.name)))
            .w_full()
            .h(px(32.))
            .px_3()
            .py_1()
            .gap_2()
            .items_center()
            .rounded(px(4.))
            .when(is_selected, |this| {
                this.bg(cx.theme().accent.opacity(0.15))
            })
            .hover(|this| {
                this.bg(cx.theme().muted.opacity(0.2))
            })
            .cursor_pointer()
            .child(
                Icon::new(icon)
                    .size_4()
                    .text_color(icon_color)
            )
            .child(
                div()
                    .flex_1()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(item.name.clone())
            )
            .when(!item.is_folder, |this| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format_file_size(item.size))
                )
            })
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |drawer, event: &MouseDownEvent, _window: &mut Window, cx| {
                drawer.handle_item_click(&item_clone, &event.modifiers, cx);
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
                if is_folder {
                    context_menus::folder_context_menu(item_path.clone(), has_clipboard)(menu, _window, _cx)
                } else {
                    context_menus::item_context_menu(item_path.clone(), has_clipboard, is_class)(menu, _window, _cx)
                }
            })
    }

    fn render_combined_toolbar(&mut self, items: &[FileItem], window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(format!("{} items", items.len()))
            )
            // Lock button
            .child(
                Button::new("lock")
                    .icon(IconName::Lock)
                    .ghost()
                    .tooltip("Lock Files")
                    .on_click(cx.listener(|_drawer, _event, _window, _cx| {
                        // TODO: Implement lock functionality
                    }))
            )
            // Layout buttons
            .child(
                Button::new("toggle-view")
                    .icon(IconName::LayoutDashboard)
                    .ghost()
                    .tooltip("Grid View")
                    .on_click(cx.listener(|drawer, _event, _window, cx| {
                        drawer.view_mode = ViewMode::Grid;
                        cx.notify();
                    }))
            )
            .child(
                Button::new("toggle-list")
                    .icon(IconName::List)
                    .ghost()
                    .tooltip("List View")
                    .on_click(cx.listener(|drawer, _event, _window, cx| {
                        drawer.view_mode = ViewMode::List;
                        cx.notify();
                    }))
            )
            .child(
                Button::new("split-view")
                    .icon(IconName::HorizontalSplit)
                    .ghost()
                    .tooltip("Split View")
                    .on_click(cx.listener(|_drawer, _event, _window, _cx| {
                        // TODO: Implement split view
                    }))
            )
            // File operations
            .child(
                Button::new("new-file")
                    .icon(IconName::PagePlus)
                    .ghost()
                    .tooltip("New File")
                    .on_click(cx.listener(|drawer, _event, _window, cx| {
                        drawer.start_new_file(cx);
                    }))
            )
            .child(
                Button::new("new-folder")
                    .icon(IconName::FolderPlus)
                    .ghost()
                    .tooltip("New Folder")
                    .on_click(cx.listener(|drawer, _event, _window, cx| {
                        drawer.start_new_folder(cx);
                    }))
            )
            // Refresh
            .child(
                Button::new("refresh")
                    .icon(IconName::Refresh)
                    .ghost()
                    .tooltip("Refresh")
                    .on_click(cx.listener(|drawer, _event, _window, cx| {
                        if let Some(ref path) = drawer.project_path {
                            drawer.folder_tree = FolderNode::from_path(path);
                        }
                        cx.notify();
                    }))
            )
            // Filter/search
            .child(
                Button::new("filter")
                    .icon(IconName::Filter)
                    .ghost()
                    .tooltip("Filter")
                    .on_click(cx.listener(|_drawer, _event, _window, _cx| {
                        // TODO: Implement filter
                    }))
            )
            // Show/hide
            .child(
                Button::new("toggle-hidden")
                    .icon(if self.show_hidden_files { IconName::EyeOff } else { IconName::Eye })
                    .ghost()
                    .tooltip(if self.show_hidden_files { "Hide Hidden Files" } else { "Show Hidden Files" })
                    .on_click(cx.listener(|drawer, _event, _window, cx| {
                        drawer.show_hidden_files = !drawer.show_hidden_files;
                        cx.notify();
                    }))
            )
            // Open external
            .child(
                Button::new("external")
                    .icon(IconName::ExternalLink)
                    .ghost()
                    .tooltip("Open in File Explorer")
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
            // More options
            .child(
                Button::new("more")
                    .icon(IconName::Ellipsis)
                    .ghost()
                    .tooltip("More Options")
                    .on_click(cx.listener(|_drawer, _event, _window, _cx| {
                        // TODO: Implement more options menu
                    }))
            )
    }

    fn render_clickable_breadcrumb(&mut self, items: &[FileItem], window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
        let icon = get_icon_for_file_type(&item.file_type);
        let icon_color = get_icon_color_for_file_type(&item.file_type, cx.theme());
        let item_clone = item.clone();
        let item_clone2 = item.clone();
        let item_path = item.path.clone();
        let has_clipboard = self.clipboard.is_some();
        let is_class = item.file_type == FileType::Class;
        let is_folder = item.is_folder;

        div()
            .id(SharedString::from(format!("grid-item-{}", item.name)))
            .w(px(100.0))
            .h(px(110.0))
            .rounded(px(8.0))
            .border_1()
            .when(is_selected, |this| {
                this.border_color(cx.theme().accent.opacity(0.5))
                    .bg(cx.theme().accent.opacity(0.1))
            })
            .when(!is_selected, |this| {
                this.border_color(cx.theme().border.opacity(0.3))
                    .bg(cx.theme().background)
            })
            .cursor_pointer()
            .hover(|style| {
                style
                    .bg(cx.theme().muted.opacity(0.2))
                    .border_color(cx.theme().accent.opacity(0.5))
                    .shadow_md()
            })
            .child(
                v_flex()
                    .w_full()
                    .h_full()
                    .p_3()
                    .gap_2()
                    .items_center()
                    .justify_center()
                    .child(
                        div()
                            .size(px(48.0))
                            .rounded(px(8.0))
                            .bg(icon_color.opacity(0.15))
                            .border_1()
                            .border_color(icon_color.opacity(0.3))
                            .flex()
                            .items_center()
                            .justify_center()
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
                                .font_medium()
                                .text_color(cx.theme().foreground)
                                .overflow_hidden()
                                .text_ellipsis()
                                .child(item.name.clone())
                                .into_any_element()
                        }
                    )
            )
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |drawer, event: &MouseDownEvent, _window: &mut Window, cx| {
                drawer.handle_item_click(&item_clone, &event.modifiers, cx);
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
                if is_folder {
                    context_menus::folder_context_menu(item_path.clone(), has_clipboard)(menu, _window, _cx)
                } else {
                    context_menus::item_context_menu(item_path.clone(), has_clipboard, is_class)(menu, _window, _cx)
                }
            })
    }

    fn handle_item_click(&mut self, item: &FileItem, modifiers: &Modifiers, cx: &mut Context<Self>) {
        if modifiers.control || modifiers.platform {
            if self.selected_items.contains(&item.path) {
                self.selected_items.remove(&item.path);
            } else {
                self.selected_items.insert(item.path.clone());
            }
        } else {
            self.selected_items.clear();
            self.selected_items.insert(item.path.clone());
            
            if item.is_folder {
                self.selected_folder = Some(item.path.clone());
            } else {
                cx.emit(FileSelected {
                    path: item.path.clone(),
                    file_type: item.file_type.clone(),
                });
            }
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
                eprintln!("Failed to create file: {}", e);
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
                eprintln!("Failed to create folder: {}", e);
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

                FileItem::from_path(&path)
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
                SortBy::Type => a.file_type.display_name().cmp(b.file_type.display_name()),
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
        if let Some(ref old_path) = self.renaming_item.clone() {
            let new_name = self.rename_input_state.read(cx).text().to_string().trim().to_string();
            if !new_name.is_empty() {
                if let Ok(new_path) = self.operations.rename_item(old_path, &new_name) {
                    // Update state
                    if self.selected_folder.as_ref() == Some(old_path) {
                        self.selected_folder = Some(new_path.clone());
                    }
                    if self.selected_items.remove(old_path) {
                        self.selected_items.insert(new_path);
                    }
                    // Refresh tree
                    if let Some(ref project_path) = self.project_path {
                        self.folder_tree = FolderNode::from_path(project_path);
                    }
                }
            }
            self.renaming_item = None;
            cx.notify();
        }
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
            .child(self.render_content(window, cx))
    }
}
