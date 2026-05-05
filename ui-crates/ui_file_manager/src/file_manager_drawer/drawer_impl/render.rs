impl FileManagerDrawer {
    pub fn render_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_resizable("file-manager-resizable")
            .state(self.resizable_state.clone())
            .child(
                resizable_panel()
                    .child(self.render_folder_tree(window, cx))
                    .size(px(250.))
            )
            .child(
                resizable_panel()
                    .child(self.render_file_content(window, cx))
            )
    }

    pub fn render_file_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.get_filtered_items();
        let has_clipboard = self.clipboard.is_some();
        let selected_folder = self.selected_folder.clone();
        let selected_folder_for_drop = self.selected_folder.clone();
        let selected_folder_for_drag_move = self.selected_folder.clone();
        let file_types = self.registered_file_types.clone();
        let show_drop_hint = self.show_drop_hint;

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                // Combined toolbar with path and buttons
                self.render_combined_toolbar(&items, window, cx)
            )
            .child(
                {
                    let mut content_div = div()
                        .id("file-content-scroll")
                        .relative()
                        .flex_1()
                        .p_4()
                        .overflow_y_scroll()
                        .on_mouse_down(gpui::MouseButton::Left, cx.listener(|drawer, _event, _window, cx| {
                            // Commit rename if clicking on blank area
                            if drawer.renaming_item.is_some() {
                                drawer.commit_rename(cx);
                            }
                        }))
                        .on_mouse_move(cx.listener(|drawer, _event: &MouseMoveEvent, _window, _cx| {
                            // Only clear drop hint when no drag is active
                            if !_cx.has_active_drag() {
                                drawer.hovered_drop_folder = None;
                                drawer.show_drop_hint = false;
                            }
                        }))
                        .context_menu(move |menu, _window, _cx| {
                            // Show folder context menu for blank area
                            if let Some(path) = selected_folder.clone() {
                                context_menus::folder_context_menu(path, has_clipboard, file_types.clone())(menu, _window, _cx)
                            } else {
                                menu
                            }
                        });

                    // Add drop handler for blank area (drop into current folder)
                    if let Some(folder) = selected_folder_for_drop.clone() {
                        let selected_folder_for_drag_move_internal = selected_folder_for_drag_move.clone();
                        let selected_folder_for_drag_move_external = selected_folder_for_drag_move.clone();
                        let folder_for_internal_drop = folder.clone();
                        let folder_for_external_drop = folder.clone();

                        content_div = content_div
                            .on_drag_move(cx.listener(move |drawer, _event: &DragMoveEvent<DraggedFile>, _window, cx| {
                                drawer.hovered_drop_folder = selected_folder_for_drag_move_internal.clone();
                                drawer.show_drop_hint = true;
                                cx.notify();
                            }))
                            .on_drag_move(cx.listener(move |drawer, _event: &DragMoveEvent<ExternalPaths>, _window, cx| {
                                drawer.hovered_drop_folder = selected_folder_for_drag_move_external.clone();
                                drawer.show_drop_hint = true;
                                cx.notify();
                            }))
                            .drag_over::<DraggedFile>(|style, _, _, cx| {
                                style
                                    .bg(cx.theme().accent.opacity(0.12))
                                    .border_1()
                                    .border_color(cx.theme().accent.opacity(0.8))
                            })
                            .drag_over::<ExternalPaths>(|style, _, _, cx| {
                                style
                                    .bg(cx.theme().accent.opacity(0.12))
                                    .border_1()
                                    .border_color(cx.theme().accent.opacity(0.8))
                            })
                            .on_drop(cx.listener(move |drawer, drag: &DraggedFile, _window, cx| {
                                    eprintln!("[FILE_DROP] 🟡 on_drop::<DraggedFile> fired on content area");
                                // Right-pane background drop: always use current folder.
                                // Folder-specific drops are handled by folder item drop targets.
                                let target = folder_for_internal_drop.clone();
                                drawer.show_drop_hint = false;
                                drawer.hovered_drop_folder = None;
                                drawer.handle_drop_on_folder_new(&target, &drag.paths, cx);
                            }))
                            .on_drop(cx.listener(move |drawer, external: &ExternalPaths, _window, cx| {
                                    eprintln!("[FILE_DROP] 🔵 on_drop::<ExternalPaths> fired on content area, paths={:?}", external.paths());
                                // Right-pane background drop: always use current folder.
                                // Folder-specific drops are handled by folder item drop targets.
                                let target = folder_for_external_drop.clone();
                                drawer.show_drop_hint = false;
                                drawer.hovered_drop_folder = None;
                                drawer.handle_external_drop_on_folder(&target, external.paths(), cx);
                            }));
                    }

                    let content_div = content_div
                        .child(
                            match self.view_mode {
                                ViewMode::Grid => self.render_grid_view(&items, window, cx).into_any_element(),
                                ViewMode::List => self.render_list_view(&items, window, cx).into_any_element(),
                            }
                        );

                    if show_drop_hint {
                        content_div.child(
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
                                        .child("Release mouse to begin import")
                                )
                        )
                    } else {
                        content_div
                    }
                }
            )
    }
}
