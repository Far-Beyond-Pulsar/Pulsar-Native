use engine_state::{ConfigValue, GlobalSettings};
use gpui::prelude::*;
use gpui::*;
use rust_i18n::t;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use ui::{
    button::{Button, ButtonGroup, ButtonVariants as _},
    h_flex,
    input::{InputState, TextInput},
    menu::context_menu::ContextMenuExt,
    popup_menu::PopupMenuExt as _,
    resizable::{h_resizable, resizable_panel, ResizableState},
    scroll::{Scrollbar, ScrollbarState},
    v_flex, v_virtual_list, ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _,
    StyledExt, VirtualListScrollHandle,
};

use crate::components::commit_picker::{CommitPicker, CommitSelected};
use crate::components::sidebar;
use crate::utils::{
    actions::*,
    fs_metadata::FsMetadataManager,
    helpers::{format_file_size, get_icon_color_for_file_type, get_icon_for_file_type},
    operations::FileOperations,
    tree::FolderNode,
    types::*,
};

pub struct FileManagerDrawer {
    pub project_path: Option<PathBuf>,
    pub(crate) folder_tree: Option<FolderNode>,
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
        std::cell::RefCell<std::collections::HashMap<PathBuf, Vec<FileItem>>>,
    /// An item [`Self::reveal`] asked to scroll into view on the next render.
    pub(crate) pending_reveal: Option<PathBuf>,
    pub(crate) fs_event_listener: Option<gpui::Task<()>>,
    /// Rescans for imports when importable files change (#1101).
    pub(crate) import_watch: Option<crate::utils::import_scan::ImportWatch>,
    pub(crate) clipboard: Option<(Vec<PathBuf>, bool)>,
    pub(crate) grid_scroll_handle: VirtualListScrollHandle,
    pub(crate) list_scroll_handle: VirtualListScrollHandle,
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

