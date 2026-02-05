impl FileManagerDrawer {
    pub fn render_grid_view(&mut self, items: &[FileItem], window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w_full()
            .flex()
            .flex_wrap()
            .gap_3()
            .children(
                items.iter().map(|item| {
                    self.render_grid_item(item, window, cx)
                })
            )
    }

    pub fn render_grid_item(&mut self, item: &FileItem, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_selected = self.selected_items.contains(&item.path);
        let is_renaming = self.renaming_item.as_ref() == Some(&item.path);
        let path = item.path.clone();
        let icon = get_icon_for_file_type(&item);
        let icon_color = get_icon_color_for_file_type(&item, cx.theme(), &mut self.fs_metadata);
        let item_clone = item.clone();
        let item_clone2 = item.clone();
        let item_clone3 = item.clone(); // For double-click
        let item_path = item.path.clone();
        let has_clipboard = self.clipboard.is_some();
        let is_class = item.is_class();
        let is_folder = item.is_folder;

        // Create drag data for this item
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

        // Add drag functionality
        content = content.on_drag(drag_data, move |drag, position, _, cx| {
            let mut drag_with_pos = drag.clone();
            drag_with_pos.drag_start_position = Some(position);
            cx.stop_propagation();
            cx.new(|_| drag_with_pos)
        });

        // Add drop functionality if this is a folder
        if is_folder {
            content = content
                .drag_over::<DraggedFile>(|style, _, _, cx| {
                    style
                        .bg(cx.theme().accent.opacity(0.2))
                        .border_2()
                        .border_color(cx.theme().accent)
                        .rounded_lg()
                })
                .on_drop(cx.listener(move |drawer, drag: &DraggedFile, _window, cx| {
                    drawer.handle_drop_on_folder_new(&item_for_drop.path, &drag.paths, cx);
                }));
        }

        div()
            .w(px(100.0))
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
            .child(content
                    .child(
                        div()
                            .size(px(48.0))
                            .rounded_lg()
                            .bg(icon_color.opacity(0.15))
                            .border_1()
                            .border_color(icon_color.opacity(0.3))
                            .flex()
                            .items_center()
                            .justify_center()
                            .shadow_sm()
                            .child(
                                Icon::new(icon)
                                    .size(px(24.0))
                                    .text_color(icon_color)
                            )
                    )
                    .child(
                        if is_renaming {
                            div()
                                .w_full()
                                .text_xs()
                                .text_center()
                                .child(
                                    TextInput::new(&self.rename_input_state)
                                        .xsmall()
                                )
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
                        }
                    )
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |drawer, event: &MouseDownEvent, _window: &mut Window, cx| {
                        if is_renaming {
                            // Stop propagation when clicking the item being renamed
                            cx.stop_propagation();
                        } else {
                            // Commit any active rename before handling this item
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
                    .on_mouse_down(gpui::MouseButton::Right, cx.listener(move |drawer, event: &MouseDownEvent, _window: &mut Window, cx| {
                        // Select item on right-click if not already selected (without changing folder view)
                        if !drawer.selected_items.contains(&item_clone2.path) {
                            drawer.selected_items.clear();
                            drawer.selected_items.insert(item_clone2.path.clone());
                            // Don't change selected_folder on right-click to avoid navigating
                            cx.notify();
                        }
                        // Stop propagation so parent container's context menu doesn't show
                        cx.stop_propagation();
                    }))
                    .context_menu(move |menu, _window, _cx| {
                        // All items (files and folders) use item_context_menu
                        // Only blank area uses folder_context_menu with "New" options
                        context_menus::item_context_menu(item_path.clone(), has_clipboard, is_class)(menu, _window, _cx)
                    })
            )
    }

    pub fn render_list_view(&mut self, items: &[FileItem], window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_1()
            .children(items.iter().map(|item| {
                self.render_list_item(item, window, cx)
            }))
    }

    pub fn render_list_item(&mut self, item: &FileItem, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_selected = self.selected_items.contains(&item.path);
        let is_renaming = self.renaming_item.as_ref() == Some(&item.path);
        let icon = get_icon_for_file_type(&item);
        let icon_color = get_icon_color_for_file_type(&item, cx.theme(), &mut self.fs_metadata);
        let item_clone = item.clone();
        let item_clone2 = item.clone();
        let item_clone3 = item.clone(); // For double-click
        let item_path = item.path.clone();
        let has_clipboard = self.clipboard.is_some();
        let is_class = item.is_class();
        let is_folder = item.is_folder;

        // Create drag data for this item
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
            .when(!is_selected, |this| {
                this.border_color(gpui::transparent_black())
            })
            .hover(|this| {
                this.bg(cx.theme().secondary.opacity(0.5))
                    .border_color(cx.theme().accent.opacity(0.2))
            });

        // Add drag functionality
        list_item = list_item.on_drag(drag_data, move |drag, position, _, cx| {
            let mut drag_with_pos = drag.clone();
            drag_with_pos.drag_start_position = Some(position);
            cx.stop_propagation();
            cx.new(|_| drag_with_pos)
        });

        // Add drop functionality if this is a folder
        if is_folder {
            list_item = list_item
                .drag_over::<DraggedFile>(|style, _, _, cx| {
                    style
                        .bg(cx.theme().accent.opacity(0.2))
                        .border_2()
                        .border_color(cx.theme().accent)
                })
                .on_drop(cx.listener(move |drawer, drag: &DraggedFile, _window, cx| {
                    drawer.handle_drop_on_folder_new(&item_for_drop.path, &drag.paths, cx);
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
                    .child(
                        Icon::new(icon)
                            .size_4()
                            .text_color(icon_color)
                    )
            )
            .child(
                if is_renaming {
                    div()
                        .flex_1()
                        .text_sm()
                        .child(
                            TextInput::new(&self.rename_input_state)
                                .w_full()
                                .xsmall()
                        )
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
                }
            )
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
                        .child(format_file_size(item.size))
                )
            })
            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |drawer, event: &MouseDownEvent, _window: &mut Window, cx| {
                if is_renaming {
                    // Stop propagation when clicking the item being renamed
                    cx.stop_propagation();
                } else {
                    // Commit any active rename before handling this item
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
            .on_mouse_down(gpui::MouseButton::Right, cx.listener(move |drawer, event: &MouseDownEvent, _window: &mut Window, cx| {
                // Select item on right-click if not already selected (without changing folder view)
                if !drawer.selected_items.contains(&item_clone2.path) {
                    drawer.selected_items.clear();
                    drawer.selected_items.insert(item_clone2.path.clone());
                    // Don't change selected_folder on right-click to avoid navigating
                    cx.notify();
                }
                // Stop propagation so parent container's context menu doesn't show
                cx.stop_propagation();
            }))
            .context_menu(move |menu, _window, _cx| {
                // All items (files and folders) use item_context_menu
                // Only blank area uses folder_context_menu with "New" options
                context_menus::item_context_menu(item_path.clone(), has_clipboard, is_class)(menu, _window, _cx)
            })
    }
}
