use super::*;
use crate::dock::tab_drag;

impl TabPanel {
    // ─────────────────────────────────────────────────────────────────────
    // Outside-window detection
    // ─────────────────────────────────────────────────────────────────────

    /// Update `dragging_outside_window` and repaint if it changed.
    /// Returns true when the pointer is outside the window bounds (± 20 px margin).
    pub(crate) fn check_drag_outside_window(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let b = window.bounds();
        let m = px(20.0);
        let outside = position.x < b.left() - m
            || position.x > b.right() + m
            || position.y < b.top() - m
            || position.y > b.bottom() + m;

        if outside != self.dragging_outside_window {
            self.dragging_outside_window = outside;
            cx.notify();
        }
        outside
    }

    // ─────────────────────────────────────────────────────────────────────
    // Live extraction
    // ─────────────────────────────────────────────────────────────────────

    /// Called the **first frame** the drag pointer crosses outside the source window.
    ///
    /// Behaviour:
    /// 1. Detach the panel from this TabPanel.
    /// 2. Create a new floating window at the cursor position.
    /// 3. Call `start_window_move()` on the new window — the OS then drives the
    ///    window move so the user is already "holding" it.
    /// 4. Store the new window handle in `extracted_window` so re-entry detection
    ///    can close it if the drag re-enters the source window.
    ///
    /// `extraction_in_flight` is set **synchronously** before the defer so that
    /// subsequent drag-move frames (other TabPanel entities receiving the same
    /// on_drag_move event) cannot fire a second extraction.
    pub(crate) fn begin_live_extraction(
        &mut self,
        drag: &DragPanel,
        screen_pos: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extraction_in_flight = true;

        let panel       = drag.panel.clone();
        let channel     = self.channel;
        let source_self = cx.entity().clone();
        let source_tab  = drag.tab_panel.clone();
        let is_same_tab = drag.tab_panel == cx.entity();
        let n_panels    = self.panels.len();
        let in_tiles    = self.in_tiles;

        window.defer(cx, move |window, cx| {
            // Detach panel from source.
            if is_same_tab {
                let _ = source_self.update(cx, |v, cx| v.detach_panel(panel.clone(), window, cx));
            } else {
                let _ = source_tab.update(cx, |v, cx| {
                    v.detach_panel(panel.clone(), window, cx);
                    v.remove_self_if_empty(window, cx);
                });
            }

            let should_close_source = n_panels == 1 && !in_tiles;

            // Create the floating window and record its handle.
            let new_handle =
                TabPanel::create_window_with_panel_returning_handle(panel, screen_pos, channel, cx);

            if let Some(handle) = new_handle {
                // Tell the OS to start dragging this window — user is already holding it.
                let _ = cx.update_window(handle, |_, new_win, _cx| {
                    new_win.start_window_move();
                });
                // Store handle for re-entry cancellation.
                let _ = source_self.update(cx, |v, _| v.extracted_window = Some(handle));
            }

            if should_close_source {
                window.remove_window();
            } else {
                let _ = source_self.update(cx, |_, cx| cx.emit(PanelEvent::LayoutChanged));
            }
        });
    }