        let view_mode = match Self::persisted_choice("view_mode", "grid").as_str() {
            "list" => ViewMode::List,
            _ => ViewMode::Grid,
        };
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
            pending_reveal: None,
            fs_event_listener: None,
            import_watch: None,
            show_hidden_files,
            clipboard: None,
            registered_file_types: Vec::new(),
            grid_scroll_handle: VirtualListScrollHandle::new(),
            list_scroll_handle: VirtualListScrollHandle::new(),
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
                            drawer.folder_tree = FolderNode::from_path(&project_root);
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
                    this.folder_tree = FolderNode::from_path(p);
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

pub fn render_content(
    d: &mut FileManagerDrawer,
    window: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) -> AnyElement {
    if d.folder_tree_hidden {
        return render_file_content(d, window, cx).into_any_element();
    }
    h_resizable("file-manager-resizable")
        .state(d.resizable_state.clone())
        .child(
            resizable_panel()
                .child(sidebar::render_folder_tree(d, window, cx))
                .size(px(250.)),
        )
        .child(resizable_panel().child(render_file_content(d, window, cx)))
        .into_any_element()
}

pub fn render_file_content(
    d: &mut FileManagerDrawer,
    w: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) -> impl IntoElement {
    // Queue thumbnails for every asset in the opened folder before applying
    // the UI filter or virtualizing rows. This keeps offscreen and filtered
    // items from being skipped by the editor task queue.
    if let Some(folder) = d.selected_folder.clone() {
        for item in d.cached_items_for_folder(&folder) {
            if !item.is_folder && !item.is_ghost {
                d.ensure_thumbnail(&item.path, cx);
            }
        }
    }
    let items = d.get_filtered_items();
    let hc = d.clipboard.is_some();
    let sf = d.selected_folder.clone();
    let ft = d.registered_file_types.clone();
    let sh = d.show_drop_hint;

    v_flex()
        .size_full()
        .bg(cx.theme().background)
        .child(render_combined_toolbar(d, &items, w, cx))
        .child({
            let mut cd = v_flex()
                .id("file-content-area")
                .relative()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|d, _e, w, cx| {
                        if d.renaming_item.is_some() {
                            d.commit_rename(cx);
                        }
                        d.focus_handle.clone().focus(w, cx);
                    }),
                )
                .on_mouse_move(cx.listener(|d, _: &MouseMoveEvent, _w, cx| {
                    if !cx.has_active_drag() {
                        d.hovered_drop_folder = None;
                        d.show_drop_hint = false;
                    }
                }))
                .context_menu(move |m, w, cx| {
                    if let Some(p) = sf.clone() {
                        crate::components::context_menus::folder_context_menu(p, hc, ft.clone())(
                            m, w, cx,
                        )
                    } else {
                        m
                    }
                })
                .on_drag_hover::<plugin_editor_api::AssetPayload>(cx.listener(
                    move |d, is_hovered: &bool, _w, cx| {
                        if !*is_hovered && !d.asset_drag_emitted {
                            d.asset_drag_emitted = true;
                            if let Some(payload) =
                                cx.active_drag_value::<plugin_editor_api::AssetPayload>()
                            {
                                cx.emit(ui_types_common::DragEvent::AssetDragStarted(
                                    payload.clone().into(),
                                ));
                            }
                        }
                    },
                ));
            if d.selected_folder.is_some() {
                cd = cd
                    .on_drag_move(
                        cx.listener(move |d, _: &DragMoveEvent<DraggedFile>, _w, cx| {
                            d.hovered_drop_folder = d.selected_folder.clone();
                            d.show_drop_hint = true;
                            cx.notify();
                        }),
                    )
                    .on_drag_move(cx.listener(
                        move |d, _: &DragMoveEvent<ExternalPaths>, _w, cx| {
                            d.hovered_drop_folder = d.selected_folder.clone();
                            d.show_drop_hint = true;
                            cx.notify();
                        },
                    ))
                    .on_drag_move(cx.listener(
                        move |d, _: &DragMoveEvent<plugin_editor_api::AssetPayload>, _w, cx| {
                            d.hovered_drop_folder = d.selected_folder.clone();
                            d.show_drop_hint = true;
                            cx.notify();
                        },
                    ))
                    .drag_over::<DraggedFile>(|s, _, _, cx| {
                        s.bg(cx.theme().accent.opacity(0.12))
                            .border_1()
                            .border_color(cx.theme().accent.opacity(0.8))
                    })
                    .drag_over::<ExternalPaths>(|s, _, _, cx| {
                        s.bg(cx.theme().accent.opacity(0.12))
                            .border_1()
                            .border_color(cx.theme().accent.opacity(0.8))
                    })
                    .drag_over::<plugin_editor_api::AssetPayload>(|s, _, _, cx| {
                        s.bg(cx.theme().accent.opacity(0.12))
                            .border_1()
                            .border_color(cx.theme().accent.opacity(0.8))
                    })
                    .on_drop(cx.listener(move |d, drag: &DraggedFile, w, cx| {
                        d.show_drop_hint = false;
                        d.hovered_drop_folder = None;
                        if let Some(ref f) = d.selected_folder.clone() {
                            d.handle_drop_on_folder_new(f, &drag.paths, w, cx);
                        }
                    }))
                    .on_drop(cx.listener(move |d, ext: &ExternalPaths, w, cx| {
                        d.show_drop_hint = false;
                        d.hovered_drop_folder = None;
                        if let Some(ref f) = d.selected_folder.clone() {
                            d.handle_external_drop_on_folder(f, ext.paths(), w, cx);
                        }
                    }))
                    .on_drop(cx.listener(
                        move |d, asset: &plugin_editor_api::AssetPayload, w, cx| {
                            d.show_drop_hint = false;
                            d.hovered_drop_folder = None;
                            if let Some(ref f) = d.selected_folder.clone() {
                                d.handle_drop_on_folder_new(
                                    f,
                                    &[PathBuf::from(&asset.engine_path)],
                                    w,
                                    cx,
                                );
                            }
                        },
                    ));
            }
            let cd = cd.child(match d.view_mode {
                ViewMode::Grid => render_grid_view(d, &items, w, cx).into_any_element(),
                ViewMode::List => render_list_view(d, &items, w, cx).into_any_element(),
            });
            if sh {
                cd.child(
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .px_4()
                                .py_2()
                                .rounded_lg()
                                .bg(cx.theme().background.opacity(0.88))
                                .border_1()
                                .border_color(cx.theme().accent)
                                .text_sm()
                                .font_medium()
                                .text_color(cx.theme().foreground)
                                .child("Release mouse to begin import"),
                        ),
                )
            } else {
                cd
            }
        })
}

