//! The file manager drawer: [`FileManagerDrawer`] state, construction and its
//! `Render` impl. The folder-contents rendering lives in siblings:
//! [`content`] (pane and view-mode dispatch), [`grid`], [`list`] and
//! [`toolbar`].

use engine_state::{ConfigValue, GlobalSettings};
use gpui::prelude::*;
use gpui::*;
use rust_i18n::t;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use ui::{
    button::{Button, ButtonGroup, ButtonVariants as _},
    h_flex,
    input::{InputState, TextInput},
    menu::context_menu::ContextMenuExt,
    popup_menu::PopupMenuExt as _,
    resizable::{h_resizable, resizable_panel, ResizableState},
    scroll::{Scrollbar, ScrollbarState},
    v_flex, ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _, StyledExt,
};

mod content;
mod grid;
mod list;
mod toolbar;

use content::render_content;

use crate::components::commit_picker::{CommitPicker, CommitSelected};
use crate::components::sidebar;
use crate::utils::{
    actions::*,
    fs_metadata::FsMetadataManager,
    helpers::{
        format_file_size, format_modified, get_icon_color_for_file_type, get_icon_for_file_type,
        type_label,
    },
    operations::FileOperations,
    tree::FolderNode,
    types::*,
};

pub struct FileManagerDrawer {
    pub project_path: Option<PathBuf>,
    pub(crate) folder_tree: Option<FolderNode>,
    /// Bumped whenever the folder tree or the cached file listings change, so
    /// views built from them (the sidebar's content tree) know to rebuild.
    pub(crate) content_revision: u64,
    pub(crate) selected_folder: Option<PathBuf>,
    pub(crate) selected_items: HashSet<PathBuf>,
    /// Anchor item for Shift-click range selection.
    pub(crate) selection_anchor: Option<PathBuf>,
    /// Focus handle so the drawer participates in keyboard dispatch (Ctrl+C/V/X/A).
    pub(crate) focus_handle: FocusHandle,
    pub(crate) operations: FileOperations,
    pub(crate) fs_metadata: FsMetadataManager,
    pub(crate) drag_state: DragState,
    pub(crate) hovered_drop_folder: Option<PathBuf>,
    pub(crate) show_drop_hint: bool,
    pub(crate) breadcrumb_hover_timer: Option<gpui::Task<()>>,
    pub(crate) breadcrumb_hover_path: Option<PathBuf>,
    pub(crate) asset_drag_emitted: bool,
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
    /// [`Self::files_in`] listings, by folder; cleared with the directory cache.
    pub(crate) tree_files:
        std::cell::RefCell<std::collections::HashMap<PathBuf, std::rc::Rc<Vec<FileItem>>>>,
    /// An item [`Self::reveal`] asked to scroll into view on the next render.
    pub(crate) pending_reveal: Option<PathBuf>,
    pub(crate) fs_event_listener: Option<gpui::Task<()>>,
    /// Rescans for imports when importable files change (#1101).
    pub(crate) import_watch: Option<crate::utils::import_scan::ImportWatch>,
    /// Import state of the open folder's files, for their context menus and
    /// out-of-date badges (#1102).
    pub(crate) import_states: crate::utils::import_state::FolderImportStates,
    pub(crate) clipboard: Option<(Vec<PathBuf>, bool)>,
    pub(crate) grid_scroll_handle: UniformListScrollHandle,
    pub(crate) list_scroll_handle: UniformListScrollHandle,
    pub(crate) grid_scrollbar_state: ScrollbarState,
    pub(crate) list_scrollbar_state: ScrollbarState,
    pub(crate) thumbnails:
        std::collections::HashMap<std::path::PathBuf, Option<std::sync::Arc<gpui::RenderImage>>>,
    pub(crate) thumbnail_cache_root: std::path::PathBuf,
    pub(crate) show_deleted_files: bool,
    pub(crate) selected_deleted_commit: Option<String>,
    pub(crate) deleted_files: Vec<crate::utils::git_integration::DeletedFileEntry>,
    pub(crate) recent_commits: Vec<crate::utils::git_integration::CommitInfo>,
    pub(crate) commit_picker: gpui::Entity<crate::components::commit_picker::CommitPicker>,
    /// Show only the folder's contents, without the folder tree beside them.
    /// The editor's left sidebar sets this when it supplies the tree itself.
    pub(crate) folder_tree_hidden: bool,
}