    /// Called when the drag pointer **re-enters** the source window while
    /// `extraction_in_flight` is true.  Closes the floating window and returns
    /// the panel here so the drag can continue normally.
    pub(crate) fn cancel_live_extraction(
        &mut self,
        panel: Arc<dyn PanelView>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extraction_in_flight = false;
        self.dragging_outside_window = false;

        if let Some(handle) = self.extracted_window.take() {
            let self_entity = cx.entity().clone();
            window.defer(cx, move |window, cx| {
                // Close the floating window (panel stays alive via Arc).
                let _ = cx.update_window(handle, |_, win, _| win.remove_window());
                // Return panel to this TabPanel.
                let _ = self_entity.update(cx, |v, cx| {
                    v.add_panel(panel, window, cx);
                    cx.emit(PanelEvent::LayoutChanged);
                });
            });
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // Split-direction logic
    // ─────────────────────────────────────────────────────────────────────

    pub(crate) fn on_panel_drag_move(
        &mut self,
        event: &DragMoveEvent<DragPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.in_valid_drag {
            return;
        }

        let position = event.event.position;
        self.last_drag_screen_pos = Some(position);
        tab_drag::set_drag_screen_position(position, cx);

        if self.check_drag_outside_window(position, window, cx) {
            self.will_split_placement = None;
            return;
        }

        let bounds = event.bounds;
        if position.x < bounds.left() + bounds.size.width * 0.35 {
            self.will_split_placement = Some(Placement::Left);
        } else if position.x > bounds.left() + bounds.size.width * 0.65 {
            self.will_split_placement = Some(Placement::Right);
        } else if position.y < bounds.top() + bounds.size.height * 0.35 {
            self.will_split_placement = Some(Placement::Top);
        } else if position.y > bounds.top() + bounds.size.height * 0.65 {
            self.will_split_placement = Some(Placement::Bottom);
        } else {
            self.will_split_placement = None;
        }
        cx.notify();
    }

    // ─────────────────────────────────────────────────────────────────────
    // Window creation
    // ─────────────────────────────────────────────────────────────────────

    /// Create a floating window containing only `panel`; return its handle.
    pub(crate) fn create_window_with_panel_returning_handle(
        panel: Arc<dyn PanelView>,
        screen_pos: Point<Pixels>,
        source_channel: DockChannel,
        cx: &mut App,
    ) -> Option<AnyWindowHandle> {
        use crate::Root;

        // Tab bar is at y=0 in the new window (no separate title bar).
        // Place the window so the cursor sits in the middle of the ~36 px tab bar.
        let tab_bar_h = px(36.0);
        let opts = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                Point {
                    x: screen_pos.x - px(120.0),
                    y: screen_pos.y - tab_bar_h / 2.0,
                },
                size(px(800.), px(600.)),
            ))),
            titlebar: None,
            window_min_size: Some(gpui::Size { width: px(400.), height: px(300.) }),
            kind: WindowKind::Normal,
            window_decorations: Some(gpui::WindowDecorations::Client),
            ..Default::default()
        };

        window_manager::WindowManager::update_global(cx, |wm, cx| {
            wm.create_window(
                WindowRequest::DetachedPanel,
                opts,
                move |window: &mut gpui::Window, cx: &mut gpui::App| {
                    let dock = cx.new(|cx| {
                        DockArea::new_with_channel(
                            "detached-dock",
                            Some(1),
                            source_channel,
                            window,
                            cx,
                        )
                    });
                    let weak = dock.downgrade();
                    let tp = cx.new(|cx| {
                        let mut t = Self::new(None, weak.clone(), source_channel, window, cx);
                        t.closable = true;
                        t
                    });
                    tp.update(cx, |t, cx| t.add_panel(panel.clone(), window, cx));
                    dock.update(cx, |d, cx| {
                        d.set_center(
                            DockItem::Tabs {
                                view: tp.clone(),
                                active_ix: 0,
                                items: vec![panel.clone()],
                            },
                            window,
                            cx,
                        );
                    });
                    cx.new(|cx| Root::new(dock.into(), window, cx))
                },
                cx,
            )
        })
        .ok()
        .map(|(_, h)| h)
    }

    /// Convenience wrapper (pop-out button path, handle not needed).
    pub(crate) fn create_window_with_panel(
        panel: Arc<dyn PanelView>,
        screen_pos: Point<Pixels>,
        source_channel: DockChannel,
        cx: &mut App,
    ) {
        Self::create_window_with_panel_returning_handle(panel, screen_pos, source_channel, cx);
    }

    // ─────────────────────────────────────────────────────────────────────
    // Drop handler
    // ─────────────────────────────────────────────────────────────────────

    pub(crate) fn on_drop(
        &mut self,
        drag: &DragPanel,
        ix: Option<usize>,
        active: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::debug!("DROP: self_ch={:?} drag_ch={:?}", self.channel, drag.channel);

        self.in_valid_drag = false;

        if drag.channel != self.channel {
            tracing::debug!("DROP: rejected – channel mismatch");
            return;
        }

        let panel        = drag.panel.clone();
        let is_same_tab  = drag.tab_panel == cx.entity();
        let will_split   = self.will_split_placement;
        let was_outside  = self.dragging_outside_window;
        let extracted    = self.extraction_in_flight;
        let source_tab   = drag.tab_panel.clone();
        let source_index = drag.source_index;
        let channel      = self.channel;
        let target       = cx.entity().clone();
        let panels_count = self.panels.len();
        let in_tiles     = self.in_tiles;
        let screen_pos   = self.last_drag_screen_pos
            .or(drag.drag_start_position)
            .unwrap_or_default();

        // Clear all per-drag state.
        self.last_drag_screen_pos    = None;
        self.dragging_outside_window = false;
        self.will_split_placement    = None;
        self.extraction_in_flight    = false;
        self.extracted_window        = None;

        tracing::debug!(
            "DROP: same={} ix={:?} split={:?} outside={} extracted={}",
            is_same_tab, ix, will_split, was_outside, extracted
        );

        window.defer(cx, move |window, cx| {
            // ── Live extraction handled it: window already exists ─────────
            if extracted {
                // Panel is already in the floating window; source already detached.
                // Nothing to create — just emit layout changed.
                let _ = target.update(cx, |_, cx| cx.emit(PanelEvent::LayoutChanged));
                return;
            }

            // ── Outside but extraction never fired (very fast drag) ───────
            if was_outside {
                if is_same_tab {
                    let _ = target.update(cx, |v, cx| v.detach_panel(panel.clone(), window, cx));
                } else {
                    let _ = source_tab.update(cx, |v, cx| {
                        v.detach_panel(panel.clone(), window, cx);
                        v.remove_self_if_empty(window, cx);
                    });
                }

                let close_src = panels_count == 1 && !in_tiles;
                window.defer(cx, move |window, cx| {
                    if close_src { window.remove_window(); }
                    let src_win = window.window_handle();
                    if let Some(t) = tab_drag::find_target_window(screen_pos, src_win, channel, cx) {
                        tab_drag::deposit_panel_into_window(panel, &t, cx);
                    } else {
                        TabPanel::create_window_with_panel(panel, screen_pos, channel, cx);
                    }
                });
                let _ = target.update(cx, |_, cx| cx.emit(PanelEvent::LayoutChanged));
                return;
            }

            // ── Reorder within same TabPanel ──────────────────────────────
            if is_same_tab && ix.is_some() && will_split.is_none() {
                let tgt = ix.unwrap();
                let _ = target.update(cx, |v, cx| {
                    if source_index != tgt {
                        let p   = v.panels.remove(source_index);
                        let ins = if tgt > source_index { tgt - 1 } else { tgt };
                        v.panels.insert(ins, p);
                        if v.active_ix == source_index       { v.active_ix = ins; }
                        else if source_index < v.active_ix && ins >= v.active_ix { v.active_ix -= 1; }
                        else if source_index > v.active_ix && ins <= v.active_ix { v.active_ix += 1; }
                        cx.emit(PanelEvent::LayoutChanged);
                        cx.notify();
                    }
                });
                return;
            }

            if is_same_tab && ix.is_none() && will_split.is_none() { return; }

            if !is_same_tab {
                let _ = source_tab.update(cx, |v, cx| {
                    v.detach_panel(panel.clone(), window, cx);
                    v.remove_self_if_empty(window, cx);
                });
            }

            let _ = target.update(cx, |v, cx| {
                if let Some(placement) = will_split {
                    if is_same_tab {
                        v.split_panel(panel.clone(), placement, None, window, cx);
                        v.detach_panel(panel.clone(), window, cx);
                    } else {
                        v.split_panel(panel.clone(), placement, None, window, cx);
                    }
                } else {
                    if is_same_tab { v.detach_panel(panel.clone(), window, cx); }
                    if let Some(i) = ix { v.insert_panel_at(panel.clone(), i, window, cx); }
                    else                { v.add_panel_with_active(panel.clone(), active, window, cx); }
                }
                v.remove_self_if_empty(window, cx);
                cx.emit(PanelEvent::LayoutChanged);
            });
        });
    }
}