pub fn render_grid_view(
    d: &mut FileManagerDrawer,
    items: &[FileItem],
    w: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) -> impl IntoElement {
    let items: Vec<FileItem> = items.to_vec();
    let n = items.len();
    if n == 0 {
        return v_flex().flex_1().min_h_0().into_any_element();
    }
    let pw = d
        .resizable_state
        .read(cx)
        .sizes()
        .get(1)
        .copied()
        .map(f32::from)
        .unwrap_or_else(|| {
            let vp: f32 = w.viewport_size().width.into();
            (vp - 250.0).max(100.0)
        });
    const CW: f32 = 100.0;
    const CH: f32 = 144.0;
    const G: f32 = 12.0;
    const HP: f32 = 16.0;
    let aw = (pw - HP).max(CW);
    let cols = (((aw + G) / (CW + G)).floor() as usize).max(1);
    let cw = ((aw - (cols.saturating_sub(1)) as f32 * G) / cols as f32).max(CW);
    let rows = n.div_ceil(cols);
    let sizes = Rc::new(vec![size(px(0.0), px(CH + G)); rows]);
    let view = cx.entity().clone();
    let handle = d.grid_scroll_handle.clone();
    if let Some(index) = d.take_pending_reveal(&items) {
        handle.scroll_to_item(index / cols, gpui::ScrollStrategy::Center);
    }
    let scrollbar_state = d.grid_scrollbar_state.clone();
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .px_2()
        .pt_2()
        .child(
            v_virtual_list(
                view,
                "file-manager-grid",
                sizes,
                move |this, range, w, cx| {
                    range
                        .map(|row| {
                            let s = row * cols;
                            let e = (s + cols).min(n);
                            h_flex()
                                .w_full()
                                .gap(px(G))
                                .py(px(G / 2.))
                                .items_start()
                                .children(
                                    (0..cols)
                                        .map(|off| {
                                            let idx = s + off;
                                            if idx < e {
                                                render_grid_item(this, &items[idx], cw, w, cx)
                                                    .into_any_element()
                                            } else {
                                                div()
                                                    .w(px(cw))
                                                    .h(px(CH))
                                                    .invisible()
                                                    .into_any_element()
                                            }
                                        })
                                        .collect::<Vec<_>>(),
                                )
                                .into_any_element()
                        })
                        .collect()
                },
            )
            .track_scroll(&handle),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .child(Scrollbar::vertical(&scrollbar_state, &handle)),
        )
        .into_any_element()
}

