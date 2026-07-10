use gpui::prelude::*;
use gpui::*;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use ui::input::InputState;
use ui::resizable::ResizableState;
use ui::VirtualListScrollHandle;

use crate::utils::{
    actions::*, fs_metadata::FsMetadataManager, operations::FileOperations, tree::FolderNode,
    types::*,
};

mod content;
mod toolbar;
mod tree;

pub struct FileManagerDrawer {
    pub project_path: Option<PathBuf>,
    pub(crate) folder_tree: Option<FolderNode>,
    pub(crate) selected_folder: Option<PathBuf>,
    pub(crate) selected_items: HashSet<PathBuf>,
    pub(crate) operations: FileOperations,
    pub(crate) fs_metadata: FsMetadataManager,
    pub(crate) drag_state: DragState,
    pub(crate) hovered_drop_folder: Option<PathBuf>,
    pub(crate) show_drop_hint: bool,
    pub(crate) breadcrumb_hover_timer: Option<gpui::Task<()>>,
    pub(crate) breadcrumb_hover_path: Option<PathBuf>,
    pub(crate) resizable_state: Entity<ResizableState>,
    pub(crate) view_mode: ViewMode,
    pub(crate) sort_by: SortBy,
    pub(crate) sort_order: SortOrder,
    pub(crate) show_hidden_files: bool,
    pub(crate) renaming_item: Option<PathBuf>,
    pub(crate) rename_input_state: Entity<InputState>,
    pub(crate) registered_file_types: Vec<plugin_editor_api::FileTypeDefinition>,
    pub(crate) search_query: String,
    pub(crate) folder_search_state: Entity<InputState>,
    pub(crate) file_filter_query: String,
    pub(crate) file_filter_state: Entity<InputState>,
    pub(crate) directory_cache: Option<(PathBuf, Vec<FileItem>)>,
    pub(crate) directory_cache_dirty: bool,
    pub(crate) fs_event_listener: Option<gpui::Task<()>>,
    pub(crate) clipboard: Option<(Vec<PathBuf>, bool)>,
    pub(crate) grid_scroll_handle: VirtualListScrollHandle,
    pub(crate) list_scroll_handle: VirtualListScrollHandle,
    pub(crate) thumbnails:
        std::collections::HashMap<std::path::PathBuf, Option<std::sync::Arc<gpui::RenderImage>>>,
    pub(crate) thumbnail_cache_root: std::path::PathBuf,
}

impl FileManagerDrawer {
    pub fn new(project_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let resizable_state = ResizableState::new(cx);
        let rename_input_state = cx.new(|cx| InputState::new(window, cx));
        let folder_search_state = cx.new(|cx| InputState::new(window, cx));
        let file_filter_state = cx.new(|cx| InputState::new(window, cx));

        cx.subscribe(
            &rename_input_state,
            |drawer, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::PressEnter { .. } = event {
                    drawer.commit_rename(cx);
                }
            },
        )
        .detach();

        cx.subscribe(
            &folder_search_state,
            |drawer, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::Change = event {
                    drawer.search_query = drawer.folder_search_state.read(cx).text().to_string();
                    cx.notify();
                }
            },
        )
        .detach();

        cx.subscribe(
            &file_filter_state,
            |drawer, _input, event: &ui::input::InputEvent, cx| {
                if let ui::input::InputEvent::Change = event {
                    drawer.file_filter_query = drawer.file_filter_state.read(cx).text().to_string();
                    cx.notify();
                }
            },
        )
        .detach();

        let operations = FileOperations::new(project_path.clone());
        let fs_metadata = FsMetadataManager::new();
        let folder_tree = crate::preload::take_preloaded_tree()
            .or_else(|| project_path.as_ref().and_then(|p| FolderNode::from_path(p)));

        let mut this = Self {
            folder_tree,
            project_path: project_path.clone(),
            selected_folder: project_path.clone(),
            selected_items: HashSet::new(),
            operations,
            fs_metadata,
            drag_state: DragState::None,
            hovered_drop_folder: None,
            show_drop_hint: false,
            breadcrumb_hover_timer: None,
            breadcrumb_hover_path: None,
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
            directory_cache: None,
            directory_cache_dirty: true,
            fs_event_listener: None,
            show_hidden_files: false,
            clipboard: None,
            registered_file_types: Vec::new(),
            grid_scroll_handle: VirtualListScrollHandle::new(),
            list_scroll_handle: VirtualListScrollHandle::new(),
            thumbnails: std::collections::HashMap::new(),
            thumbnail_cache_root: project_path
                .as_deref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
                }),
        };

        this.fs_event_listener = Some(cx.spawn(async move |drawer, cx| {
            let mut rx = engine_fs::subscribe();
            while let Ok(event) = rx.recv().await {
                let _ = cx.update(|cx| {
                    drawer.update(cx, |drawer, cx| {
                        let Some(project_root) = drawer.project_path.clone() else {
                            return;
                        };
                        if !event.path.starts_with(&project_root) {
                            return;
                        }
                        drawer.mark_directory_cache_dirty();
                        if !matches!(event.kind, engine_fs::FsChangeKind::Modified) {
                            drawer.folder_tree = FolderNode::from_path(&project_root);
                        }
                        cx.notify();
                    })
                });
            }
        }));

        this
    }

    pub fn new_in_window(
        project_path: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::new(project_path, window, cx)
    }

    pub fn update_file_types(&mut self, file_types: Vec<plugin_editor_api::FileTypeDefinition>) {
        self.registered_file_types = file_types;
    }

    pub(crate) fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let e = entry?;
            let sp = e.path();
            let dp = dst.join(e.file_name());
            if sp.is_dir() {
                Self::copy_dir_recursive(&sp, &dp)?;
            } else {
                std::fs::copy(&sp, &dp)?;
            }
        }
        Ok(())
    }
}

