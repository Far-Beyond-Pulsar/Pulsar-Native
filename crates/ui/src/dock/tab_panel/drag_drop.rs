use super::*;
use crate::dock::tab_drag;

impl TabPanel {
    // ─────────────────────────────────────────────────────────────────────
    // Coordinate helpers
    // ─────────────────────────────────────────────────────────────────────

    /// `window.mouse_position()` is **window-local** logical pixels (0 = left edge
    /// of content area).  To place a new OS window we need **screen** logical pixels.
    /// This adds the window's outer origin, queried from winit directly.
    fn mouse_to_screen(local: Point<Pixels>, window: &Window) -> Point<Pixels> {
        let mut screen = local;
        window.with_winit_window(|w| {
            let scale = w.scale_factor() as f32;
            if let Ok(origin) = w.outer_position() {
                screen = Point {
                    x: px(origin.x as f32 / scale) + local.x,
                    y: px(origin.y as f32 / scale) + local.y,
                };
            }
        });
        screen
    }

    /// Content-area size in logical pixels from winit (never mixed with outer origin).
    fn content_size(window: &Window) -> gpui::Size<Pixels> {
        let mut sz = window.bounds().size;
        window.with_winit_window(|w| {
            let scale = w.scale_factor() as f32;
            let s = w.inner_size();
            sz = gpui::Size {
                width: px(s.width as f32 / scale),
                height: px(s.height as f32 / scale),
            };
        });
        sz
    }

    // ─────────────────────────────────────────────────────────────────────
    // Outside-window detection
    // ─────────────────────────────────────────────────────────────────────

    /// Returns true when the mouse cursor (in window-local logical pixels) is
    /// outside the content area.  Uses winit's `inner_size` directly — no
    /// coordinate-system mixing, no fudge margin.
    pub(crate) fn check_drag_outside_window(
        &mut self,
        mouse_local: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let sz = Self::content_size(window);
        let outside = mouse_local.x < px(0.0)
            || mouse_local.x > sz.width
            || mouse_local.y < px(0.0)
            || mouse_local.y > sz.height;

        if outside != self.dragging_outside_window {
            self.dragging_outside_window = outside;
            cx.notify();
        }
        outside
    }

    // ─────────────────────────────────────────────────────────────────────
    // Live extraction
    // ─────────────────────────────────────────────────────────────────────

    /// Called the first frame the drag cursor crosses outside the source window.
    ///
    /// 1. Detaches the panel.
    /// 2. Creates a new floating window positioned under the cursor in screen coords.
    /// 3. Stores the handle so `move_extracted_window` can reposition it every frame.
    ///
    /// `extraction_in_flight` is set synchronously before the defer so other
    /// TabPanel entities that receive the same on_drag_move don't fire again.
    pub(crate) fn begin_live_extraction(
        &mut self,
        drag: &DragPanel,
        mouse_local: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.extraction_in_flight = true;

        // Convert now while we still have `window`.
        let screen_pos = Self::mouse_to_screen(mouse_local, window);

        let panel = drag.panel.clone();
        let channel = self.channel;
        let source_self = cx.entity().clone();
        let source_tab = drag.tab_panel.clone();
        let is_same_tab = drag.tab_panel == cx.entity();
        let n_panels = self.panels.len();
        let in_tiles = self.in_tiles;

        window.defer(cx, move |window, cx| {
            if is_same_tab {
                let _ = source_self.update(cx, |v, cx| v.detach_panel(panel.clone(), window, cx));
            } else {
                let _ = source_tab.update(cx, |v, cx| {
                    v.detach_panel(panel.clone(), window, cx);
                    v.remove_self_if_empty(window, cx);
                });
            }

            let should_close_source = n_panels == 1 && !in_tiles;
            let new_handle =
                TabPanel::create_window_with_panel_returning_handle(panel, screen_pos, channel, cx);

            if let Some(handle) = new_handle {
                let _ = source_self.update(cx, |v, _| v.extracted_window = Some(handle));
            }

            if should_close_source {
                window.remove_window();
            } else {
                let _ = source_self.update(cx, |_, cx| cx.emit(PanelEvent::LayoutChanged));
            }
        });
    }

    /// Reposition the extracted floating window so its tab bar stays under the cursor.
    /// Called every drag-move frame while `extraction_in_flight` is true.
    pub(crate) fn move_extracted_window(
        &self,
        mouse_local: Point<Pixels>,
        window: &Window,
        cx: &mut App,
    ) {
        if let Some(handle) = self.extracted_window {
            let screen = Self::mouse_to_screen(mouse_local, window);
            let tab_bar_h = px(36.0);
            let target = Point {
                x: screen.x - px(120.0),
                y: screen.y - tab_bar_h / 2.0,
            };
            let _ = cx.update_window(handle, |_, win, _| {
                win.set_window_position(target);
            });
        }
    }