pub fn render_grid_item(
    d: &mut FileManagerDrawer,
    item: &FileItem,
    cw: f32,
    _w: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) -> impl IntoElement {
    let sel = d.selected_items.contains(&item.path);
    let ren = d.renaming_item.as_ref() == Some(&item.path);
    let icon = get_icon_for_file_type(item);
    let ic = get_icon_color_for_file_type(item, cx.theme(), &mut d.fs_metadata);
    let icl = item.clone();
    let idc = item.clone();
    let irc = item.clone();
    let irc_path = item.path.clone();
    let ip = item.path.clone();
    let ihp = item.path.clone();
    let hc = d.clipboard.is_some();
    let cls = item.is_class();
    let fld = item.is_folder;
    let ghost = item.is_ghost;
    if !fld {
        d.ensure_thumbnail(&item.path, cx);
    }
    let thumb = d.thumbnails.get(&item.path).and_then(|t| t.clone());
    let paths = if sel {
        d.selected_items.iter().cloned().collect()
    } else {
        vec![item.path.clone()]
    };
    let dd = DraggedFile {
        paths,
        is_folder: item.is_folder,
        drag_start_position: None,
    };
    let ifd = item.clone();
    let mut inner = v_flex()
        .id(SharedString::from(format!("grid-item-{}", item.name)))
        .w_full()
        .h_full()
        .items_stretch()
        .justify_start();
    if fld {
        inner = inner.on_drag(dd, move |d, pos, _, cx| {
            let mut x = d.clone();
            x.drag_start_position = Some(pos);
            cx.stop_propagation();
            cx.new(|_| x)
        });
    } else {
        let ap = if cls {
            plugin_editor_api::AssetPayload {
                engine_path: icl.path.to_string_lossy().replace('\\', "/"),
                name: icl.name.clone(),
                kind: plugin_editor_api::AssetKind::Blueprint,
                extension: "class".to_string(),
            }
        } else {
            plugin_editor_api::AssetPayload::from_path(&icl.path)
        };
        inner = inner.on_drag(ap, move |d, _, _, cx| {
            cx.stop_propagation();
            cx.new(|_| d.clone())
        });
    }
    if fld {
        let (d1, d2, d3, d4, d5) = (
            ifd.path.clone(),
            ifd.path.clone(),
            ifd.path.clone(),
            ifd.path.clone(),
            ifd.path.clone(),
        );
        inner = inner
            .on_drag_move(
                cx.listener(move |d, _: &DragMoveEvent<ExternalPaths>, _w, cx| {
                    d.hovered_drop_folder = Some(d1.clone());
                    d.show_drop_hint = true;
                    d.start_breadcrumb_hover_timer(&d1, cx);
                    cx.notify();
                }),
            )
            .on_drag_move(cx.listener(
                move |d, _: &DragMoveEvent<plugin_editor_api::AssetPayload>, _w, cx| {
                    d.start_breadcrumb_hover_timer(&d5, cx);
                },
            ))
            .drag_over::<DraggedFile>(|s, _, _, cx| {
                s.bg(cx.theme().accent.opacity(0.2))
                    .border_2()
                    .border_color(cx.theme().accent)
                    .rounded_lg()
            })
            .drag_over::<plugin_editor_api::AssetPayload>(|s, _, _, cx| {
                s.bg(cx.theme().accent.opacity(0.2))
                    .border_2()
                    .border_color(cx.theme().accent)
                    .rounded_lg()
            })
            .drag_over::<ExternalPaths>(|s, _, _, cx| {
                s.bg(cx.theme().accent.opacity(0.2))
                    .border_2()
                    .border_color(cx.theme().accent)
                    .rounded_lg()
            })
            .on_drop(cx.listener(move |d, drag: &DraggedFile, w, cx| {
                cx.stop_propagation();
                d.handle_drop_on_folder_new(&d2, &drag.paths, w, cx);
            }))
            .on_drop(
                cx.listener(move |d, p: &plugin_editor_api::AssetPayload, w, cx| {
                    cx.stop_propagation();
                    d.handle_drop_on_folder_new(
                        &d3,
                        &[std::path::PathBuf::from(&p.engine_path)],
                        w,
                        cx,
                    );
                }),
            )
            .on_drop(cx.listener(move |d, ext: &ExternalPaths, w, cx| {
                cx.stop_propagation();
                d.handle_external_drop_on_folder(&d4, ext.paths(), w, cx);
            }));
    }
    div()
        .w(px(cw))
        .h(px(144.0))
        .rounded_lg()
        .overflow_hidden()
        .border_1()
        .when(ghost, |e| e.opacity(0.45))
        .when(sel, |e| {
            e.border_color(cx.theme().accent)
                .bg(cx.theme().secondary)
                .shadow_md()
        })
        .when(!sel, |e| {
            e.border_color(cx.theme().border.opacity(0.3))
                .bg(cx.theme().sidebar.opacity(0.5))
        })
        .cursor_pointer()
        .hover(|s| {
            s.bg(cx.theme().secondary.opacity(0.7))
                .border_color(cx.theme().accent.opacity(0.7))
                .shadow_lg()
        })
        .child(
            inner
                .child(
                    div()
                        .w_full()
                        .flex_1()
                        .min_h_0()
                        .rounded_tl(px(8.0))
                        .rounded_tr(px(8.0))
                        .bg(ic.opacity(0.15))
                        .flex()
                        .items_center()
                        .justify_center()
                        .overflow_hidden()
                        .map(|e| match thumb {
                            Some(ref img) => e.child(
                                gpui::img(gpui::ImageSource::Render(img.clone()))
                                    .w_full()
                                    .h_full()
                                    .object_fit(gpui::ObjectFit::Cover),
                            ),
                            None => e.child(Icon::new(icon).size(px(48.0)).text_color(ic)),
                        }),
                )
                .child(if ren {
                    div()
                        .w_full()
                        .h(px(30.0))
                        .px_2()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .text_xs()
                        .text_center()
                        .child(TextInput::new(&d.rename_input_state).xsmall())
                        .into_any_element()
                } else {
                    div()
                        .w_full()
                        .h(px(30.0))
                        .px_2()
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(cx.theme().background.opacity(0.9))
                        .text_xs()
                        .text_center()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(cx.theme().foreground)
                        .overflow_hidden()
                        .text_ellipsis()
                        .line_height(rems(1.3))
                        .child(item.name.clone())
                        .into_any_element()
                })
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |d, e: &MouseDownEvent, w: &mut Window, cx| {
                        if ren {
                            cx.stop_propagation();
                            return;
                        }
                        if d.renaming_item.is_some() {
                            d.commit_rename(cx);
                        }
                        d.focus_handle.clone().focus(w, cx);
                        if e.click_count == 2 {
                            crate::handlers::handle_item_double_click(d, &idc, cx);
                        } else {
                            crate::handlers::handle_item_click(d, &icl, &e.modifiers, cx);
                        }
                    }),
                )
                .on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(move |d, _: &MouseDownEvent, _w: &mut Window, cx| {
                        if !d.selected_items.contains(&irc_path) {
                            d.selected_items.clear();
                            d.selected_items.insert(irc_path.clone());
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }),
                )
                .on_drag_move(
                    cx.listener(move |d, _: &DragMoveEvent<DraggedFile>, _w, cx| {
                        d.hovered_drop_folder = if fld { Some(ihp.clone()) } else { None };
                        d.show_drop_hint = fld;
                        if fld {
                            d.start_breadcrumb_hover_timer(&ihp, cx);
                        }
                        cx.notify();
                    }),
                )
                .context_menu(move |m, w, cx| {
                    if ghost {
                        crate::components::context_menus::deleted_item_context_menu(&irc)(m, w, cx)
                    } else {
                        crate::components::context_menus::item_context_menu(ip.clone(), hc, cls)(
                            m, w, cx,
                        )
                    }
                }),
        )
}

