impl FileManagerDrawer {
    pub fn render_combined_toolbar(&mut self, items: &[FileItem], window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let current_view_mode = self.view_mode;

        h_flex()
            .w_full()
            .h(px(56.))
            .px_4()
            .items_center()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .child(
                // Clickable breadcrumb path - takes remaining space with accent styling
                self.render_clickable_breadcrumb(&items, window, cx)
            )
            .child(
                // Item count badge
                div()
                    .px_2()
                    .py_1()
                    .rounded(px(6.))
                    .bg(cx.theme().accent.opacity(0.1))
                    .border_1()
                    .border_color(cx.theme().accent.opacity(0.3))
                    .text_xs()
                    .font_medium()
                    .text_color(cx.theme().accent)
                    .child(t!("FileManager.Items", count => items.len()).to_string())
            )
            // Divider
            .child(ui::divider::Divider::vertical().h(px(24.)))
            // View mode toggle group
            .child(
                ButtonGroup::new("view-mode-group")
                    .child(
                        Button::new("toggle-view")
                            .icon(IconName::LayoutDashboard)
                            .tooltip(t!("FileManager.GridView").to_string())
                            .selected(current_view_mode == ViewMode::Grid)
                    )
                    .child(
                        Button::new("toggle-list")
                            .icon(IconName::List)
                            .tooltip(t!("FileManager.ListView").to_string())
                            .selected(current_view_mode == ViewMode::List)
                    )
                    .ghost()
                    .on_click(cx.listener(|drawer, selected: &Vec<usize>, _window, cx| {
                        if selected.contains(&0) {
                            drawer.view_mode = ViewMode::Grid;
                        } else if selected.contains(&1) {
                            drawer.view_mode = ViewMode::List;
                        }
                        cx.notify();
                    }))
            )
            // Divider
            .child(ui::divider::Divider::vertical().h(px(24.)))
            // File operations group
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("new-file")
                            .icon(IconName::PagePlus)
                            .ghost()
                            .tooltip(t!("FileManager.NewFile").to_string())
                            .on_click(cx.listener(|drawer, _event, _window, cx| {
                                drawer.start_new_file(cx);
                            }))
                    )
                    .child(
                        Button::new("new-folder")
                            .icon(IconName::FolderPlus)
                            .ghost()
                            .tooltip(t!("FileManager.NewFolder").to_string())
                            .on_click(cx.listener(|drawer, _event, _window, cx| {
                                drawer.start_new_folder(cx);
                            }))
                    )
            )
            // Divider
            .child(ui::divider::Divider::vertical().h(px(24.)))
            // View options group
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("toggle-hidden")
                            .icon(if self.show_hidden_files { IconName::EyeOff } else { IconName::Eye })
                            .ghost()
                            .tooltip(if self.show_hidden_files {
                                t!("FileManager.HideHidden").to_string()
                            } else {
                                t!("FileManager.ShowHidden").to_string()
                            })
                            .on_click(cx.listener(|drawer, _event, _window, cx| {
                                drawer.show_hidden_files = !drawer.show_hidden_files;
                                cx.notify();
                            }))
                    )
                    .child(
                        Button::new("refresh")
                            .icon(IconName::Refresh)
                            .ghost()
                            .tooltip(t!("FileManager.Refresh").to_string())
                            .on_click(cx.listener(|drawer, _event, _window, cx| {
                                if let Some(ref path) = drawer.project_path {
                                    drawer.folder_tree = FolderNode::from_path(path);
                                }
                                cx.notify();
                            }))
                    )
            )
            // Divider
            .child(ui::divider::Divider::vertical().h(px(24.)))
            // Actions group
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("external")
                            .icon(IconName::ExternalLink)
                            .ghost()
                            .tooltip(t!("FileManager.OpenInFileManager").to_string())
                            .on_click(cx.listener(|drawer, _event, _window, _cx| {
                                if let Some(ref folder) = drawer.selected_folder {
                                    #[cfg(target_os = "windows")]
                                    let _ = std::process::Command::new("explorer")
                                        .arg(folder)
                                        .spawn();
                                    #[cfg(target_os = "macos")]
                                    let _ = std::process::Command::new("open")
                                        .arg(folder)
                                        .spawn();
                                    #[cfg(target_os = "linux")]
                                    let _ = std::process::Command::new("xdg-open")
                                        .arg(folder)
                                        .spawn();
                                }
                            }))
                    )
                    .child(
                        Button::new("popout")
                            .icon(IconName::ARrowUpRightSquare)
                            .ghost()
                            .tooltip("Pop Out to New Window")
                            .on_click(cx.listener(|drawer, _event, window: &mut Window, cx| {
                                let mouse_pos = window.mouse_position();
                                cx.emit(PopoutFileManagerEvent { position: mouse_pos });
                            }))
                    )
            )
    }

    pub fn render_clickable_breadcrumb(&mut self, _items: &[FileItem], _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut path_parts = Vec::new();

        // Get path components
        if let Some(ref selected) = self.selected_folder {
            if let Some(ref project) = self.project_path {
                if let Ok(relative) = selected.strip_prefix(project) {
                    let mut current = project.clone();
                    path_parts.push(("Project".to_string(), current.clone()));

                    for component in relative.components() {
                        if let Some(name) = component.as_os_str().to_str() {
                            current = current.join(name);
                            path_parts.push((name.to_string(), current.clone()));
                        }
                    }
                }
            }
        }

        if path_parts.is_empty() {
            path_parts.push(("Project".to_string(), self.project_path.clone().unwrap_or_default()));
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
            .child(
                Icon::new(IconName::Folder)
                    .size_4()
                    .text_color(cx.theme().accent)
            )
            .children(
                path_parts.into_iter().enumerate().flat_map(|(i, (name, path))| {
                    let mut elements: Vec<gpui::AnyElement> = Vec::new();

                    if i > 0 {
                        elements.push(
                            Icon::new(IconName::ChevronRight)
                                .size_3()
                                .text_color(cx.theme().muted_foreground)
                                .into_any_element()
                        );
                    }

                    let path_clone = path.clone();
                    let path_for_hover = path.clone();
                    elements.push(
                        div()
                            .text_sm()
                            .px_1()
                            .py_px()
                            .rounded(px(4.))
                            .text_color(cx.theme().foreground)
                            .font_medium()
                            .cursor_pointer()
                            .hover(|style| style.bg(cx.theme().accent.opacity(0.15)))
                            .child(name)
                            .on_mouse_down(gpui::MouseButton::Left, cx.listener(move |drawer, _event: &MouseDownEvent, _window: &mut Window, cx| {
                                drawer.selected_folder = Some(path_clone.clone());
                                cx.notify();
                            }))
                            // Add drag-over support for navigation
                            .drag_over::<DraggedFile>(|style, _, _, cx| {
                                style
                                    .bg(cx.theme().accent.opacity(0.3))
                                    .border_1()
                                    .border_color(cx.theme().accent)
                            })
                            .on_drop(cx.listener(move |drawer, drag: &DraggedFile, _window, cx| {
                                // Don't actually drop here, just trigger the navigation if timer completed
                                // The actual drop will be handled by the folder below
                                tracing::debug!("[FILE_MANAGER] Drop on breadcrumb - navigation already handled by hover");
                            }))
                            .on_mouse_move(cx.listener(move |drawer, _event: &MouseMoveEvent, _window, cx| {
                                // Check if we're currently dragging something
                                // Start hover timer when moving over breadcrumb during drag
                                drawer.start_breadcrumb_hover_timer(&path_for_hover, cx);
                            }))
                            .into_any_element()
                    );

                    elements
                })
            )
    }
}
