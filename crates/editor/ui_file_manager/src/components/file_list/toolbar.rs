//! The combined toolbar above the file list: navigation, breadcrumb, search,
//! sort and view-mode controls. Entry point: `render_combined_toolbar`.

use engine_state::ConfigValue;
use gpui::prelude::*;
use gpui::*;
use rust_i18n::t;
use std::path::PathBuf;
use ui::{
    button::{Button, ButtonGroup, ButtonVariants as _},
    h_flex,
    input::TextInput,
    popup_menu::PopupMenuExt as _,
    ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _,
};
use ui::{menu::context_menu::ContextMenuExt, StyledExt};

use crate::components::commit_picker::CommitPicker;
use crate::components::sidebar;
use crate::utils::{actions::*, types::*};

use super::FileManagerDrawer;

/// Toolbar above the list: navigation, breadcrumb, view mode and actions.
pub(super) fn render_combined_toolbar(
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
                .child(
                    Button::new("toggle-details")
                        .icon(IconName::Table)
                        .tooltip(ViewMode::Details.label())
                        .selected(vm == ViewMode::Details),
                )
                .child(
                    Button::new("toggle-compact")
                        .icon(IconName::Menu)
                        .tooltip(ViewMode::Compact.label())
                        .selected(vm == ViewMode::Compact),
                )
                .child(
                    Button::new("toggle-tiles")
                        .icon(IconName::TableRows)
                        .tooltip(ViewMode::Tiles.label())
                        .selected(vm == ViewMode::Tiles),
                )
                .ghost()
                .on_click(cx.listener(|d, s: &Vec<usize>, _w, cx| {
                    // Button order matches `ViewMode::ALL`.
                    if let Some(mode) = s.first().and_then(|ix| ViewMode::ALL.get(*ix)) {
                        d.view_mode = *mode;
                        d.persist_preference("view_mode", ConfigValue::String(mode.key().into()));
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

/// The path as clickable segments that navigate to each ancestor.
pub(super) fn render_clickable_breadcrumb(
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
