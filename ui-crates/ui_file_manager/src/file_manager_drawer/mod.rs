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
    FsMetadataManager,
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

    // Metadata management
    fs_metadata: FsMetadataManager,

    // Drag and drop
    drag_state: DragState,
    breadcrumb_hover_timer: Option<gpui::Task<()>>,
    breadcrumb_hover_path: Option<PathBuf>,

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

// ============================================================================
// MODULAR IMPLEMENTATION
// ============================================================================

// Include implementation files directly
include!("drawer_impl/constructors.rs");
include!("drawer_impl/action_handlers.rs");
include!("drawer_impl/render.rs");
include!("drawer_impl/render_toolbar.rs");
include!("drawer_impl/render_tree.rs");
include!("drawer_impl/render_content.rs");
include!("drawer_impl/item_management.rs");
include!("drawer_impl/event_handlers.rs");
include!("drawer_impl/rename_operations.rs");
include!("drawer_impl/drag_drop_handlers.rs");

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
                // Cancel drag on Escape
                this.cancel_drag(cx);
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
            .on_action(cx.listener(|this, action: &SetColorOverride, _window, cx| {
                this.handle_set_color_override(action, cx);
            }))
            .child(self.render_content(window, cx))
    }
}
