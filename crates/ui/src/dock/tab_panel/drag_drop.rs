use super::*;

impl TabPanel {
    /// Check if the drag position is outside the window bounds
    pub(crate) fn check_drag_outside_window(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let window_bounds = window.bounds();

        // Add a small margin (20px) to make it easier to trigger
        let margin = px(20.0);
        let is_outside = position.x < window_bounds.left() - margin
            || position.x > window_bounds.right() + margin
            || position.y < window_bounds.top() - margin
            || position.y > window_bounds.bottom() + margin;

        if is_outside != self.dragging_outside_window {
            self.dragging_outside_window = is_outside;
            cx.notify();
        }

        is_outside
    }

    /// Create a simple new window with just the dragged panel
    ///
    /// NOTE: This creates a minimal window container. The panel itself maintains its
    /// references to shared services (like rust analyzer) from the main window,
    /// so there's no duplication of services.
    ///
    /// The window is positioned so the tab bar appears directly under the cursor,
    /// giving the impression that the tab "follows" the mouse during the drag.
    pub(crate) fn create_window_with_panel(
        panel: Arc<dyn PanelView>,
        position: Point<Pixels>,
        _dock_area: WeakEntity<DockArea>,
        cx: &mut App,
    ) {
        let window_size = size(px(800.), px(600.));

        // Approximate height of title bar in the new window
        let title_bar_height = px(36.0);

        // Position window so the cursor is over the tab area (just below title bar)
        let window_bounds = Bounds::new(
            Point {
                x: position.x - px(100.0),
                y: position.y - title_bar_height - px(4.0),
            },
            window_size,
        );

        let window_options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(window_bounds)),
            titlebar: None,
            window_min_size: Some(gpui::Size {
                width: px(400.),
                height: px(300.),
            }),
            kind: WindowKind::Normal,
            window_decorations: Some(gpui::WindowDecorations::Client),
            ..Default::default()
        };

        let _ = window_manager::WindowManager::update_global(cx, |wm, cx| {
            wm.create_window(
                WindowRequest::DetachedPanel,
                window_options,
                move |window: &mut gpui::Window, cx: &mut gpui::App| {
                    use crate::Root;

                    let new_dock_area =
                        cx.new(|cx| DockArea::new("detached-dock", Some(1), window, cx));
                    let weak_new_dock = new_dock_area.downgrade();

                    let new_tab_panel = cx.new(|cx| {
                        let channel = weak_new_dock
                            .upgrade()
                            .map(|d| d.read(cx).channel)
                            .unwrap_or_default();
                        let mut tab_panel =
                            Self::new(None, weak_new_dock.clone(), channel, window, cx);
                        tab_panel.closable = true;
                        tab_panel
                    });

                    new_tab_panel.update(cx, |view: &mut TabPanel, cx: &mut Context<TabPanel>| {
                        view.add_panel(panel.clone(), window, cx);
                    });

                    new_dock_area.update(cx, |dock: &mut DockArea, cx: &mut Context<DockArea>| {
                        let dock_item = DockItem::Tabs {
                            view: new_tab_panel.clone(),
                            active_ix: 0,
                            items: vec![panel.clone()],
                        };
                        dock.set_center(dock_item, window, cx);
                    });

                    cx.new(|cx| Root::new(new_dock_area.into(), window, cx))
                },
                cx,
            )
        });
    }

    /// Calculate the split direction based on the current mouse position
    pub(crate) fn on_panel_drag_move(
        &mut self,
        drag: &DragMoveEvent<DragPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Only process if we're in a valid same-channel drag
        if !self.in_valid_drag {
            return;
        }

        let bounds = drag.bounds;
        let position = drag.event.position;

        // Check if dragging outside window bounds for window extraction
        if self.check_drag_outside_window(position, window, cx) {
            self.will_split_placement = None;
            return;
        }

        // Check the mouse position to determine the split direction
        if position.x < bounds.left() + bounds.size.width * 0.35 {
            self.will_split_placement = Some(Placement::Left);
        } else if position.x > bounds.left() + bounds.size.width * 0.65 {
            self.will_split_placement = Some(Placement::Right);
        } else if position.y < bounds.top() + bounds.size.height * 0.35 {
            self.will_split_placement = Some(Placement::Top);
        } else if position.y > bounds.top() + bounds.size.height * 0.65 {
            self.will_split_placement = Some(Placement::Bottom);
        } else {
            // center to merge into the current tab
            self.will_split_placement = None;
        }
        cx.notify()
    }

    /// Handle the drop event when dragging a panel
    ///
    /// - `active` - When true, the panel will be active after the drop
    pub(crate) fn on_drop(
        &mut self,
        drag: &DragPanel,
        ix: Option<usize>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::debug!(
            "DROP: Panel being dropped on channel {:?}, drag from channel {:?}",
            self.channel,
            drag.channel
        );

        // Reset drag state
        self.in_valid_drag = false;

        // Verify that the drag is from the same channel
        if drag.channel != self.channel {
            tracing::debug!(
                "DROP: Rejected - drag from different channel (cross-channel drops not allowed)"
            );
            return;
        }

        // Clone all needed data BEFORE any entity access to avoid borrow conflicts
        let panel = drag.panel.clone();
        let is_same_tab = drag.tab_panel == cx.entity();
        let will_split = self.will_split_placement;
        let dragging_outside = self.dragging_outside_window;
        let drag_start_position = drag.drag_start_position;
        let source_panel = drag.tab_panel.clone();
        let source_index = drag.source_index;
        let dock_area = self.dock_area.clone();
        let target_entity = cx.entity().clone();
        let panels_count = self.panels.len();
        let in_tiles = self.in_tiles;
        tracing::debug!(
            "DROP: is_same_tab={}, ix={:?}, will_split={:?}",
            is_same_tab,
            ix,
            will_split
        );

        window.defer(cx, move |window, cx| {
            // Check if we should create a new window (dragged outside bounds)
            if dragging_outside {
                if let Some(start_pos) = drag_start_position {
                    let panel_to_extract = panel.clone();

                    // Detach the panel from the source
                    if is_same_tab {
                        _ = target_entity.update(cx, |view, cx| {
                            view.detach_panel(panel_to_extract.clone(), window, cx);
                        });
                    } else {
                        _ = source_panel.update(cx, |view, cx| {
                            view.detach_panel(panel_to_extract.clone(), window, cx);
                            view.remove_self_if_empty(window, cx);
                        });
                    }

                    let should_close_window = panels_count == 1 && !in_tiles;

                    window.defer(cx, move |window, cx| {
                        if should_close_window {
                            window.remove_window();
                        }

                        TabPanel::create_window_with_panel(
                            panel_to_extract,
                            start_pos,
                            dock_area,
                            cx,
                        );
                    });

                    _ = target_entity.update(cx, |view, cx| {
                        view.dragging_outside_window = false;
                        cx.emit(PanelEvent::LayoutChanged);
                    });
                    return;
                }
            }

            // Handle reordering within the same tab panel
            if is_same_tab && ix.is_some() && will_split.is_none() {
                let target_ix = ix.unwrap();

                _ = target_entity.update(cx, |view, cx| {
                    // Only reorder if different positions
                    if source_index != target_ix {
                        // Remove panel from old position
                        let panel = view.panels.remove(source_index);

                        // Calculate new insert position
                        let insert_ix = if target_ix > source_index {
                            target_ix - 1
                        } else {
                            target_ix
                        };

                        // Insert at new position
                        view.panels.insert(insert_ix, panel);

                        // Update active index if needed
                        if view.active_ix == source_index {
                            view.active_ix = insert_ix;
                        } else if source_index < view.active_ix && insert_ix >= view.active_ix {
                            view.active_ix -= 1;
                        } else if source_index > view.active_ix && insert_ix <= view.active_ix {
                            view.active_ix += 1;
                        }

                        cx.emit(PanelEvent::LayoutChanged);
                        cx.notify();
                    }
                });
                return;
            }

            // If target is same tab, not splitting, and no specific index, do nothing.
            if is_same_tab && ix.is_none() && will_split.is_none() {
                return;
            }

            // Detach from source (if different tab panel)
            if !is_same_tab {
                _ = source_panel.update(cx, |view, cx| {
                    view.detach_panel(panel.clone(), window, cx);
                    view.remove_self_if_empty(window, cx);
                });
            }

            // Insert into target (and detach if same tab, all in one update)
            tracing::debug!("DROP: Inserting panel into target, will_split={:?}, is_same_tab={}", will_split, is_same_tab);
            _ = target_entity.update(cx, |view, cx| {
                if let Some(placement) = will_split {
                    tracing::debug!("DROP: Splitting with placement {:?}", placement);
                    if is_same_tab {
                        tracing::debug!("DROP: Splitting from same tab - split_panel will handle panel management");
                        view.split_panel(panel.clone(), placement, None, window, cx);
                        view.detach_panel(panel.clone(), window, cx);
                    } else {
                        tracing::debug!("DROP: Splitting from different tab");
                        view.split_panel(panel.clone(), placement, None, window, cx);
                    }
                } else {
                    if is_same_tab {
                        tracing::debug!("DROP: Not splitting, detaching from same tab first");
                        view.detach_panel(panel.clone(), window, cx);
                    }

                    if let Some(ix) = ix {
                        tracing::debug!("DROP: Inserting at index {}", ix);
                        view.insert_panel_at(panel.clone(), ix, window, cx)
                    } else {
                        tracing::debug!("DROP: Adding panel with active={}", active);
                        view.add_panel_with_active(panel.clone(), active, window, cx)
                    }
                }

                tracing::debug!("DROP: Drop complete, checking if empty");
                view.remove_self_if_empty(window, cx);
                cx.emit(PanelEvent::LayoutChanged);
            });
        });
    }
}