impl EventEmitter<FileSelected> for FileManagerDrawer {}
impl EventEmitter<PopoutFileManagerEvent> for FileManagerDrawer {}
impl EventEmitter<ui_types_common::DragEvent> for FileManagerDrawer {}

impl Render for FileManagerDrawer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .key_context("FileManagerDrawer")
            .on_action(cx.listener(|this, _: &RefreshFileManager, _w, cx| {
                if let Some(ref p) = this.project_path {
                    this.folder_tree = FolderNode::from_path(p);
                }
                this.mark_directory_cache_dirty();
                cx.notify();
            }))
            .on_action(cx.listener(|this, a: &CreateAsset, _w, cx| {
                crate::handlers::handle_create_asset(this, a, cx)
            }))
            .on_action(cx.listener(|this, a: &NewFolder, _w, cx| {
                crate::handlers::handle_new_folder(this, a, cx)
            }))
            .on_action(cx.listener(|this, _: &DeleteItem, _w, cx| {
                crate::handlers::handle_delete_item(this, cx)
            }))
            .on_action(cx.listener(|this, _: &RenameItem, w, cx| {
                crate::handlers::handle_rename_item(this, w, cx)
            }))
            .on_action(cx.listener(|this, _: &ui::input::Escape, _w, cx| {
                if this.renaming_item.is_some() {
                    this.renaming_item = None;
                    cx.notify();
                }
                crate::utils::cancel_drag(this, cx);
            }))
            .on_action(cx.listener(|this, _: &DuplicateItem, _w, cx| {
                crate::handlers::handle_duplicate_item(this, cx)
            }))
            .on_action(cx.listener(|this, _: &Copy, _w, cx| crate::handlers::handle_copy(this, cx)))
            .on_action(cx.listener(|this, _: &Cut, _w, cx| crate::handlers::handle_cut(this, cx)))
            .on_action(
                cx.listener(|this, _: &Paste, _w, cx| crate::handlers::handle_paste(this, cx)),
            )
            .on_action(cx.listener(|this, a: &OpenInFileManager, _w, cx| {
                crate::handlers::handle_open_in_file_manager(this, a, cx)
            }))
            .on_action(cx.listener(|this, a: &OpenTerminalHere, _w, cx| {
                crate::handlers::handle_open_terminal_here(this, a, cx)
            }))
            .on_action(cx.listener(|this, a: &ValidateAsset, _w, cx| {
                crate::handlers::handle_validate_asset(this, a, cx)
            }))
            .on_action(cx.listener(|this, a: &ToggleFavorite, _w, cx| {
                crate::handlers::handle_toggle_favorite(this, a, cx)
            }))
            .on_action(cx.listener(|this, a: &ToggleGitignore, _w, cx| {
                crate::handlers::handle_toggle_gitignore(this, a, cx)
            }))
            .on_action(cx.listener(|this, a: &ToggleHidden, _w, cx| {
                crate::handlers::handle_toggle_hidden(this, a, cx)
            }))
            .on_action(cx.listener(|this, a: &ShowHistory, _w, cx| {
                crate::handlers::handle_show_history(this, a, cx)
            }))
            .on_action(cx.listener(|this, a: &CheckMultiuserSync, _w, cx| {
                crate::handlers::handle_check_multiuser_sync(this, a, cx)
            }))
            .on_action(cx.listener(|this, a: &SetColorOverride, _w, cx| {
                crate::handlers::handle_set_color_override(this, a, cx)
            }))
            .child(self.render_content(window, cx))
    }
}