impl FileManagerDrawer {
    fn persisted_choice(key: &str, fallback: &str) -> String {
        engine_state::settings::global_config()
            .get(engine_state::settings::NS_EDITOR, "file_manager", key)
            .ok()
            .and_then(|value| value.as_str().ok().map(str::to_owned))
            .unwrap_or_else(|| fallback.to_owned())
    }

    pub(crate) fn persist_preference(&self, key: &str, value: ConfigValue) {
        let Some(handle) = engine_state::settings::global_config()
            .owner_handle(engine_state::settings::NS_EDITOR, "file_manager")
        else {
            return;
        };
        if let Err(error) = handle.set(key, value) {
            tracing::warn!(%error, key, "Could not update file manager preference");
            return;
        }
        if let Err(error) = GlobalSettings::new().save_owner_keys("file_manager", &[key]) {
            tracing::warn!(%error, key, "Could not persist file manager preference");
        }
    }

    pub fn new(project_path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let resizable_state = ResizableState::new(cx);
        let rename_input_state = cx.new(|cx| InputState::new(window, cx));
        let folder_search_state = cx.new(|cx| InputState::new(window, cx));
        let file_filter_state =
            cx.new(|cx| InputState::new(window, cx).placeholder("Filter files…"));

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

        let commit_picker = cx.new(|cx| CommitPicker::new(Vec::new(), window, cx));
        cx.subscribe(
            &commit_picker,
            |drawer, _picker, event: &CommitSelected, cx| {
                crate::handlers::handle_select_deleted_commit(
                    drawer,
                    &crate::utils::actions::SelectDeletedCommit {
                        commit: event.0.clone(),
                    },
                    cx,
                );
            },
        )
        .detach();

        let operations = FileOperations::new(project_path.clone());
        let mut fs_metadata = FsMetadataManager::new();
        if let Some(ref p) = project_path {
            fs_metadata.load_from_project_root(p);
        }
        let folder_tree = crate::preload::take_preloaded_tree()
            .or_else(|| project_path.as_ref().and_then(|p| FolderNode::from_path(p)));

        let view_mode = ViewMode::from_key(&Self::persisted_choice("view_mode", "grid"));
        let sort_by = match Self::persisted_choice("sort_by", "name").as_str() {
            "modified" => SortBy::Modified,
            "size" => SortBy::Size,
            "type" => SortBy::Type,
            _ => SortBy::Name,
        };
        let sort_order = match Self::persisted_choice("sort_order", "ascending").as_str() {
            "descending" => SortOrder::Descending,
            _ => SortOrder::Ascending,
        };
        let show_hidden_files = engine_state::settings::global_config()
            .get(
                engine_state::settings::NS_EDITOR,
                "file_manager",
                "show_hidden_files",
            )
            .ok()
            .and_then(|value| value.as_bool().ok())
            .unwrap_or(false);

        let mut this = Self {
            folder_tree,
            project_path: project_path.clone(),
            selected_folder: project_path.clone(),
            selected_items: HashSet::new(),
            selection_anchor: None,
            focus_handle: cx.focus_handle(),
            operations,
            fs_metadata,
            drag_state: DragState::None,
            hovered_drop_folder: None,
            show_drop_hint: false,
            breadcrumb_hover_timer: None,
            breadcrumb_hover_path: None,
            asset_drag_emitted: false,
            resizable_state,
            renaming_item: None,
            rename_input_state,
            view_mode,
            sort_by,
            sort_order,
            search_query: String::new(),
            folder_search_state,
            file_filter_query: String::new(),
            file_filter_state,
            directory_cache: None,
            directory_cache_dirty: true,
            tree_files: Default::default(),
            content_revision: 0,
            pending_reveal: None,
            fs_event_listener: None,
            import_watch: None,
            import_states: Default::default(),
            show_hidden_files,
            clipboard: None,
            registered_file_types: Vec::new(),
            grid_scroll_handle: UniformListScrollHandle::new(),
            list_scroll_handle: UniformListScrollHandle::new(),
            grid_scrollbar_state: ScrollbarState::default(),
            list_scrollbar_state: ScrollbarState::default(),
            thumbnails: std::collections::HashMap::new(),
            thumbnail_cache_root: project_path
                .as_deref()
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."))
                }),
            show_deleted_files: false,
            selected_deleted_commit: None,
            deleted_files: Vec::new(),
            recent_commits: Vec::new(),
            commit_picker,
            folder_tree_hidden: false,
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
                            drawer.set_folder_tree(FolderNode::from_path(&project_root));
                        }
                        cx.notify();
                    })
                });
            }
        }));

        this.scan_for_imports(window, cx);
        this.import_watch = this.watch_for_imports(window, cx);
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
}

