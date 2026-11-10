//! The folder-contents pane: the tree/content split, the toolbar slot and the
//! view-mode dispatch (grid, tiles, list, details). Entry points:
//! `render_content` and `render_file_content`.

use gpui::prelude::*;
use gpui::*;
use std::path::PathBuf;
use ui::{
    button::ButtonVariants as _,
    popup_menu::PopupMenuExt as _,
    resizable::{h_resizable, resizable_panel},
    v_flex, ActiveTheme as _, Selectable as _, Sizable as _,
};
use ui::{menu::context_menu::ContextMenuExt, StyledExt};

use crate::components::sidebar;
use crate::utils::{actions::*, types::*};

use super::grid::render_grid_view;
use super::list::render_list_view;
use super::toolbar::render_combined_toolbar;
use super::FileManagerDrawer;

/// The content split: folder tree on the left, the file list on the right.
pub(super) fn render_content(
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

/// The right-hand pane: toolbar plus the grid, tile or list view of the current folder.
pub(super) fn render_file_content(
    d: &mut FileManagerDrawer,
    w: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
) -> impl IntoElement {
    // Import-state discovery runs in the background. Thumbnails are requested
    // by render_grid_item for visible rows only, so opening a large folder does
    // not enqueue work for every offscreen or filtered asset.
    if let Some(folder) = d.selected_folder.clone() {
        let folder_items = d.cached_items_for_folder(&folder);
        d.refresh_import_states(&folder, &folder_items, cx);
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
                ViewMode::Grid => render_grid_view(d, &items, w, cx, false).into_any_element(),
                ViewMode::Tiles => render_grid_view(d, &items, w, cx, true).into_any_element(),
                _ => render_list_view(d, &items, w, cx).into_any_element(),
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

/// Width of the pane that shows the folder's contents.
pub(super) fn pane_width(d: &FileManagerDrawer, w: &Window, cx: &App) -> f32 {
    d.resizable_state
        .read(cx)
        .sizes()
        .get(1)
        .copied()
        .map(f32::from)
        .unwrap_or_else(|| {
            let vp: f32 = w.viewport_size().width.into();
            (vp - 250.0).max(100.0)
        })
}
