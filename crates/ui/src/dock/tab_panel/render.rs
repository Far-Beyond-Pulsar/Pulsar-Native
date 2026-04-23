use super::*;

impl TabPanel {
    pub(crate) fn render_toolbar(
        &self,
        state: &TabState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.collapsed {
            return div();
        }

        let zoomed = self.zoomed;
        let view = cx.entity().clone();
        let zoomable_toolbar_visible = state.zoomable.map_or(false, |v| v.toolbar_visible());

        h_flex()
            .gap_1()
            .occlude()
            .when_some(self.toolbar_buttons(window, cx), |this, buttons| {
                this.children(
                    buttons
                        .into_iter()
                        .map(|btn| btn.xsmall().ghost().tab_stop(false)),
                )
            })
            .map(|this| {
                let value = if zoomed {
                    Some(("zoom-out", IconName::Minimize, t!("Dock.Zoom Out")))
                } else if zoomable_toolbar_visible {
                    Some(("zoom-in", IconName::Maximize, t!("Dock.Zoom In")))
                } else {
                    None
                };

                if let Some((id, icon, tooltip)) = value {
                    this.child(
                        Button::new(id)
                            .icon(icon)
                            .xsmall()
                            .ghost()
                            .tab_stop(false)
                            .tooltip_with_action(tooltip, &ToggleZoom, None)
                            .when(zoomed, |this| this.selected(true))
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.on_action_toggle_zoom(&ToggleZoom, window, cx)
                            })),
                    )
                } else {
                    this
                }
            })
            .child(
                Button::new("menu")
                    .icon(IconName::Ellipsis)
                    .xsmall()
                    .ghost()
                    .tab_stop(false)
                    .popup_menu({
                        let zoomable = state.zoomable.map_or(false, |v| v.menu_visible());
                        let closable = state.closable;

                        move |this, window, cx| {
                            view.read(cx)
                                .popup_menu(this, window, cx)
                                .separator()
                                .menu_with_disabled(
                                    if zoomed {
                                        t!("Dock.Zoom Out")
                                    } else {
                                        t!("Dock.Zoom In")
                                    },
                                    Box::new(ToggleZoom),
                                    !zoomable,
                                )
                                .when(closable, |this| {
                                    this.separator()
                                        .menu(t!("Dock.Close"), Box::new(ClosePanel))
                                })
                        }
                    })
                    .anchor(Corner::TopRight),
            )
    }

    pub(crate) fn render_dock_toggle_button(
        &self,
        placement: DockPlacement,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement> {
        if self.zoomed {
            return None;
        }

        let dock_area = self.dock_area.upgrade()?.read(cx);
        if !dock_area.toggle_button_visible {
            return None;
        }
        if !dock_area.is_dock_collapsible(placement, cx) {
            return None;
        }

        let view_entity_id = cx.entity().entity_id();
        let toggle_button_panels = dock_area.toggle_button_panels;

        if !match placement {
            DockPlacement::Left => {
                dock_area.left_dock.is_some() && toggle_button_panels.left == Some(view_entity_id)
            }
            DockPlacement::Right => {
                dock_area.right_dock.is_some() && toggle_button_panels.right == Some(view_entity_id)
            }
            DockPlacement::Bottom => {
                dock_area.bottom_dock.is_some()
                    && toggle_button_panels.bottom == Some(view_entity_id)
            }
            DockPlacement::Center => unreachable!(),
        } {
            return None;
        }

        let is_open = dock_area.is_dock_open(placement, cx);

        let icon = match placement {
            DockPlacement::Left => {
                if is_open {
                    IconName::PanelLeft
                } else {
                    IconName::PanelLeftOpen
                }
            }
            DockPlacement::Right => {
                if is_open {
                    IconName::PanelRight
                } else {
                    IconName::PanelRightOpen
                }
            }
            DockPlacement::Bottom => {
                if is_open {
                    IconName::PanelBottom
                } else {
                    IconName::PanelBottomOpen
                }
            }
            DockPlacement::Center => unreachable!(),
        };

        Some(
            Button::new(SharedString::from(format!("toggle-dock:{:?}", placement)))
                .icon(icon)
                .xsmall()
                .ghost()
                .tab_stop(false)
                .tooltip(match is_open {
                    true => t!("Dock.Collapse"),
                    false => t!("Dock.Expand"),
                })
                .on_click(cx.listener({
                    let dock_area = self.dock_area.clone();
                    move |_, _, window, cx| {
                        _ = dock_area.update(cx, |dock_area, cx| {
                            dock_area.toggle_dock(placement, window, cx);
                        });
                    }
                })),
        )
    }

    pub(crate) fn render_title_bar(
        &self,
        state: &TabState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let view = cx.entity().clone();

        let Some(dock_area) = self.dock_area.upgrade() else {
            return div().into_any_element();
        };
        let panel_style = dock_area.read(cx).panel_style;

        let left_dock_button = self.render_dock_toggle_button(DockPlacement::Left, window, cx);
        let bottom_dock_button = self.render_dock_toggle_button(DockPlacement::Bottom, window, cx);
        let right_dock_button = self.render_dock_toggle_button(DockPlacement::Right, window, cx);

        let is_bottom_dock = bottom_dock_button.is_some();

        if self.panels.len() == 1 && panel_style == PanelStyle::Default {
            let panel = self.panels.get(0).unwrap();

            if !panel.visible(cx) {
                return div().into_any_element();
            }

            let title_style = panel.title_style(cx);

            return h_flex()
                .justify_between()
                .line_height(rems(1.0))
                .h(px(30.))
                .py_2()
                .pl_3()
                .pr_2()
                .when(left_dock_button.is_some(), |this| this.pl_2())
                .when(right_dock_button.is_some(), |this| this.pr_2())
                .when_some(title_style, |this, theme| {
                    this.bg(theme.background).text_color(theme.foreground)
                })
                .when(title_style.is_none(), |this| this.bg(cx.theme().tab_bar))
                .when(
                    left_dock_button.is_some() || bottom_dock_button.is_some(),
                    |this| {
                        this.child(
                            h_flex()
                                .flex_shrink_0()
                                .mr_1()
                                .gap_1()
                                .children(left_dock_button)
                                .children(bottom_dock_button),
                        )
                    },
                )
                .child(
                    div()
                        .id("tab")
                        .flex_1()
                        .min_w_16()
                        .overflow_hidden()
                        .text_ellipsis()
                        .whitespace_nowrap()
                        .child(panel.title(window, cx))
                        .when(state.draggable, |this| {
                            let channel = state.channel;
                            this.on_drag(
                                DragPanel::new(panel.clone(), view.clone(), channel).with_index(0),
                                move |drag, position, _, cx| {
                                    let mut drag_with_pos = drag.clone();
                                    drag_with_pos.drag_start_position = Some(position);
                                    cx.stop_propagation();
                                    cx.new(|_| drag_with_pos)
                                },
                            )
                            .on_drag_move(cx.listener(
                                |this, event: &DragMoveEvent<DragPanel>, window, cx| {
                                    this.check_drag_outside_window(
                                        event.event.position,
                                        window,
                                        cx,
                                    );
                                },
                            ))
                        }),
                )
                .children(panel.title_suffix(window, cx))
                .child(
                    h_flex()
                        .flex_shrink_0()
                        .ml_1()
                        .gap_1()
                        .child(self.render_toolbar(&state, window, cx))
                        .children(right_dock_button),
                )
                .into_any_element();
        }

        let tabs_count = self.panels.len();

        // Shared state to track which tab was right-clicked
        let clicked_tab_index = Rc::new(RefCell::new(None::<usize>));

        let tab_bar = TabBar::new("tab-bar")
            .tab_item_top_offset(-px(1.))
            .track_scroll(&self.tab_bar_scroll_handle)
            .when(
                left_dock_button.is_some() || bottom_dock_button.is_some(),
                |this| {
                    this.prefix(
                        h_flex()
                            .items_center()
                            .top_0()
                            .right(-px(1.))
                            .border_r_1()
                            .border_b_1()
                            .h_full()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().tab_bar)
                            .px_2()
                            .children(left_dock_button)
                            .children(bottom_dock_button),
                    )
                },
            )
            .children(self.panels.iter().enumerate().filter_map(|(ix, panel)| {
                let mut active = state.active_panel.as_ref() == Some(panel);
                let droppable = self.collapsed;

                if !panel.visible(cx) {
                    return None;
                }

                if self.collapsed {
                    active = false;
                }

                Some({
                    let is_level_editor = panel.panel_name(cx) == "Level Editor";
                    let panel_for_menu = panel.clone();
                    let view_for_menu = view.clone();

                    let tab = Tab::empty()
                        .map(|this| {
                            if let Some(tab_name) = panel.tab_name(cx) {
                                this.child(tab_name)
                            } else {
                                this.child(panel.title(window, cx))
                            }
                        })
                        .selected(active)
                        .on_click(cx.listener({
                            let is_collapsed = self.collapsed;
                            let dock_area = self.dock_area.clone();
                            move |view, _, window, cx| {
                                view.set_active_tab(ix, window, cx);

                                if is_bottom_dock && is_collapsed {
                                    _ = dock_area.update(cx, |dock_area, cx| {
                                        dock_area.toggle_dock(DockPlacement::Bottom, window, cx);
                                    });
                                }
                            }
                        }))
                        .on_mouse_down(gpui::MouseButton::Right, cx.listener({
                            let clicked_index = clicked_tab_index.clone();
                            move |_view, _event: &gpui::MouseDownEvent, _window, cx| {
                                *clicked_index.borrow_mut() = Some(ix);
                            }
                        }))
                        .when(state.draggable, |this| {
                            let channel = state.channel;
                            this.on_drag(
                                DragPanel::new(panel.clone(), view.clone(), channel)
                                    .with_index(ix),
                                move |drag, position, _, cx| {
                                    let mut drag_with_pos = drag.clone();
                                    drag_with_pos.drag_start_position = Some(position);
                                    cx.stop_propagation();
                                    cx.new(|_| drag_with_pos)
                                },
                            )
                            .on_drag_move(cx.listener(|this, event: &DragMoveEvent<DragPanel>, window, cx| {
                                let is_outside = this.check_drag_outside_window(event.event.position, window, cx);

                                if is_outside {
                                    this.will_split_placement = None;
                                }
                            }))
                        })
                        .when(state.droppable, |this| {
                            let channel = state.channel;
                            let view = view.clone();
                            this.drag_over::<DragPanel>(move |this, drag, window, cx| {
                                if drag.channel == channel {
                                    view.update(cx, |v, cx| {
                                        v.in_valid_drag = true;
                                        cx.notify();
                                    });
                                    this.rounded_l_none()
                                        .border_l_2()
                                        .border_r_0()
                                        .border_color(cx.theme().drag_border)
                                } else {
                                    this
                                }
                            })
                            .on_drop(cx.listener(
                                move |this, drag: &DragPanel, window, cx| {
                                    this.will_split_placement = None;
                                    this.on_drop(drag, Some(ix), true, window, cx)
                                },
                            ))
                        })
                        .suffix(h_flex().gap_1().when(!is_level_editor, |this| {
                            let panel = panel_for_menu.clone();
                            let view = view_for_menu.clone();
                            let dock = self.dock_area.clone();

                            this.child(
                                Button::new(("move-to-window", ix))
                                    .icon(IconName::ExternalLink)
                                    .ghost()
                                    .xsmall()
                                    .tooltip("Move to New Window")
                                    .on_click(cx.listener(move |tab_panel, _, window, cx| {
                                        let panel_to_move = panel.clone();
                                        let dock_area = dock.clone();
                                        let mouse_pos = window.mouse_position();
                                        let panel_index = ix;

                                        tracing::trace!("[TAB_PANEL] Popout button clicked");
                                        tracing::trace!("[TAB_PANEL] TabPanel entity ID: {:?}", cx.entity_id());
                                        tracing::trace!("[TAB_PANEL] Panel index: {}", panel_index);
                                        tracing::trace!("[TAB_PANEL] Dock area: {:?}", dock_area);

                                        tracing::trace!("[TAB_PANEL] Emitting PanelEvent::MoveToNewWindow with source info");
                                        cx.emit(PanelEvent::MoveToNewWindow {
                                            panel: panel_to_move.clone(),
                                            position: mouse_pos,
                                            source_tab_panel: cx.entity().downgrade(),
                                            source_index: panel_index,
                                        });

                                        tracing::trace!("[TAB_PANEL] Detaching panel from current tab panel");
                                        tab_panel.detach_panel(panel_to_move, window, cx);
                                        tab_panel.remove_self_if_empty(window, cx);
                                        tracing::trace!("[TAB_PANEL] Panel detached");
                                    }))
                            )
                        }).when(!is_level_editor, |this| {
                            this.child(
                                Button::new(("close-tab", ix))
                                    .icon(IconName::Close)
                                    .ghost()
                                    .xsmall()
                                    .on_click(cx.listener({
                                        let panel = panel.clone();
                                        move |this, _, window, cx| {
                                            this.remove_panel(panel.clone(), window, cx);
                                        }
                                    }))
                            )
                        }).into_any_element());
                    tab
                })
            }))
            .last_empty_space(
                div()
                    .id("tab-bar-empty-space")
                    .h_full()
                    .flex_grow()
                    .min_w_16()
                    .when(state.droppable, |this| {
                        let channel = state.channel;
                        let view_entity = view.clone();
                        let view_for_drop = view.clone();
                        this.drag_over::<DragPanel>(move |this, drag, window, cx| {
                            if drag.channel == channel {
                                view_entity.update(cx, |v, cx| {
                                    v.in_valid_drag = true;
                                    cx.notify();
                                });
                                this.bg(cx.theme().drop_target)
                            } else {
                                this
                            }
                        })
                        .on_drop(cx.listener(
                            move |this, drag: &DragPanel, window, cx| {
                                this.will_split_placement = None;

                                let ix = if drag.tab_panel == view_for_drop {
                                    Some(tabs_count - 1)
                                } else {
                                    None
                                };

                                this.on_drop(drag, ix, false, window, cx)
                            },
                        ))
                    }),
            )
            .when(!self.collapsed, |this| {
                this.suffix(
                    h_flex()
                        .items_center()
                        .top_0()
                        .right_0()
                        .border_l_1()
                        .border_b_1()
                        .h_full()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().tab_bar)
                        .px_2()
                        .gap_1()
                        .children(
                            self.active_panel(cx)
                                .and_then(|panel| panel.title_suffix(window, cx)),
                        )
                        .child(self.render_toolbar(state, window, cx))
                        .when_some(right_dock_button, |this, btn| this.child(btn)),
                )
            });

        // Wrap TabBar with context menu overlay
        let view_for_menu = view.clone();
        let panels_for_menu: Vec<_> = self.panels.iter().cloned().collect();

        div()
            .id("tab-bar-container")
            .relative()
            .child(tab_bar)
            .child(
                div()
                    .id("tab-context-overlay")
                    .absolute()
                    .inset_0()
                    .context_menu(move |menu, _window, cx| {
                        let view = view_for_menu.clone();
                        let total_tabs = panels_for_menu.len();
                        let clicked_index_ref = clicked_tab_index.clone();
                        let tab_index = clicked_index_ref.borrow().unwrap_or(0);

                        let can_close = panels_for_menu
                            .get(tab_index)
                            .map(|p| p.closable(cx) && p.panel_name(cx) != "Level Editor")
                            .unwrap_or(false);

                        let mut result = menu;

                        if can_close {
                            let view = view.clone();
                            let clicked_idx = clicked_index_ref.clone();
                            let panels = panels_for_menu.clone();
                            result.menu_items.push(PopupMenuItem::Item {
                                icon: None,
                                label: "Close".into(),
                                disabled: false,
                                action: None,
                                is_link: false,
                                handler: Some(Rc::new(move |window, cx| {
                                    let idx = clicked_idx.borrow().unwrap_or(0);
                                    if let Some(panel) = panels.get(idx) {
                                        let _ = view.update(cx, |view, cx| {
                                            view.remove_panel(panel.clone(), window, cx);
                                        });
                                    }
                                })),
                            });
                        }

                        if can_close && total_tabs > 1 {
                            let view = view_for_menu.clone();
                            let clicked_idx = clicked_index_ref.clone();
                            result.menu_items.push(PopupMenuItem::Item {
                                icon: None,
                                label: "Close Others".into(),
                                disabled: false,
                                action: None,
                                is_link: false,
                                handler: Some(Rc::new(move |window, cx| {
                                    let idx = clicked_idx.borrow().unwrap_or(0);
                                    let _ = view.update(cx, |view, cx| {
                                        view.close_other_tabs(idx, window, cx);
                                    });
                                })),
                            });
                        }

                        if can_close && tab_index > 0 {
                            let view = view_for_menu.clone();
                            let clicked_idx = clicked_index_ref.clone();
                            result.menu_items.push(PopupMenuItem::Item {
                                icon: None,
                                label: "Close to the Left".into(),
                                disabled: false,
                                action: None,
                                is_link: false,
                                handler: Some(Rc::new(move |window, cx| {
                                    let idx = clicked_idx.borrow().unwrap_or(0);
                                    let _ = view.update(cx, |view, cx| {
                                        view.close_tabs_to_left(idx, window, cx);
                                    });
                                })),
                            });
                        }

                        if can_close && tab_index < total_tabs.saturating_sub(1) {
                            let view = view_for_menu.clone();
                            let clicked_idx = clicked_index_ref.clone();
                            result.menu_items.push(PopupMenuItem::Item {
                                icon: None,
                                label: "Close to the Right".into(),
                                disabled: false,
                                action: None,
                                is_link: false,
                                handler: Some(Rc::new(move |window, cx| {
                                    let idx = clicked_idx.borrow().unwrap_or(0);
                                    let _ = view.update(cx, |view, cx| {
                                        view.close_tabs_to_right(idx, window, cx);
                                    });
                                })),
                            });
                        }

                        if can_close {
                            result.menu_items.push(PopupMenuItem::Separator);
                        }

                        result.menu_items.push(PopupMenuItem::Item {
                            icon: None,
                            label: "Close All Saved".into(),
                            disabled: total_tabs == 0,
                            action: None,
                            is_link: false,
                            handler: Some(Rc::new(move |window, cx| {
                                let _ = view.update(cx, |view, cx| {
                                    view.close_all_saved_tabs(window, cx);
                                });
                            })),
                        });

                        result
                    }),
            )
            .into_any_element()
    }

    pub(crate) fn render_active_panel(
        &self,
        state: &TabState,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.collapsed {
            return Empty {}.into_any_element();
        }

        let Some(active_panel) = state.active_panel.as_ref() else {
            return Empty {}.into_any_element();
        };

        let is_render_in_tabs = self.panels.len() > 1 && self.inner_padding(cx);

        v_flex()
            .id("active-panel")
            .group("")
            .flex_1()
            .child(
                div()
                    .id("tab-content")
                    .overflow_y_scroll()
                    .overflow_x_hidden()
                    .flex_1()
                    .child(
                        active_panel
                            .view()
                            .cached(StyleRefinement::default().absolute().size_full()),
                    ),
            )
            .when(state.droppable, |this| {
                let channel = state.channel;
                let view = cx.entity().clone();
                this.on_drag_move(cx.listener(Self::on_panel_drag_move))
                    .child(
                        div()
                            .invisible()
                            .absolute()
                            .bg(cx.theme().drop_target)
                            .map(|this| match self.will_split_placement {
                                Some(placement) => {
                                    let size = relative(0.5);
                                    match placement {
                                        Placement::Left => this.left_0().top_0().bottom_0().w(size),
                                        Placement::Right => {
                                            this.right_0().top_0().bottom_0().w(size)
                                        }
                                        Placement::Top => this.top_0().left_0().right_0().h(size),
                                        Placement::Bottom => {
                                            this.bottom_0().left_0().right_0().h(size)
                                        }
                                    }
                                }
                                None => this.top_0().left_0().size_full(),
                            })
                            .drag_over::<DragPanel>(move |this, drag, _window, cx| {
                                if drag.channel == channel {
                                    view.update(cx, |v, cx| {
                                        v.in_valid_drag = true;
                                        cx.notify();
                                    });
                                    this.visible()
                                } else {
                                    this
                                }
                            })
                            .on_drop(cx.listener(|this, drag: &DragPanel, window, cx| {
                                this.on_drop(drag, None, true, window, cx)
                            })),
                    )
            })
            .into_any_element()
    }
}

impl Render for TabPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let focus_handle = self.focus_handle(cx);
        let active_panel = self.active_panel(cx);
        let mut state = TabState {
            closable: self.closable(cx),
            draggable: self.draggable(cx),
            droppable: self.droppable(cx),
            zoomable: self.zoomable(cx),
            active_panel,
            channel: self.channel,
        };

        // 1. When is the final panel in the dock, it will not able to close.
        // 2. When is in the Tiles, it will always able to close (by active panel state).
        if !state.draggable && !self.in_tiles {
            state.closable = false;
        }

        self.bind_actions(cx)
            .id("tab-panel")
            .track_focus(&focus_handle)
            .tab_group()
            .size_full()
            .overflow_hidden()
            // NO BACKGROUND - allow transparency for viewports
            .child(self.render_title_bar(&state, window, cx))
            .child(self.render_active_panel(&state, window, cx))
    }
}