pub fn render_list_view(
    d: &mut FileManagerDrawer,
    items: &[FileItem],
    _w: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) -> impl IntoElement {
    let items: Vec<FileItem> = items.to_vec();
    let n = items.len();
    if n == 0 {
        return v_flex().flex_1().min_h_0().into_any_element();
    }
    let sizes = Rc::new(vec![size(px(0.0), px(40.0)); n]);
    let view = cx.entity().clone();
    let handle = d.list_scroll_handle.clone();
    if let Some(index) = d.take_pending_reveal(&items) {
        handle.scroll_to_item(index, gpui::ScrollStrategy::Center);
    }
    let scrollbar_state = d.list_scrollbar_state.clone();
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .px_2()
        .pt_2()
        .child(
            v_virtual_list(
                view,
                "file-manager-list",
                sizes,
                move |this, range, w, cx| {
                    range
                        .map(|i| render_list_item(this, &items[i], w, cx).into_any_element())
                        .collect()
                },
            )
            .track_scroll(&handle),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .child(Scrollbar::vertical(&scrollbar_state, &handle)),
        )
        .into_any_element()
}

pub fn render_list_item(
    d: &mut FileManagerDrawer,
    item: &FileItem,
    _w: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) -> impl IntoElement {
    let sel = d.selected_items.contains(&item.path);
    let ren = d.renaming_item.as_ref() == Some(&item.path);
    let icon = get_icon_for_file_type(item);
    let ic = get_icon_color_for_file_type(item, cx.theme(), &mut d.fs_metadata);
    let icl = item.clone();
    let idc = item.clone();
    let irc = item.clone();
    let irc_path = item.path.clone();
    let ip = item.path.clone();
    let ihp = item.path.clone();
    let hc = d.clipboard.is_some();
    let cls = item.is_class();
    let fld = item.is_folder;
    let ghost = item.is_ghost;
    let paths = if sel {
        d.selected_items.iter().cloned().collect()
    } else {
        vec![item.path.clone()]
    };
    let dd = DraggedFile {
        paths,
        is_folder: item.is_folder,
        drag_start_position: None,
    };
    let ifd = item.clone();
    let mut row = h_flex()
        .id(SharedString::from(format!("list-item-{}", item.name)))
        .w_full()
        .h(px(36.))
        .px_3()
        .py_1p5()
        .gap_3()
        .items_center()
        .rounded_md()
        .border_1()
        .cursor_pointer()
        .when(sel, |e| {
            e.bg(cx.theme().secondary)
                .border_l_2()
                .border_color(cx.theme().accent)
        })
        .when(!sel, |e| e.border_color(gpui::transparent_black()))
        .hover(|e| {
            e.bg(cx.theme().secondary.opacity(0.5))
                .border_color(cx.theme().accent.opacity(0.2))
        });
    row = row.on_drag(dd, move |d, pos, _, cx| {
        let mut x = d.clone();
        x.drag_start_position = Some(pos);
        cx.stop_propagation();
        cx.new(|_| x)
    });
    if !fld {
        let ap = if cls {
            plugin_editor_api::AssetPayload {
                engine_path: icl.path.to_string_lossy().replace('\\', "/"),
                name: icl.name.clone(),
                kind: plugin_editor_api::AssetKind::Blueprint,
                extension: "class".to_string(),
            }
        } else {
            plugin_editor_api::AssetPayload::from_path(&icl.path)
        };
        row = row.on_drag(ap, move |d, _, _, cx| {
            cx.stop_propagation();
            cx.new(|_| d.clone())
        });
    }
    if fld {
        let (d1, d2, d3, d4, d5) = (
            ifd.path.clone(),
            ifd.path.clone(),
            ifd.path.clone(),
            ifd.path.clone(),
            ifd.path.clone(),
        );
        row = row
            .on_drag_move(
                cx.listener(move |d, _: &DragMoveEvent<ExternalPaths>, _w, cx| {
                    d.hovered_drop_folder = Some(d1.clone());
                    d.show_drop_hint = true;
                    d.start_breadcrumb_hover_timer(&d1, cx);
                    cx.notify();
                }),
            )
            .on_drag_move(cx.listener(
                move |d, _: &DragMoveEvent<plugin_editor_api::AssetPayload>, _w, cx| {
                    d.start_breadcrumb_hover_timer(&d5, cx);
                },
            ))
            .drag_over::<DraggedFile>(|s, _, _, cx| {
                s.bg(cx.theme().accent.opacity(0.2))
                    .border_2()
                    .border_color(cx.theme().accent)
            })
            .drag_over::<plugin_editor_api::AssetPayload>(|s, _, _, cx| {
                s.bg(cx.theme().accent.opacity(0.2))
                    .border_2()
                    .border_color(cx.theme().accent)
            })
            .drag_over::<ExternalPaths>(|s, _, _, cx| {
                s.bg(cx.theme().accent.opacity(0.2))
                    .border_2()
                    .border_color(cx.theme().accent)
            })
            .on_drop(cx.listener(move |d, drag: &DraggedFile, w, cx| {
                cx.stop_propagation();
                d.handle_drop_on_folder_new(&d2, &drag.paths, w, cx);
            }))
            .on_drop(
                cx.listener(move |d, p: &plugin_editor_api::AssetPayload, w, cx| {
                    cx.stop_propagation();
                    d.handle_drop_on_folder_new(
                        &d3,
                        &[std::path::PathBuf::from(&p.engine_path)],
                        w,
                        cx,
                    );
                }),
            )
            .on_drop(cx.listener(move |d, ext: &ExternalPaths, w, cx| {
                cx.stop_propagation();
                d.handle_external_drop_on_folder(&d4, ext.paths(), w, cx);
            }));
    }
    row.child(
        div()
            .w(px(24.0))
            .h(px(24.0))
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .bg(ic.opacity(0.15))
            .child(Icon::new(icon).size_4().text_color(ic)),
    )
    .child(if ren {
        div()
            .flex_1()
            .text_sm()
            .child(TextInput::new(&d.rename_input_state).w_full().xsmall())
            .into_any_element()
    } else {
        div()
            .flex_1()
            .text_sm()
            .font_weight(if sel {
                gpui::FontWeight::SEMIBOLD
            } else {
                gpui::FontWeight::NORMAL
            })
            .text_color(cx.theme().foreground)
            .child(item.name.clone())
            .into_any_element()
    })
    .when(!item.is_folder, |e| {
        e.child(
            div()
                .px_2()
                .py_0p5()
                .rounded_sm()
                .bg(cx.theme().muted.opacity(0.2))
                .text_xs()
                .font_family("monospace")
                .text_color(cx.theme().muted_foreground)
                .child(format_file_size(item.size)),
        )
    })
    .on_mouse_down(
        gpui::MouseButton::Left,
        cx.listener(move |d, e: &MouseDownEvent, _w: &mut Window, cx| {
            if ren {
                cx.stop_propagation();
                return;
            }
            if d.renaming_item.is_some() {
                d.commit_rename(cx);
            }
            if e.click_count == 2 {
                crate::handlers::handle_item_double_click(d, &idc, cx);
            } else {
                crate::handlers::handle_item_click(d, &icl, &e.modifiers, cx);
            }
        }),
    )
    .on_mouse_down(
        gpui::MouseButton::Right,
        cx.listener(move |d, _: &MouseDownEvent, _w: &mut Window, cx| {
            if !d.selected_items.contains(&irc_path) {
                d.selected_items.clear();
                d.selected_items.insert(irc_path.clone());
                cx.notify();
            }
            cx.stop_propagation();
        }),
    )
    .on_drag_move(
        cx.listener(move |d, _: &DragMoveEvent<DraggedFile>, _w, cx| {
            d.hovered_drop_folder = if fld { Some(ihp.clone()) } else { None };
            d.show_drop_hint = fld;
            if fld {
                d.start_breadcrumb_hover_timer(&ihp, cx);
            }
            cx.notify();
        }),
    )
    .context_menu(move |m, w, cx| {
        if ghost {
            crate::components::context_menus::deleted_item_context_menu(&irc)(m, w, cx)
        } else {
            crate::components::context_menus::item_context_menu(ip.clone(), hc, cls)(m, w, cx)
        }
    })
}

