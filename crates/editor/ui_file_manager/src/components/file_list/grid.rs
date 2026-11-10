//! Grid and tile views: the wrapped, virtualised grid of file cards.
//! Entry point: `render_grid_view`.

use gpui::prelude::*;
use gpui::*;
use rust_i18n::t;
use std::path::PathBuf;
use ui::{
    button::ButtonVariants as _, h_flex, input::TextInput, popup_menu::PopupMenuExt as _,
    scroll::Scrollbar, v_flex, ActiveTheme as _, Icon, Selectable as _, Sizable as _,
};
use ui::{menu::context_menu::ContextMenuExt, StyledExt};

use crate::components::sidebar;
use crate::utils::{
    actions::*,
    helpers::{get_icon_color_for_file_type, get_icon_for_file_type},
    types::*,
};

use super::content::pane_width;
use super::list::{out_of_date_badge, render_list_item};
use super::FileManagerDrawer;

/// Virtualised grid (or wide tiles) of the folder's entries.
pub(super) fn render_grid_view(
    d: &mut FileManagerDrawer,
    items: &[FileItem],
    w: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
    tiles: bool,
) -> impl IntoElement {
    let items: Vec<FileItem> = items.to_vec();
    let n = items.len();
    if n == 0 {
        return v_flex().flex_1().min_h_0().into_any_element();
    }
    let pw = pane_width(d, w, cx);
    // Tiles are wide cards (icon beside a name and a line of details); the
    // grid is tall cells with the icon above the name.
    let (cw_min, ch, g): (f32, f32, f32) = if tiles {
        (260.0, ViewMode::Tiles.row_height(), 8.0)
    } else {
        (100.0, 144.0, 12.0)
    };
    let (CW, CH, G) = (cw_min, ch, g);
    const HP: f32 = 16.0;
    let aw = (pw - HP).max(CW);
    let cols = (((aw + G) / (CW + G)).floor() as usize).max(1);
    let cw = ((aw - (cols.saturating_sub(1)) as f32 * G) / cols as f32).max(CW);
    let rows = n.div_ceil(cols);
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
            uniform_list(
                "file-manager-grid",
                rows,
                cx.processor(move |this, range: std::ops::Range<usize>, w, cx| {
                    range
                        .map(|row| {
                            let s = row * cols;
                            let e = (s + cols).min(n);
                            h_flex()
                                .w_full()
                                .h(px(CH + G))
                                .gap(px(G))
                                .py(px(G / 2.))
                                .items_start()
                                .children(
                                    (0..cols)
                                        .map(|off| {
                                            let idx = s + off;
                                            if idx < e && tiles {
                                                // Keep column widths independent of each tile's
                                                // text and of the other items in this row.
                                                div()
                                                    .w(px(cw))
                                                    .h(px(CH))
                                                    .flex_none()
                                                    .min_w_0()
                                                    .child(render_list_item(
                                                        this,
                                                        &items[idx],
                                                        w,
                                                        cx,
                                                        ViewMode::Tiles,
                                                        true,
                                                    ))
                                                    .into_any_element()
                                            } else if idx < e {
                                                render_grid_item(this, &items[idx], cw, w, cx)
                                                    .into_any_element()
                                            } else {
                                                div()
                                                    .w(px(cw))
                                                    .h(px(CH))
                                                    .flex_none()
                                                    .invisible()
                                                    .into_any_element()
                                            }
                                        })
                                        .collect::<Vec<_>>(),
                                )
                                .into_any_element()
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .size_full()
            .track_scroll(&handle),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .child(Scrollbar::uniform_scroll(&scrollbar_state, &handle)),
        )
        .into_any_element()
}

/// One grid cell or tile: icon, name, badges and its context menu.
pub(super) fn render_grid_item(
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
    let import = d.import_state(&item.path).cloned();
    let out_of_date = import.as_ref().is_some_and(|state| state.is_out_of_date());
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
        .flex_none()
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
                        .relative()
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
                        })
                        .when(out_of_date, |e| {
                            e.child(
                                div()
                                    .absolute()
                                    .top_1()
                                    .right_1()
                                    .child(out_of_date_badge(&item.name, cx)),
                            )
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
                        crate::components::context_menus::item_context_menu(
                            ip.clone(),
                            hc,
                            cls,
                            import.clone(),
                        )(m, w, cx)
                    }
                }),
        )
}