    /// Re-entry: close the extracted window and return the panel to this TabPanel.
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
                let _ = cx.update_window(handle, |_, win, _| win.remove_window());
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

        let mouse_local = window.mouse_position();
        let position = event.event.position; // element-relative, for split calc only
        let bounds = event.bounds;

        self.last_drag_screen_pos = Some(mouse_local);
        tab_drag::set_drag_screen_position(mouse_local, cx);

        if self.check_drag_outside_window(mouse_local, window, cx) {
            self.will_split_placement = None;
            return;
        }

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

    /// Create a floating window; return its handle.
    /// `screen_pos` must be in logical screen coordinates.
    pub(crate) fn create_window_with_panel_returning_handle(
        panel: Arc<dyn PanelView>,
        screen_pos: Point<Pixels>,
        source_channel: DockChannel,
        cx: &mut App,
    ) -> Option<AnyWindowHandle> {
        use crate::Root;

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
            window_min_size: Some(gpui::Size {
                width: px(400.),
                height: px(300.),
            }),
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
        tracing::debug!(
            "DROP: self_ch={:?} drag_ch={:?}",
            self.channel,
            drag.channel
        );

        self.in_valid_drag = false;

        if drag.channel != self.channel {
            return;
        }

        let panel = drag.panel.clone();
        let is_same_tab = drag.tab_panel == cx.entity();
        let will_split = self.will_split_placement;
        let was_outside = self.dragging_outside_window;
        let extracted = self.extraction_in_flight;
        let source_tab = drag.tab_panel.clone();
        let source_index = drag.source_index;
        let channel = self.channel;
        let target = cx.entity().clone();
        let panels_count = self.panels.len();
        let in_tiles = self.in_tiles;

        // Convert last-known local mouse pos to screen for window placement.
        let screen_pos = self
            .last_drag_screen_pos
            .map(|local| Self::mouse_to_screen(local, window))
            .or(drag.drag_start_position)
            .unwrap_or_default();

        self.last_drag_screen_pos = None;
        self.dragging_outside_window = false;
        self.will_split_placement = None;
        self.extraction_in_flight = false;
        self.extracted_window = None;

        window.defer(cx, move |window, cx| {
            // Live extraction already created the window.
            if extracted {
                let _ = target.update(cx, |_, cx| cx.emit(PanelEvent::LayoutChanged));
                return;
            }

            // Outside but extraction never fired (very fast flick).
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
                    if close_src {
                        window.remove_window();
                    }
                    let src_win = window.window_handle();
                    if let Some(t) = tab_drag::find_target_window(screen_pos, src_win, channel, cx)
                    {
                        tab_drag::deposit_panel_into_window(panel, &t, cx);
                    } else {
                        TabPanel::create_window_with_panel(panel, screen_pos, channel, cx);
                    }
                });
                let _ = target.update(cx, |_, cx| cx.emit(PanelEvent::LayoutChanged));
                return;
            }

            // Reorder within same TabPanel.
            if is_same_tab && ix.is_some() && will_split.is_none() {
                let tgt = ix.unwrap();
                let _ = target.update(cx, |v, cx| {
                    if source_index != tgt {
                        let p = v.panels.remove(source_index);
                        let ins = if tgt > source_index { tgt - 1 } else { tgt };
                        v.panels.insert(ins, p);
                        if v.active_ix == source_index {
                            v.active_ix = ins;
                        } else if source_index < v.active_ix && ins >= v.active_ix {
                            v.active_ix -= 1;
                        } else if source_index > v.active_ix && ins <= v.active_ix {
                            v.active_ix += 1;
                        }
                        cx.emit(PanelEvent::LayoutChanged);
                        cx.notify();
                    }
                });
                return;
            }

            if is_same_tab && ix.is_none() && will_split.is_none() {
                return;
            }

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
                    if is_same_tab {
                        v.detach_panel(panel.clone(), window, cx);
                    }
                    if let Some(i) = ix {
                        v.insert_panel_at(panel.clone(), i, window, cx);
                    } else {
                        v.add_panel_with_active(panel.clone(), active, window, cx);
                    }
                }
                v.remove_self_if_empty(window, cx);
                cx.emit(PanelEvent::LayoutChanged);
            });
        });
    }
}
