//! List and details views: the virtualised row list, column header with
//! sort toggles and one row per entry. Entry point: `render_list_view`.

use engine_state::ConfigValue;
use gpui::prelude::*;
use gpui::*;
use rust_i18n::t;
use std::path::PathBuf;
use ui::{
    button::ButtonVariants as _, h_flex, input::TextInput, popup_menu::PopupMenuExt as _,
    scroll::Scrollbar, v_flex, ActiveTheme as _, Icon, IconName, Selectable as _, Sizable as _,
};
use ui::{menu::context_menu::ContextMenuExt, StyledExt};

use crate::utils::{
    actions::*,
    helpers::{
        format_file_size, format_modified, get_icon_color_for_file_type, get_icon_for_file_type,
        type_label,
    },
    types::*,
};

use super::content::pane_width;
use super::FileManagerDrawer;

const WIDE_PANE: f32 = 560.0;
const TYPE_COL: f32 = 150.0;
const MODIFIED_COL: f32 = 140.0;
const SIZE_COL: f32 = 80.0;

impl FileManagerDrawer {
    /// A column header was clicked: sort by it, or flip the order if it already is.
    fn sort_by_column(&mut self, by: SortBy, cx: &mut Context<Self>) {
        if self.sort_by == by {
            self.sort_order = match self.sort_order {
                SortOrder::Ascending => SortOrder::Descending,
                SortOrder::Descending => SortOrder::Ascending,
            };
        } else {
            self.sort_by = by;
            self.sort_order = SortOrder::Ascending;
        }
        let by_key = match self.sort_by {
            SortBy::Name => "name",
            SortBy::Modified => "modified",
            SortBy::Size => "size",
            SortBy::Type => "type",
        };
        let order_key = match self.sort_order {
            SortOrder::Ascending => "ascending",
            SortOrder::Descending => "descending",
        };
        self.persist_preference("sort_by", ConfigValue::String(by_key.into()));
        self.persist_preference("sort_order", ConfigValue::String(order_key.into()));
        cx.notify();
    }
}