pub fn render_combined_toolbar(
    d: &mut FileManagerDrawer,
    items: &[FileItem],
    w: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) -> impl IntoElement {
    let vm = d.view_mode;
    let current_commit = d
        .selected_deleted_commit
        .clone()
        .unwrap_or_else(|| "HEAD".to_string());
    let create_menu_focus = d.focus_handle.clone();
    let more_menu_focus = d.focus_handle.clone();
    let show_hidden_files = d.show_hidden_files;
    h_flex()
        .w_full()
        .h(px(56.))
        .px_4()
        .items_center()
        .gap_3()
        .border_b_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().background)
        .child(render_clickable_breadcrumb(d, items, w, cx))
        .child(
            h_flex()
                .w(px(180.))
                .h(px(32.))
                .px_2()
                .rounded(px(6.))
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().sidebar.opacity(0.5))
                .child(
                    TextInput::new(&d.file_filter_state)
                        .w_full()
                        .xsmall()
                        .appearance(false)
                        .prefix(
                            Icon::new(IconName::Search)
                                .size_3()
                                .text_color(cx.theme().muted_foreground),
                        ),
                ),
        )
        .child(
            div()
                .px_2()
                .py_1()
                .rounded(px(6.))
                .bg(cx.theme().accent.opacity(0.1))
                .border_1()
                .border_color(cx.theme().accent.opacity(0.3))
                .text_xs()
                .font_medium()
                .text_color(cx.theme().foreground)
                .child(t!("FileManager.Items", count => items.len()).to_string()),
        )
        .when(engine_fs::virtual_fs::is_remote(), |e| {
            e.child(
                div()
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .bg(cx.theme().success.opacity(0.12))
                    .border_1()
                    .border_color(cx.theme().success.opacity(0.4))
                    .text_xs()
                    .font_medium()
                    .text_color(cx.theme().success)
                    .child(format!("☁ {}", engine_fs::virtual_fs::current_label())),
            )
        })
        .child(ui::divider::Divider::vertical().h(px(24.)))
        .child(
            ButtonGroup::new("view-mode-group")
                .child(
                    Button::new("toggle-view")
                        .icon(IconName::LayoutDashboard)
                        .tooltip(t!("FileManager.GridView").to_string())
                        .selected(vm == ViewMode::Grid),
                )
                .child(
                    Button::new("toggle-list")
                        .icon(IconName::List)
                        .tooltip(t!("FileManager.ListView").to_string())
                        .selected(vm == ViewMode::List),
                )
                .ghost()
                .on_click(cx.listener(|d, s: &Vec<usize>, _w, cx| {
                    if s.contains(&0) {
                        d.view_mode = ViewMode::Grid;
                        d.persist_preference("view_mode", ConfigValue::String("grid".into()));
                    } else if s.contains(&1) {
                        d.view_mode = ViewMode::List;
                        d.persist_preference("view_mode", ConfigValue::String("list".into()));
                    }
                    cx.notify();
                })),
        )
        .child(
            Button::new("create-item")
                .icon(IconName::Plus)
                .ghost()
                .tooltip("Create file or folder")
                .popup_menu(move |menu, _, _| {
                    menu.action_context(create_menu_focus.clone())
                        .menu_with_icon(
                            t!("FileManager.NewFile").to_string(),
                            Icon::new(IconName::PagePlus),
                            Box::new(NewFile::default()),
                        )
                        .menu_with_icon(
                            t!("FileManager.NewFolder").to_string(),
                            Icon::new(IconName::FolderPlus),
                            Box::new(NewFolder::default()),
                        )
                })
                .anchor(Corner::BottomRight),
        )
        .child(
            Button::new("file-manager-more")
                .icon(IconName::Ellipsis)
                .ghost()
                .tooltip("More file options")
                .popup_menu(move |menu, _, _| {
                    menu.action_context(more_menu_focus.clone())
                        .menu_with_check(
                            if show_hidden_files {
                                "Hide hidden files"
                            } else {
                                "Show hidden files"
                            },
                            show_hidden_files,
                            Box::new(ToggleHiddenFiles),
                        )
                        .separator()
                        .menu_with_icon(
                            t!("FileManager.Refresh").to_string(),
                            Icon::new(IconName::Refresh),
                            Box::new(RefreshFileManager),
                        )
                        .menu_with_icon(
                            t!("FileManager.OpenInFileManager").to_string(),
                            Icon::new(IconName::ExternalLink),
                            Box::new(OpenInFileManager::default()),
                        )
                        .separator()
                        .menu_with_icon(
                            "Pop out to new window",
                            Icon::new(IconName::ArrowUpRightSquare),
                            Box::new(PopoutFileManager),
                        )
                        .menu_with_icon(
                            "Dock at bottom",
                            Icon::new(IconName::PanelBottom),
                            Box::new(DockFileManager),
                        )
                })
                .anchor(Corner::BottomRight),
        )
        .when_some(d.project_path.as_ref(), |el, _| {
            el.child(ui::divider::Divider::vertical().h(px(24.)))
                .child(
                    Button::new("toggle-deleted")
                        .icon(IconName::GitPullRequest)
                        .ghost()
                        .selected(d.show_deleted_files)
                        .tooltip("Browse deleted files from git history")
                        .on_click(cx.listener(|d, _e, _w, cx| {
                            crate::handlers::handle_toggle_deleted_files(d, cx)
                        })),
                )
                .when(d.show_deleted_files, |el| {
                    el.child(
                        ui::popover::Popover::<
                            crate::components::commit_picker::CommitPicker,
                        >::new("commit-picker")
                        .anchor(Corner::TopRight)
                        .trigger(
                            Button::new("commit-picker-btn")
                                .small()
                                .ghost()
                                .label(current_commit.clone())
                                .tooltip("Select a commit to browse deleted files"),
                        )
                        .content({
                            let picker = d.commit_picker.clone();
                            move |_, _| picker.clone()
                        }),
                    )
                })
        })
}

