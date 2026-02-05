impl FileManagerDrawer {
    pub fn render_content(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        h_resizable("file-manager-resizable", self.resizable_state.clone())
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
        let file_types = self.registered_file_types.clone();

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
                        .flex_1()
                        .p_4()
                        .overflow_y_scroll()
                        .on_mouse_down(gpui::MouseButton::Left, cx.listener(|drawer, _event, _window, cx| {
                            // Commit rename if clicking on blank area
                            if drawer.renaming_item.is_some() {
                                drawer.commit_rename(cx);
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
                        content_div = content_div
                            .drag_over::<DraggedFile>(|style, _, _, cx| {
                                style
                                    .bg(cx.theme().accent.opacity(0.1))
                            })
                            .on_drop(cx.listener(move |drawer, drag: &DraggedFile, _window, cx| {
                                drawer.handle_drop_on_folder_new(&folder, &drag.paths, cx);
                            }));
                    }

                    content_div
                        .child(
                            match self.view_mode {
                                ViewMode::Grid => self.render_grid_view(&items, window, cx).into_any_element(),
                                ViewMode::List => self.render_list_view(&items, window, cx).into_any_element(),
                            }
                        )
                }
            )
    }
}