/// Virtualised list/details view with the sortable column header.
pub(super) fn render_list_view(
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
    let mode = d.view_mode;
    let wide = pane_width(d, w, cx) >= WIDE_PANE;
    let handle = d.list_scroll_handle.clone();
    if let Some(index) = d.take_pending_reveal(&items) {
        handle.scroll_to_item(index, gpui::ScrollStrategy::Center);
    }
    let scrollbar_state = d.list_scrollbar_state.clone();
    let row_height = mode.row_height();
    let list = div()
        .relative()
        .flex_1()
        .min_h_0()
        .overflow_hidden()
        .px_2()
        .pt_2()
        .child(
            uniform_list(
                "file-manager-list",
                n,
                cx.processor(move |this, range: std::ops::Range<usize>, w, cx| {
                    range
                        .map(|i| {
                            div()
                                .w_full()
                                .h(px(row_height + 4.0))
                                .child(render_list_item(this, &items[i], w, cx, mode, wide))
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
        );
    if mode != ViewMode::Details {
        return list.into_any_element();
    }
    v_flex()
        .flex_1()
        .min_h_0()
        .child(render_details_header(d, wide, cx))
        .child(list)
        .into_any_element()
}

/// The sortable column headers of the Details view. They line up with the
/// columns of [`render_list_item`]: the row's left padding, icon box and gap
/// are repeated before the Name header.
pub(super) fn render_details_header(
    d: &FileManagerDrawer,
    wide: bool,
    cx: &mut Context<FileManagerDrawer>,
) -> impl IntoElement {
    let theme = cx.theme();
    let (active, order) = (d.sort_by, d.sort_order);
    let cell = |id: &'static str, label: &'static str, by: SortBy, width: Option<f32>| {
        let arrow = if active == by {
            match order {
                SortOrder::Ascending => " ▲",
                SortOrder::Descending => " ▼",
            }
        } else {
            ""
        };
        let base = h_flex()
            .id(id)
            .h_full()
            .items_center()
            .cursor_pointer()
            .text_xs()
            .font_medium()
            .text_color(if active == by {
                theme.foreground
            } else {
                theme.muted_foreground
            })
            .hover(|e| e.text_color(theme.foreground))
            .child(format!("{label}{arrow}"))
            .on_click(cx.listener(move |d, _, _, cx| d.sort_by_column(by, cx)));
        match width {
            Some(width) => base.w(px(width)).flex_none(),
            None => base.flex_1().min_w_0(),
        }
    };
    h_flex()
        .w_full()
        .h(px(26.))
        .px_2()
        .gap_3()
        .items_center()
        .border_b_1()
        .border_color(theme.border)
        // Row padding (px_3 inside px_2) + icon box + gap.
        .pl(px(8. + 12. + 20. + 12.))
        .pr(px(8. + 12.))
        .child(cell("hdr-name", "Name", SortBy::Name, None))
        .when(wide, |e| {
            e.child(cell("hdr-type", "Type", SortBy::Type, Some(TYPE_COL)))
                .child(cell(
                    "hdr-modified",
                    "Date modified",
                    SortBy::Modified,
                    Some(MODIFIED_COL),
                ))
        })
        .child(cell("hdr-size", "Size", SortBy::Size, Some(SIZE_COL)))
}

/// Marks a linked source, or the native asset built from it, whose source
/// changed since it was last imported.
pub(super) fn out_of_date_badge(name: &str, cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    let selector = format!("out-of-date-{name}");
    h_flex()
        .id("out-of-date")
        .debug_selector(move || selector.clone())
        .gap_1()
        .px_1p5()
        .py_0p5()
        .rounded_sm()
        .bg(theme.warning.opacity(0.9))
        .text_color(theme.warning_foreground)
        .text_xs()
        .font_weight(gpui::FontWeight::MEDIUM)
        .child(Icon::new(IconName::WarningTriangle).size_3())
        .child(t!("FileManager.OutOfDate").to_string())
        .tooltip(|window, cx| {
            ui::tooltip::Tooltip::new(t!("FileManager.OutOfDateTooltip").to_string())
                .build(window, cx)
        })
}

/// One list or details row, with its context menu and drag handling.
pub(super) fn render_list_item(
    d: &mut FileManagerDrawer,
    item: &FileItem,
    _w: &mut Window,
    cx: &mut Context<FileManagerDrawer>,
    mode: ViewMode,
    wide: bool,
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
    let read_only = item.read_only_mount.clone();
    let import = d.import_state(&item.path).cloned();
    let out_of_date = import.as_ref().is_some_and(|state| state.is_out_of_date());
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
        .h(px(mode.row_height()))
        .px_3()
        .py_1()
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
    let box_px = match mode {
        ViewMode::Compact => 18.0,
        ViewMode::Details => 20.0,
        ViewMode::Tiles => 36.0,
        _ => 24.0,
    };
    let icon_px = match mode {
        ViewMode::Compact | ViewMode::Details => 14.0,
        ViewMode::Tiles => 20.0,
        _ => 16.0,
    };
    let muted = cx.theme().muted_foreground;
    let size_text = if item.is_folder {
        "—".to_string()
    } else {
        format_file_size(item.size)
    };
    let modified_text = item
        .modified
        .map(format_modified)
        .unwrap_or_else(|| "—".to_string());
    let type_text = type_label(item);
    let column = move |width: f32, text: String| {
        div()
            .w(px(width))
            .flex_none()
            .truncate()
            .text_xs()
            .text_color(muted)
            .child(text)
    };
    let name_block = if ren {
        div()
            .flex_1()
            .text_sm()
            .child(TextInput::new(&d.rename_input_state).w_full().xsmall())
            .into_any_element()
    } else {
        let name = div()
            .truncate()
            .text_sm()
            .font_weight(if sel {
                gpui::FontWeight::SEMIBOLD
            } else {
                gpui::FontWeight::NORMAL
            })
            .text_color(cx.theme().foreground)
            .child(item.name.clone());
        // The engine's and plugins' mounted content cannot be changed.
        let name = match &item.read_only_mount {
            Some(mount) => h_flex()
                .gap_1()
                .items_center()
                .child(name)
                .child(
                    Icon::new(IconName::Lock)
                        .size_3()
                        .text_color(cx.theme().muted_foreground),
                )
                .id(SharedString::from(format!("read-only-{}", item.name)))
                .tooltip({
                    let text = format!("Read-only: served by '{mount}'");
                    move |window, cx| ui::tooltip::Tooltip::new(text.clone()).build(window, cx)
                })
                .into_any_element(),
            None => name.into_any_element(),
        };
        if mode == ViewMode::Tiles {
            // Name over a line of details: type, size, modified.
            let mut parts = vec![type_text.clone()];
            if !item.is_folder {
                parts.push(size_text.clone());
            }
            if item.modified.is_some() {
                parts.push(modified_text.clone());
            }
            v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(name)
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(muted)
                        .child(parts.join(" · ")),
                )
                .into_any_element()
        } else {
            div().flex_1().min_w_0().child(name).into_any_element()
        }
    };
    let columns = matches!(mode, ViewMode::List | ViewMode::Details | ViewMode::Compact);
    row.child(
        div()
            .w(px(box_px))
            .h(px(box_px))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded_sm()
            .bg(ic.opacity(0.15))
            .child(Icon::new(icon).size(px(icon_px)).text_color(ic)),
    )
    .child(name_block)
    .when(out_of_date, |e| e.child(out_of_date_badge(&item.name, cx)))
    .when(columns && wide && mode != ViewMode::Compact, |e| {
        e.child(column(TYPE_COL, type_text.clone()))
            .child(column(MODIFIED_COL, modified_text.clone()))
    })
    .when(columns, |e| {
        e.child(column(SIZE_COL, size_text.clone()).text_right())
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
        } else if let Some(mount) = read_only.clone() {
            crate::components::context_menus::read_only_context_menu(mount)(m, w, cx)
        } else {
            crate::components::context_menus::item_context_menu(ip.clone(), hc, cls, import.clone())(
                m, w, cx,
            )
        }
    })
}