impl EventEmitter<FileSelected> for FileManagerDrawer {}
impl EventEmitter<PopoutFileManagerEvent> for FileManagerDrawer {}
impl EventEmitter<DockFileManagerEvent> for FileManagerDrawer {}
impl EventEmitter<ui_types_common::DragEvent> for FileManagerDrawer {}

impl Render for FileManagerDrawer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.asset_drag_emitted && !cx.has_active_drag() {
            self.asset_drag_emitted = false;
            cx.emit(ui_types_common::DragEvent::AssetDragCancelled);
        }

        div()
            .size_full()
            .track_focus(&self.focus_handle)
            .key_context("FileManagerDrawer")
            // The drawer is mounted over the editor viewport as a flyout.
            // Let its own scrollable children handle wheel input, then stop
            // the event here so it cannot continue into the viewport behind.
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .on_action(cx.listener(|this, _: &RefreshFileManager, _w, cx| {
                if let Some(ref p) = this.project_path {
                    this.set_folder_tree(FolderNode::from_path(p));
                }
                this.mark_directory_cache_dirty();
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleHiddenFiles, _w, cx| {
                this.show_hidden_files = !this.show_hidden_files;
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleDeletedFiles, _w, cx| {
                crate::handlers::handle_toggle_deleted_files(this, cx)
            }))
            .on_action(cx.listener(|_this, _: &PopoutFileManager, w, cx| {
                cx.emit(PopoutFileManagerEvent {
                    position: w.mouse_position(),
                });
            }))
            .on_action(cx.listener(|_, _: &DockFileManager, _w, cx| {
                cx.emit(DockFileManagerEvent);
            }))
            .on_action(cx.listener(|this, a: &CreateAsset, _w, cx| {
                crate::handlers::handle_create_asset(this, a, cx)
            }))
            .on_action(cx.listener(|this, a: &NewFile, w, cx| {
                crate::handlers::handle_new_file(this, a, w, cx)
            }))
            .on_action(cx.listener(|this, a: &NewFolder, w, cx| {
                crate::handlers::handle_new_folder(this, a, w, cx)
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
            .on_action(
                cx.listener(|this, _: &crate::utils::actions::Copy, _w, cx| {
                    crate::handlers::handle_copy(this, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &crate::utils::actions::Cut, _w, cx| {
                crate::handlers::handle_cut(this, cx)
            }))
            .on_action(
                cx.listener(|this, _: &crate::utils::actions::Paste, _w, cx| {
                    crate::handlers::handle_paste(this, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::utils::actions::SelectAll, _w, cx| {
                    crate::handlers::handle_select_all(this, cx)
                }),
            )
            .on_action(cx.listener(|this, a: &OpenInFileManager, _w, cx| {
                crate::handlers::handle_open_in_file_manager(this, a, cx)
            }))
            .on_action(cx.listener(|this, a: &OpenTerminalHere, _w, cx| {
                crate::handlers::handle_open_terminal_here(this, a, cx)
            }))
            .on_action(cx.listener(|this, a: &ReimportSource, w, cx| {
                crate::utils::import_state::handle_reimport_source(this, a, w, cx)
            }))
            .on_action(cx.listener(|this, a: &ImportSource, w, cx| {
                crate::utils::import_state::handle_import_source(this, a, w, cx)
            }))
            .on_action(cx.listener(|this, a: &UnlinkSource, w, cx| {
                crate::utils::import_state::handle_unlink_source(this, a, w, cx)
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
            .on_action(cx.listener(|this, _: &ToggleDeletedFiles, _w, cx| {
                crate::handlers::handle_toggle_deleted_files(this, cx)
            }))
            .on_action(cx.listener(|this, a: &SelectDeletedCommit, _w, cx| {
                crate::handlers::handle_select_deleted_commit(this, a, cx)
            }))
            .on_action(cx.listener(|this, a: &RestoreFile, _w, cx| {
                crate::handlers::handle_restore_file(this, a, _w, cx)
            }))
            .child(render_content(self, window, cx))
    }
}
