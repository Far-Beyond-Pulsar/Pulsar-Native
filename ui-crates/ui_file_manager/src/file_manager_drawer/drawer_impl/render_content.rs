impl FileManagerDrawer {
    pub fn render_grid_view(
        &mut self,
        items: &[FileItem],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let items: Vec<FileItem> = items.to_vec();
        let total_items = items.len();

        if total_items == 0 {
            return v_flex().flex_1().min_h_0().into_any_element();
        }

        let pane_width = self
            .resizable_state
            .read(cx)
            .sizes()
            .get(1)
            .copied()
            .map(f32::from)
            .unwrap_or_else(|| {
                let vp_width: f32 = window.viewport_size().width.into();
                (vp_width - 250.0).max(100.0)
            });

        const CARD_MIN_W: f32 = 100.0;
        const CARD_H: f32 = 110.0;
        const GAP: f32 = 12.0;
        const H_PADDING: f32 = 16.0;

        let available_w = (pane_width - H_PADDING).max(CARD_MIN_W);
        let cols = (((available_w + GAP) / (CARD_MIN_W + GAP)).floor() as usize).max(1);
        let card_w = ((available_w - (cols.saturating_sub(1)) as f32 * GAP) / cols as f32)
            .max(CARD_MIN_W);
        let total_rows = total_items.div_ceil(cols);
        let row_h = CARD_H + GAP;
        let item_sizes = Rc::new(vec![size(px(0.0), px(row_h)); total_rows]);
        let view = cx.entity().clone();
        let scroll_handle = self.grid_scroll_handle.clone();

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
                    item_sizes,
                    move |this, range, window, cx| {
                        range
                            .map(|row| {
                                let start = row * cols;
                                let end = (start + cols).min(total_items);
                                h_flex()
                                    .w_full()
                                    .gap(px(GAP))
                                    .py(px(GAP / 2.0))
                                    .items_start()
                                    .children(
                                        (0..cols)
                                            .map(|offset| {
                                                let item_index = start + offset;
                                                if item_index < end {
                                                    this.render_grid_item(&items[item_index], card_w, window, cx)
                                                        .into_any_element()
                                                } else {
                                                    div()
                                                        .w(px(card_w))
                                                        .h(px(CARD_H))
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
                .track_scroll(&scroll_handle),
            )
            .into_any_element()

    }

    pub fn render_grid_item(
        &mut self,
        item: &FileItem,
        card_width: f32,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_selected = self.selected_items.contains(&item.path);
        let is_renaming = self.renaming_item.as_ref() == Some(&item.path);
        let icon = get_icon_for_file_type(item);
        let icon_color = get_icon_color_for_file_type(item, cx.theme(), &mut self.fs_metadata);
        let item_clone = item.clone();
        let item_clone2 = item.clone();
        let item_clone3 = item.clone();
        let item_path = item.path.clone();
        let item_hover_path = item.path.clone();
        let drawer_entity = cx.entity().clone();
        let has_clipboard = self.clipboard.is_some();
        let is_class = item.is_class();
        let is_folder = item.is_folder;

        // Queue thumbnail and read the cached result (if ready).
        if !is_folder {
            self.ensure_thumbnail(&item.path, cx);
        }
        let thumb = self.thumbnails.get(&item.path).and_then(|t| t.clone());

        let drag_paths = if is_selected {
            self.selected_items.iter().cloned().collect()
        } else {
            vec![item.path.clone()]
        };

        let drag_data = DraggedFile {
            paths: drag_paths,
            is_folder: item.is_folder,
            drag_start_position: None,
        };

        let item_for_drop = item.clone();

        let mut content = v_flex()
            .id(SharedString::from(format!("grid-item-{}", item.name)))
            .w_full()
            .h_full()
            .p_3()
            .gap_2()
            .items_center()
            .justify_center();

        if is_folder {
            content = content.on_drag(drag_data, move |drag, position, _, cx| {
                let mut drag_with_pos = drag.clone();
                drag_with_pos.drag_start_position = Some(position);
                cx.stop_propagation();
                cx.new(|_| drag_with_pos)
            });
        } else {
            let asset_payload = if is_class {
                AssetPayload {
                    engine_path: item_clone.path.to_string_lossy().replace('\\', "/"),
                    name: item_clone.name.clone(),
                    kind: AssetKind::Blueprint,
                    extension: "class".to_string(),
                }
            } else {
                AssetPayload::from_path(&item_clone.path)
            };
            let payload_for_event = asset_payload.clone();
            let drawer_entity_for_drag = drawer_entity.clone();
            content = content.on_drag(asset_payload, move |drag, _, _, cx| {
                drawer_entity_for_drag.update(cx, |_, cx| {
                    cx.emit(ui_types_common::DragEvent::AssetDragStarted(
                        payload_for_event.clone().into(),
                    ));
                });
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            });
        }

        if is_folder {
            let folder_path_for_external_drag_move = item_for_drop.path.clone();
            let folder_path_for_internal_drop = item_for_drop.path.clone();
            let folder_path_for_external_drop = item_for_drop.path.clone();
            let folder_path_for_asset_drop = item_for_drop.path.clone();

            content = content
                .on_drag_move(cx.listener(move |drawer, _event: &DragMoveEvent<ExternalPaths>, _window, cx| {
                    drawer.hovered_drop_folder = Some(folder_path_for_external_drag_move.clone());
                    drawer.show_drop_hint = true;
                    cx.notify();
                }))
                .drag_over::<DraggedFile>(|style, _, _, cx| {
                    style
                        .bg(cx.theme().accent.opacity(0.2))
                        .border_2()
                        .border_color(cx.theme().accent)
                        .rounded_lg()
                })
                .drag_over::<AssetPayload>(|style, _, _, cx| {
                    style
                        .bg(cx.theme().accent.opacity(0.2))
                        .border_2()
                        .border_color(cx.theme().accent)
                        .rounded_lg()
                })
                .drag_over::<ExternalPaths>(|style, _, _, cx| {
                    style
                        .bg(cx.theme().accent.opacity(0.2))
                        .border_2()
                        .border_color(cx.theme().accent)
                        .rounded_lg()
                })
                .on_drop(cx.listener(move |drawer, drag: &DraggedFile, window, cx| {
                    cx.stop_propagation();
                    drawer.handle_drop_on_folder_new(&folder_path_for_internal_drop, &drag.paths, window, cx);
                }))
                .on_drop(cx.listener(move |drawer, payload: &AssetPayload, window, cx| {
                    cx.stop_propagation();
                    let path = std::path::PathBuf::from(&payload.engine_path);
                    drawer.handle_drop_on_folder_new(&folder_path_for_asset_drop, &[path], window, cx);
                }))
                .on_drop(cx.listener(move |drawer, external: &ExternalPaths, window, cx| {
                    cx.stop_propagation();
                    drawer.handle_external_drop_on_folder(&folder_path_for_external_drop, external.paths(), window, cx);
                }));
        }

        div()
            .w(px(card_width))
            .h(px(110.0))
            .rounded_lg()
            .border_1()
            .when(is_selected, |this| {
                this.border_color(cx.theme().accent)
                    .bg(cx.theme().accent.opacity(0.1))
                    .shadow_md()
            })
            .when(!is_selected, |this| {
                this.border_color(cx.theme().border.opacity(0.3))
                    .bg(cx.theme().sidebar.opacity(0.5))
            })
            .cursor_pointer()
            .hover(|style| {
                style
                    .bg(cx.theme().secondary.opacity(0.7))
                    .border_color(cx.theme().accent.opacity(0.7))
                    .shadow_lg()
            })
            .child(
                content
                    .child(
                        div()
                            .w(px(48.0))
                            .h(px(48.0))
                            .rounded_lg()
                            .bg(icon_color.opacity(0.15))
                            .border_1()
                            .border_color(icon_color.opacity(0.3))
                            .flex()
                            .items_center()
                            .justify_center()
                            .shadow_sm()
                            .overflow_hidden()
                            .map(|el| match thumb {
                                Some(render_img) => el.child(
                                    img(gpui::ImageSource::Render(render_img))
                                        .w(px(48.0))
                                        .h(px(48.0))
                                        .object_fit(gpui::ObjectFit::Cover),
                                ),
                                None => el.child(
                                    Icon::new(icon).size(px(24.0)).text_color(icon_color),
                                ),
                            }),
                    )
                    .child(if is_renaming {
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
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |drawer, event: &MouseDownEvent, _window: &mut Window, cx| {
                        if is_renaming {
                            cx.stop_propagation();
                        } else {
                            if drawer.renaming_item.is_some() {
                                drawer.commit_rename(cx);
                            }

                            if event.click_count == 2 {
                                drawer.handle_item_double_click(&item_clone3, cx);
                            } else {
                                drawer.handle_item_click(&item_clone, &event.modifiers, cx);
                            }
                        }
                    }))
                    .on_mouse_down(gpui::MouseButton::Right, cx.listener(move |drawer, _event: &MouseDownEvent, _window: &mut Window, cx| {
                        if !drawer.selected_items.contains(&item_clone2.path) {
                            drawer.selected_items.clear();
                            drawer.selected_items.insert(item_clone2.path.clone());
                            cx.notify();
                        }
                        cx.stop_propagation();
                    }))
                    .on_drag_move(cx.listener(move |drawer, _event: &DragMoveEvent<DraggedFile>, _window, cx| {
                        drawer.hovered_drop_folder = if is_folder {
                            Some(item_hover_path.clone())
                        } else {
                            None
                        };
                        drawer.show_drop_hint = is_folder;
                        cx.notify();
                    }))
                    .context_menu(move |menu, _window, _cx| {
                        context_menus::item_context_menu(item_path.clone(), has_clipboard, is_class)(menu, _window, _cx)
                    }),
            )
    }

    pub fn render_list_view(
        &mut self,
        items: &[FileItem],
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let items: Vec<FileItem> = items.to_vec();
        let item_count = items.len();

        if item_count == 0 {
            return v_flex().flex_1().min_h_0().into_any_element();
        }

        let item_sizes = Rc::new(vec![size(px(0.0), px(40.0)); item_count]);
        let view = cx.entity().clone();
        let scroll_handle = self.list_scroll_handle.clone();

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
                    item_sizes,
                    move |this, range, window, cx| {
                        range
                            .map(|i| {
                                this.render_list_item(&items[i], window, cx)
                                    .into_any_element()
                            })
                            .collect()
                    },
                )
                .track_scroll(&scroll_handle),
            )
            .into_any_element()
    }

    pub fn render_list_item(
        &mut self,
        item: &FileItem,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let is_selected = self.selected_items.contains(&item.path);
        let is_renaming = self.renaming_item.as_ref() == Some(&item.path);
        let icon = get_icon_for_file_type(item);
        let icon_color = get_icon_color_for_file_type(item, cx.theme(), &mut self.fs_metadata);
        let item_clone = item.clone();
        let item_clone2 = item.clone();
        let item_clone3 = item.clone();
        let item_path = item.path.clone();
        let item_hover_path = item.path.clone();
        let drawer_entity = cx.entity().clone();
        let has_clipboard = self.clipboard.is_some();
        let is_class = item.is_class();
        let is_folder = item.is_folder;

        let drag_paths = if is_selected {
            self.selected_items.iter().cloned().collect()
        } else {
            vec![item.path.clone()]
        };

        let drag_data = DraggedFile {
            paths: drag_paths,
            is_folder: item.is_folder,
            drag_start_position: None,
        };

        let item_for_drop = item.clone();

        let mut list_item = h_flex()
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
            .when(is_selected, |this| {
                this.bg(cx.theme().accent.opacity(0.1))
                    .border_color(cx.theme().accent.opacity(0.3))
                    .border_l_2()
                    .border_color(cx.theme().accent)
            })
            .when(!is_selected, |this| this.border_color(gpui::transparent_black()))
            .hover(|this| {
                this.bg(cx.theme().secondary.opacity(0.5))
                    .border_color(cx.theme().accent.opacity(0.2))
            });

        list_item = list_item.on_drag(drag_data, move |drag, position, _, cx| {
            let mut drag_with_pos = drag.clone();
            drag_with_pos.drag_start_position = Some(position);
            cx.stop_propagation();
            cx.new(|_| drag_with_pos)
        });

        if !is_folder {
            let asset_payload = if is_class {
                AssetPayload {
                    engine_path: item_clone2.path.to_string_lossy().replace('\\', "/"),
                    name: item_clone2.name.clone(),
                    kind: AssetKind::Blueprint,
                    extension: "class".to_string(),
                }
            } else {
                AssetPayload::from_path(&item_clone2.path)
            };
            let payload_for_event = asset_payload.clone();
            let drawer_entity_for_drag = drawer_entity.clone();
            list_item = list_item.on_drag(asset_payload, move |drag, _, _, cx| {
                drawer_entity_for_drag.update(cx, |_, cx| {
                    cx.emit(ui_types_common::DragEvent::AssetDragStarted(
                        payload_for_event.clone().into(),
                    ));
                });
                cx.stop_propagation();
                cx.new(|_| drag.clone())
            });
        }

        if is_folder {
            let folder_path_for_external_drag_move = item_for_drop.path.clone();
            let folder_path_for_internal_drop = item_for_drop.path.clone();
            let folder_path_for_external_drop = item_for_drop.path.clone();
            let folder_path_for_asset_drop = item_for_drop.path.clone();

            list_item = list_item
                .on_drag_move(cx.listener(move |drawer, _event: &DragMoveEvent<ExternalPaths>, _window, cx| {
                    drawer.hovered_drop_folder = Some(folder_path_for_external_drag_move.clone());
                    drawer.show_drop_hint = true;
                    cx.notify();
                }))
                .drag_over::<DraggedFile>(|style, _, _, cx| {
                    style
                        .bg(cx.theme().accent.opacity(0.2))
                        .border_2()
                        .border_color(cx.theme().accent)
                })
                .drag_over::<AssetPayload>(|style, _, _, cx| {
                    style
                        .bg(cx.theme().accent.opacity(0.2))
                        .border_2()
                        .border_color(cx.theme().accent)
                })
                .drag_over::<ExternalPaths>(|style, _, _, cx| {
                    style
                        .bg(cx.theme().accent.opacity(0.2))
                        .border_2()
                        .border_color(cx.theme().accent)
                })
                .on_drop(cx.listener(move |drawer, drag: &DraggedFile, window, cx| {
                    cx.stop_propagation();
                    drawer.handle_drop_on_folder_new(&folder_path_for_internal_drop, &drag.paths, window, cx);
                }))
                .on_drop(cx.listener(move |drawer, payload: &AssetPayload, window, cx| {
                    cx.stop_propagation();
                    let path = std::path::PathBuf::from(&payload.engine_path);
                    drawer.handle_drop_on_folder_new(&folder_path_for_asset_drop, &[path], window, cx);
                }))
                .on_drop(cx.listener(move |drawer, external: &ExternalPaths, window, cx| {
                    cx.stop_propagation();
                    drawer.handle_external_drop_on_folder(&folder_path_for_external_drop, external.paths(), window, cx);
                }));
        }

        list_item
            .child(
                div()
                    .w(px(24.0))
                    .h(px(24.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_sm()
                    .bg(icon_color.opacity(0.15))
                    .child(Icon::new(icon).size_4().text_color(icon_color)),
            )
            .child(if is_renaming {
                div()
                    .flex_1()
                    .text_sm()
                    .child(TextInput::new(&self.rename_input_state).w_full().xsmall())
                    .into_any_element()
            } else {
                div()
                    .flex_1()
                    .text_sm()
                    .font_weight(if is_selected {
                        gpui::FontWeight::SEMIBOLD
                    } else {
                        gpui::FontWeight::NORMAL
                    })
                    .text_color(cx.theme().foreground)
                    .child(item.name.clone())
                    .into_any_element()
            })
            .when(!item.is_folder, |this| {
                this.child(
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
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |drawer, event: &MouseDownEvent, _window: &mut Window, cx| {
                if is_renaming {
                    cx.stop_propagation();
                } else {
                    if drawer.renaming_item.is_some() {
                        drawer.commit_rename(cx);
                    }

                    if event.click_count == 2 {
                        drawer.handle_item_double_click(&item_clone3, cx);
                    } else {
                        drawer.handle_item_click(&item_clone, &event.modifiers, cx);
                    }
                }
            }))
            .on_mouse_down(gpui::MouseButton::Right, cx.listener(move |drawer, _event: &MouseDownEvent, _window: &mut Window, cx| {
                if !drawer.selected_items.contains(&item_clone2.path) {
                    drawer.selected_items.clear();
                    drawer.selected_items.insert(item_clone2.path.clone());
                    cx.notify();
                }
                cx.stop_propagation();
            }))
            .on_drag_move(cx.listener(move |drawer, _event: &DragMoveEvent<DraggedFile>, _window, cx| {
                drawer.hovered_drop_folder = if is_folder {
                    Some(item_hover_path.clone())
                } else {
                    None
                };
                drawer.show_drop_hint = is_folder;
                cx.notify();
            }))
            .context_menu(move |menu, _window, _cx| {
                context_menus::item_context_menu(item_path.clone(), has_clipboard, is_class)(menu, _window, _cx)
            })
    }
}
