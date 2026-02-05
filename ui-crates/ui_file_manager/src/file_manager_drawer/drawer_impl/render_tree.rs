impl FileManagerDrawer {
    pub fn render_folder_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                // Folder tree header with improved styling
                v_flex()
                    .w_full()
                    .gap_3()
                    .px_4()
                    .py_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().sidebar)
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(
                                        Icon::new(IconName::Folder)
                                            .size_4()
                                            .text_color(cx.theme().foreground)
                                    )
                                    .child(
                                        div()
                                            .text_base()
                                            .font_weight(gpui::FontWeight::BOLD)
                                            .text_color(cx.theme().foreground)
                                            .child(t!("FileManager.ProjectFiles").to_string())
                                    )
                            )
                            .child(
                                h_flex()
                                    .gap_1()
                                    .child(
                                        Button::new("expand-all")
                                            .icon(IconName::ChevronDown)
                                            .ghost()
                                            .xsmall()
                                            .tooltip(t!("FileManager.ExpandAll").to_string())
                                            .on_click(cx.listener(|drawer, _event, _window, cx| {
                                                if let Some(ref mut tree) = drawer.folder_tree {
                                                    tree.expand_all();
                                                    cx.notify();
                                                }
                                            }))
                                    )
                                    .child(
                                        Button::new("collapse-all")
                                            .icon(IconName::ChevronUp)
                                            .ghost()
                                            .xsmall()
                                            .tooltip(t!("FileManager.CollapseAll").to_string())
                                            .on_click(cx.listener(|drawer, _event, _window, cx| {
                                                if let Some(ref mut tree) = drawer.folder_tree {
                                                    tree.collapse_all();
                                                    cx.notify();
                                                }
                                            }))
                                    )
                            )
                    )
                    // Search box for folder tree
                    .child(
                        div()
                            .w_full()
                            .child(
                                TextInput::new(&self.folder_search_state)
                                    .w_full()
                                    .prefix(
                                        Icon::new(IconName::Search)
                                            .size_3()
                                            .text_color(cx.theme().muted_foreground)
                                    )
                            )
                    )
            )
            .child(
                // Folder tree content - SCROLLABLE with enhanced empty state
                div()
                    .flex_1()
                    .overflow_hidden()
                    .when_some(self.folder_tree.clone(), |this, tree| {
                        this.child(
                            v_flex()
                                .size_full()
                                .p_2()
                                .gap_px()
                                .scrollable(gpui::Axis::Vertical)
                                .child(self.render_folder_node(&tree, 0, window, cx))
                        )
                    })
                    .when(self.folder_tree.is_none(), |this| {
                        this.child(
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .p_4()
                                .child(
                                    v_flex()
                                        .gap_3()
                                        .items_center()
                                        .max_w(px(200.0))
                                        .px_4()
                                        .py_6()
                                        .rounded_lg()
                                        .bg(cx.theme().secondary.opacity(0.2))
                                        .border_1()
                                        .border_color(cx.theme().border.opacity(0.3))
                                        .child(
                                            div()
                                                .w(px(48.0))
                                                .h(px(48.0))
                                                .rounded_full()
                                                .bg(cx.theme().muted.opacity(0.3))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .child(
                                                    Icon::new(IconName::FolderOpen)
                                                        .size(px(24.0))
                                                        .text_color(cx.theme().muted_foreground)
                                                )
                                        )
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                                .text_color(cx.theme().foreground)
                                                .child("No Project")
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_center()
                                                .text_color(cx.theme().muted_foreground)
                                                .line_height(rems(1.4))
                                                .child("Open a project folder to see files")
                                        )
                                )
                        )
                    })
            )
    }

    pub fn render_folder_node(&mut self, node: &FolderNode, depth: usize, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_selected = self.selected_folder.as_ref() == Some(&node.path);
        let path = node.path.clone();
        let path_for_expand = path.clone();
        let expanded = node.expanded;
        let has_children = !node.children.is_empty();
        let folder_id = format!("folder-{}", path.display());
        let indent = px(depth as f32 * 20.0 + 4.0);
        let icon = if expanded { IconName::FolderOpen } else { IconName::Folder };
        let icon_color = ui::hierarchical_tree::tree_colors::FOLDER;

        let text_color = if is_selected {
            cx.theme().accent_foreground
        } else {
            cx.theme().foreground
        };

        let muted_color = if is_selected {
            cx.theme().accent_foreground.opacity(0.7)
        } else {
            cx.theme().muted_foreground
        };

        let mut item_div = h_flex()
            .id(SharedString::from(folder_id))
            .w_full()
            .items_center()
            .gap_1()
            .h_7()
            .pl(indent)
            .pr_2()
            .rounded(px(4.0))
            .cursor_pointer();

        if is_selected {
            item_div = item_div
                .bg(cx.theme().accent)
                .shadow_sm();
        } else {
            item_div = item_div.hover(|style| style.bg(cx.theme().muted.opacity(0.3)));
        }

        v_flex()
            .w_full()
            .child(
                item_div
                    .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |drawer, _event: &MouseDownEvent, _window, cx| {
                        drawer.handle_folder_select(path.clone(), cx);
                    }))
                    .child(
                        if has_children {
                            let path_clone = path_for_expand.clone();
                            div()
                                .w_4()
                                .h_4()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded(px(2.0))
                                .cursor_pointer()
                                .hover(|s| s.bg(cx.theme().muted.opacity(0.5)))
                                .child(
                                    Icon::new(if expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                                        .size(px(12.0))
                                        .text_color(muted_color)
                                )
                                .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |drawer, _event: &MouseDownEvent, _window, cx| {
                                    cx.stop_propagation();
                                    if let Some(ref mut tree) = drawer.folder_tree {
                                        tree.toggle_expanded(&path_clone);
                                    }
                                    cx.notify();
                                }))
                                .into_any_element()
                        } else {
                            div()
                                .w_4()
                                .into_any_element()
                        }
                    )
                    .child(
                        div()
                            .w_5()
                            .h_5()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(3.0))
                            .bg(icon_color.opacity(0.15))
                            .child(
                                Icon::new(icon)
                                    .size(px(14.0))
                                    .text_color(if is_selected { text_color } else { icon_color })
                            )
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(text_color)
                            .overflow_hidden()
                            .text_ellipsis()
                            .child(node.name.clone())
                    )
            )
            .children(
                if expanded {
                    node.children.iter().map(|child| {
                        self.render_folder_node(child, depth + 1, window, cx).into_any_element()
                    }).collect::<Vec<_>>()
                } else {
                    Vec::new()
                }
            )
    }
}
