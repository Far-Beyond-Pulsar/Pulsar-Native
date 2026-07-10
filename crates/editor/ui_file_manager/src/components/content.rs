use gpui::prelude::*;
use gpui::*;
use std::rc::Rc;
use ui::{
    h_flex, input::TextInput, menu::context_menu::ContextMenuExt, v_flex, v_virtual_list,
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt,
};

use super::FileManagerDrawer;
use crate::handlers::context_menus;
use crate::utils::{
    helpers::{format_file_size, get_icon_color_for_file_type, get_icon_for_file_type},
    types::*,
};
use plugin_editor_api::{AssetKind, AssetPayload};

impl FileManagerDrawer {
    pub fn render_content(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        use ui::resizable::{h_resizable, resizable_panel};
        h_resizable("file-manager-resizable")
            .state(self.resizable_state.clone())
            .child(
                resizable_panel()
                    .child(self.render_folder_tree(window, cx))
                    .size(px(250.)),
            )
            .child(resizable_panel().child(self.render_file_content(window, cx)))
    }

    pub fn render_file_content(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let items = self.get_filtered_items();
        let hc = self.clipboard.is_some();
        let sf = self.selected_folder.clone();
        let ft = self.registered_file_types.clone();
        let sh = self.show_drop_hint;

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(self.render_combined_toolbar(&items, w, cx))
            .child({
                let mut cd = v_flex()
                    .id("file-content-area")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|d, _e, _w, cx| {
                            if d.renaming_item.is_some() {
                                d.commit_rename(cx);
                            }
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
                            context_menus::folder_context_menu(p, hc, ft.clone())(m, w, cx)
                        } else {
                            m
                        }
                    });
                if self.selected_folder.is_some() {
                    cd = cd
                        .on_drag_move(cx.listener(
                            move |d, _: &DragMoveEvent<DraggedFile>, _w, cx| {
                                d.hovered_drop_folder = d.selected_folder.clone();
                                d.show_drop_hint = true;
                                cx.notify();
                            },
                        ))
                        .on_drag_move(cx.listener(
                            move |d, _: &DragMoveEvent<ExternalPaths>, _w, cx| {
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
                        }));
                }
                let cd = cd.child(match self.view_mode {
                    ViewMode::Grid => self.render_grid_view(&items, w, cx).into_any_element(),
                    ViewMode::List => self.render_list_view(&items, w, cx).into_any_element(),
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
                                    .text_color(cx.theme().accent)
                                    .child("Release mouse to begin import"),
                            ),
                    )
                } else {
                    cd
                }
            })
    }

    pub fn render_grid_view(
        &mut self,
        items: &[FileItem],
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let items: Vec<FileItem> = items.to_vec();
        let n = items.len();
        if n == 0 {
            return v_flex().flex_1().min_h_0().into_any_element();
        }
        let pw = self
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
        const CH: f32 = 110.0;
        const G: f32 = 12.0;
        const HP: f32 = 16.0;
        let aw = (pw - HP).max(CW);
        let cols = (((aw + G) / (CW + G)).floor() as usize).max(1);
        let cw = ((aw - (cols.saturating_sub(1)) as f32 * G) / cols as f32).max(CW);
        let rows = n.div_ceil(cols);
        let sizes = Rc::new(vec![size(px(0.0), px(CH + G)); rows]);
        let view = cx.entity().clone();
        let handle = self.grid_scroll_handle.clone();
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
                                                    this.render_grid_item(&items[idx], cw, w, cx)
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
            .into_any_element()
    }

    pub fn render_grid_item(
        &mut self,
        item: &FileItem,
        cw: f32,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sel = self.selected_items.contains(&item.path);
        let ren = self.renaming_item.as_ref() == Some(&item.path);
        let icon = get_icon_for_file_type(item);
        let ic = get_icon_color_for_file_type(item, cx.theme(), &mut self.fs_metadata);
        let icl = item.clone();
        let idc = item.clone();
        let irc = item.clone();
        let ip = item.path.clone();
        let ihp = item.path.clone();
        let ent = cx.entity().clone();
        let hc = self.clipboard.is_some();
        let cls = item.is_class();
        let fld = item.is_folder;
        if !fld {
            self.ensure_thumbnail(&item.path, cx);
        }
        let thumb = self.thumbnails.get(&item.path).and_then(|t| t.clone());
        let paths = if sel {
            self.selected_items.iter().cloned().collect()
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
            .p_3()
            .gap_2()
            .items_center()
            .justify_center();
        if fld {
            inner = inner.on_drag(dd, move |d, pos, _, cx| {
                let mut x = d.clone();
                x.drag_start_position = Some(pos);
                cx.stop_propagation();
                cx.new(|_| x)
            });
        } else {
            let ap = if cls {
                AssetPayload {
                    engine_path: icl.path.to_string_lossy().replace('\\', "/"),
                    name: icl.name.clone(),
                    kind: AssetKind::Blueprint,
                    extension: "class".to_string(),
                }
            } else {
                AssetPayload::from_path(&icl.path)
            };
            let ep = ap.clone();
            let e2 = ent.clone();
            inner = inner.on_drag(ap, move |d, _, _, cx| {
                e2.update(cx, |_, cx| {
                    cx.emit(ui_types_common::DragEvent::AssetDragStarted(
                        ep.clone().into(),
                    ))
                });
                cx.stop_propagation();
                cx.new(|_| d.clone())
            });
        }
        if fld {
            let (d1, d2, d3, d4) = (
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
                        cx.notify();
                    }),
                )
                .drag_over::<DraggedFile>(|s, _, _, cx| {
                    s.bg(cx.theme().accent.opacity(0.2))
                        .border_2()
                        .border_color(cx.theme().accent)
                        .rounded_lg()
                })
                .drag_over::<AssetPayload>(|s, _, _, cx| {
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
                .on_drop(cx.listener(move |d, p: &AssetPayload, w, cx| {
                    cx.stop_propagation();
                    d.handle_drop_on_folder_new(
                        &d3,
                        &[std::path::PathBuf::from(&p.engine_path)],
                        w,
                        cx,
                    );
                }))
                .on_drop(cx.listener(move |d, ext: &ExternalPaths, w, cx| {
                    cx.stop_propagation();
                    d.handle_external_drop_on_folder(&d4, ext.paths(), w, cx);
                }));
        }
        div()
            .w(px(cw))
            .h(px(110.0))
            .rounded_lg()
            .border_1()
            .when(sel, |e| {
                e.border_color(cx.theme().accent)
                    .bg(cx.theme().accent.opacity(0.1))
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
                            .w(px(48.0))
                            .h(px(48.0))
                            .rounded_lg()
                            .bg(ic.opacity(0.15))
                            .border_1()
                            .border_color(ic.opacity(0.3))
                            .flex()
                            .items_center()
                            .justify_center()
                            .shadow_sm()
                            .overflow_hidden()
                            .map(|e| match thumb {
                                Some(ref img) => e.child(
                                    gpui::img(gpui::ImageSource::Render(img.clone()))
                                        .w(px(48.0))
                                        .h(px(48.0))
                                        .object_fit(gpui::ObjectFit::Cover),
                                ),
                                None => e.child(Icon::new(icon).size(px(24.0)).text_color(ic)),
                            }),
                    )
                    .child(if ren {
                        div()
                            .w_full()
                            .text_xs()
                            .text_center()
                            .child(TextInput::new(&self.rename_input_state).xsmall())
                            .into_any_element()
                    } else {
                        div()
                            .w_full()
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
                            if !d.selected_items.contains(&irc.path) {
                                d.selected_items.clear();
                                d.selected_items.insert(irc.path.clone());
                                cx.notify();
                            }
                            cx.stop_propagation();
                        }),
                    )
                    .on_drag_move(
                        cx.listener(move |d, _: &DragMoveEvent<DraggedFile>, _w, cx| {
                            d.hovered_drop_folder = if fld { Some(ihp.clone()) } else { None };
                            d.show_drop_hint = fld;
                            cx.notify();
                        }),
                    )
                    .context_menu(move |m, w, cx| {
                        context_menus::item_context_menu(ip.clone(), hc, cls)(m, w, cx)
                    }),
            )
    }

    pub fn render_list_view(
        &mut self,
        items: &[FileItem],
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let items: Vec<FileItem> = items.to_vec();
        let n = items.len();
        if n == 0 {
            return v_flex().flex_1().min_h_0().into_any_element();
        }
        let sizes = Rc::new(vec![size(px(0.0), px(40.0)); n]);
        let view = cx.entity().clone();
        let handle = self.list_scroll_handle.clone();
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
                            .map(|i| this.render_list_item(&items[i], w, cx).into_any_element())
                            .collect()
                    },
                )
                .track_scroll(&handle),
            )
            .into_any_element()
    }

    pub fn render_list_item(
        &mut self,
        item: &FileItem,
        _w: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let sel = self.selected_items.contains(&item.path);
        let ren = self.renaming_item.as_ref() == Some(&item.path);
        let icon = get_icon_for_file_type(item);
        let ic = get_icon_color_for_file_type(item, cx.theme(), &mut self.fs_metadata);
        let icl = item.clone();
        let idc = item.clone();
        let irc = item.clone();
        let ip = item.path.clone();
        let ihp = item.path.clone();
        let ent = cx.entity().clone();
        let hc = self.clipboard.is_some();
        let cls = item.is_class();
        let fld = item.is_folder;
        let paths = if sel {
            self.selected_items.iter().cloned().collect()
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
                e.bg(cx.theme().accent.opacity(0.1))
                    .border_color(cx.theme().accent.opacity(0.3))
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
                AssetPayload {
                    engine_path: icl.path.to_string_lossy().replace('\\', "/"),
                    name: icl.name.clone(),
                    kind: AssetKind::Blueprint,
                    extension: "class".to_string(),
                }
            } else {
                AssetPayload::from_path(&icl.path)
            };
            let ep = ap.clone();
            let e2 = ent.clone();
            row = row.on_drag(ap, move |d, _, _, cx| {
                e2.update(cx, |_, cx| {
                    cx.emit(ui_types_common::DragEvent::AssetDragStarted(
                        ep.clone().into(),
                    ))
                });
                cx.stop_propagation();
                cx.new(|_| d.clone())
            });
        }
        if fld {
            let (d1, d2, d3, d4) = (
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
                        cx.notify();
                    }),
                )
                .drag_over::<DraggedFile>(|s, _, _, cx| {
                    s.bg(cx.theme().accent.opacity(0.2))
                        .border_2()
                        .border_color(cx.theme().accent)
                })
                .drag_over::<AssetPayload>(|s, _, _, cx| {
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
                .on_drop(cx.listener(move |d, p: &AssetPayload, w, cx| {
                    cx.stop_propagation();
                    d.handle_drop_on_folder_new(
                        &d3,
                        &[std::path::PathBuf::from(&p.engine_path)],
                        w,
                        cx,
                    );
                }))
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
                .child(TextInput::new(&self.rename_input_state).w_full().xsmall())
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
                if !d.selected_items.contains(&irc.path) {
                    d.selected_items.clear();
                    d.selected_items.insert(irc.path.clone());
                    cx.notify();
                }
                cx.stop_propagation();
            }),
        )
        .on_drag_move(
            cx.listener(move |d, _: &DragMoveEvent<DraggedFile>, _w, cx| {
                d.hovered_drop_folder = if fld { Some(ihp.clone()) } else { None };
                d.show_drop_hint = fld;
                cx.notify();
            }),
        )
        .context_menu(move |m, w, cx| {
            context_menus::item_context_menu(ip.clone(), hc, cls)(m, w, cx)
        })
    }
}