pub fn render_clickable_breadcrumb(
    d: &mut FileManagerDrawer,
    _items: &[FileItem],
    _w: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) -> impl IntoElement {
    let mut parts = Vec::new();
    if let Some(ref sel) = d.selected_folder {
        if let Some(ref proj) = d.project_path {
            if let Ok(rel) = sel.strip_prefix(proj) {
                let mut cur = proj.clone();
                parts.push(("Project".to_string(), cur.clone()));
                for c in rel.components() {
                    if let Some(n) = c.as_os_str().to_str() {
                        cur = cur.join(n);
                        parts.push((n.to_string(), cur.clone()));
                    }
                }
            }
        }
    }
    if parts.is_empty() {
        parts.push((
            "Project".to_string(),
            d.project_path.clone().unwrap_or_default(),
        ));
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
        .child(Icon::new(IconName::Folder).size_4())
        .children(parts.into_iter().enumerate().flat_map(|(i, (name, path))| {
            let mut els: Vec<AnyElement> = Vec::new();
            if i > 0 {
                els.push(
                    Icon::new(IconName::ChevronRight)
                        .size_3()
                        .text_color(cx.theme().muted_foreground)
                        .into_any_element(),
                );
            }
            let cp = path.clone();
            let hp = path.clone();
            let dp = path.clone();
            let dap = path.clone();
            let dep = path.clone();
            els.push(
                div()
                    .text_sm()
                    .px_1()
                    .py_px()
                    .rounded(px(4.))
                    .text_color(cx.theme().foreground)
                    .font_medium()
                    .cursor_pointer()
                    .hover(|s| s.bg(cx.theme().accent.opacity(0.15)))
                    .child(name)
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(move |d, _: &MouseDownEvent, _w: &mut Window, cx| {
                            d.selected_folder = Some(cp.clone());
                            cx.notify();
                        }),
                    )
                    .drag_over::<DraggedFile>(|s, _, _, cx| {
                        s.bg(cx.theme().accent.opacity(0.3))
                            .border_1()
                            .border_color(cx.theme().accent)
                    })
                    .drag_over::<plugin_editor_api::AssetPayload>(|s, _, _, cx| {
                        s.bg(cx.theme().accent.opacity(0.3))
                            .border_1()
                            .border_color(cx.theme().accent)
                    })
                    .drag_over::<ExternalPaths>(|s, _, _, cx| {
                        s.bg(cx.theme().accent.opacity(0.3))
                            .border_1()
                            .border_color(cx.theme().accent)
                    })
                    .on_drop(cx.listener(move |d, drag: &DraggedFile, w, cx| {
                        d.handle_drop_on_folder_new(&dp, &drag.paths, w, cx);
                    }))
                    .on_drop(cx.listener(
                        move |d, asset: &plugin_editor_api::AssetPayload, w, cx| {
                            d.handle_drop_on_folder_new(
                                &dap,
                                &[PathBuf::from(&asset.engine_path)],
                                w,
                                cx,
                            );
                        },
                    ))
                    .on_drop(cx.listener(move |d, ext: &ExternalPaths, w, cx| {
                        d.handle_external_drop_on_folder(&dep, ext.paths(), w, cx);
                    }))
                    .on_mouse_move(cx.listener(move |d, _: &MouseMoveEvent, _w, cx| {
                        if cx.has_active_drag() {
                            d.start_breadcrumb_hover_timer(&hp, cx);
                        }
                    }))
                    .into_any_element(),
            );
            els
        }))
}
