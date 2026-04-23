use super::*;

impl TabPanel {
    /// Add panel to try to split
    pub fn add_panel_at(
        &mut self,
        panel: Arc<dyn PanelView>,
        placement: Placement,
        size: Option<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        cx.spawn_in(window, async move |view, cx| {
            cx.update(|window, cx| {
                view.update(cx, |view, cx| {
                    view.will_split_placement = Some(placement);
                    view.split_panel(panel, placement, size, window, cx)
                })
                .ok()
            })
            .ok()
        })
        .detach();
        cx.emit(PanelEvent::LayoutChanged);
        cx.notify();
    }

    /// Add panel with split placement
    pub(crate) fn split_panel(
        &self,
        panel: Arc<dyn PanelView>,
        placement: Placement,
        size: Option<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        tracing::debug!("SPLIT: split_panel called with placement {:?}", placement);
        let dock_area = self.dock_area.clone();
        let channel = self.channel;
        let panel_for_new_tab = panel.clone();
        let new_tab_panel = cx.new(|cx| Self::new(None, dock_area.clone(), channel, window, cx));
        new_tab_panel.update(cx, |view: &mut TabPanel, cx: &mut Context<TabPanel>| {
            view.add_panel(panel_for_new_tab, window, cx);
        });
        tracing::debug!("SPLIT: Created new tab panel");

        let stack_panel = match self.stack_panel.as_ref().and_then(|panel| panel.upgrade()) {
            Some(panel) => {
                tracing::debug!("SPLIT: Found parent StackPanel");
                panel
            }
            None => {
                tracing::debug!("SPLIT: No parent StackPanel - handling root-level split");
                tracing::debug!("SPLIT: Current TabPanel entity: {:?}", cx.entity());
                tracing::debug!("SPLIT: Current TabPanel has {} panels", self.panels.len());

                let axis = placement.axis();
                let current_tab_panel = cx.entity().clone();

                let new_stack_panel = cx.new(|cx| {
                    let mut stack = StackPanel::new(axis, window, cx);
                    stack.parent = None;
                    stack
                });

                let dock_area_clone = dock_area.clone();
                let new_stack_clone = new_stack_panel.clone();
                let new_tab_clone = new_tab_panel.clone();
                let current_tab_clone = current_tab_panel.clone();
                let panel_clone = panel.clone();

                window.defer(cx, move |window, cx| {
                    // Detach the panel from current tab if it exists there
                    _ = current_tab_clone.update(cx, |view, cx| {
                        if view.panels.iter().any(|p| Arc::ptr_eq(p, &panel_clone)) {
                            view.detach_panel(panel_clone.clone(), window, cx);
                            tracing::debug!("SPLIT: Detached panel from current tab");
                        }
                    });

                    // Update current TabPanel to reference the new StackPanel
                    _ = current_tab_clone.update(cx, |view, cx| {
                        view.stack_panel = Some(new_stack_clone.downgrade());
                        cx.notify();
                    });

                    // Update new TabPanel to reference the new StackPanel
                    _ = new_tab_clone.update(cx, |view, cx| {
                        view.stack_panel = Some(new_stack_clone.downgrade());
                        cx.notify();
                    });

                    // Add both TabPanels to the StackPanel in the correct order
                    _ = new_stack_clone.update(cx, |stack, cx| {
                        let new_tab_arc: Arc<dyn PanelView> = Arc::new(new_tab_clone.clone());
                        let current_tab_arc: Arc<dyn PanelView> = Arc::new(current_tab_clone.clone());
                        match placement {
                            Placement::Left | Placement::Top => {
                                stack.add_panel(new_tab_arc.clone(), size, dock_area_clone.clone(), window, cx);
                                stack.add_panel(current_tab_arc.clone(), None, dock_area_clone.clone(), window, cx);
                            }
                            Placement::Right | Placement::Bottom => {
                                stack.add_panel(current_tab_arc.clone(), None, dock_area_clone.clone(), window, cx);
                                stack.add_panel(new_tab_arc.clone(), size, dock_area_clone.clone(), window, cx);
                            }
                        }
                    });

                    // Update DockArea to use the new StackPanel in the correct location
                    _ = dock_area_clone.upgrade().map(|dock| {
                        dock.update(cx, |dock_area, cx| {
                            let current_tab_id = current_tab_clone.entity_id();
                            tracing::debug!("SPLIT: Looking for TabPanel {:?} in DockArea", current_tab_id);

                            let contains_tab_panel = |item: &DockItem| -> bool {
                                match item {
                                    DockItem::Tabs { view, .. } => view.entity_id() == current_tab_id,
                                    _ => false,
                                }
                            };

                            let new_dock_item = DockItem::Split {
                                axis,
                                items: vec![],
                                sizes: vec![None, None],
                                view: new_stack_clone.clone(),
                            };

                            let mut found = false;

                            if contains_tab_panel(&dock_area.items) {
                                tracing::debug!("SPLIT: Found in center (items), replacing it");
                                dock_area.items = new_dock_item.clone();
                                found = true;
                            }
                            else if let Some(left_dock) = &dock_area.left_dock {
                                if contains_tab_panel(&left_dock.read(cx).panel) {
                                    tracing::debug!("SPLIT: Found in left_dock, replacing its panel");
                                    let left_dock_entity = left_dock.clone();
                                    left_dock_entity.update(cx, |dock, cx| {
                                        dock.set_panel(new_dock_item.clone(), window, cx);
                                    });
                                    found = true;
                                }
                            }
                            if !found {
                                if let Some(right_dock) = &dock_area.right_dock {
                                    if contains_tab_panel(&right_dock.read(cx).panel) {
                                        tracing::debug!("SPLIT: Found in right_dock, replacing its panel");
                                        let right_dock_entity = right_dock.clone();
                                        right_dock_entity.update(cx, |dock, cx| {
                                            dock.set_panel(new_dock_item.clone(), window, cx);
                                        });
                                        found = true;
                                    }
                                }
                            }
                            if !found {
                                if let Some(bottom_dock) = &dock_area.bottom_dock {
                                    if contains_tab_panel(&bottom_dock.read(cx).panel) {
                                        tracing::debug!("SPLIT: Found in bottom_dock, replacing its panel");
                                        let bottom_dock_entity = bottom_dock.clone();
                                        bottom_dock_entity.update(cx, |dock, cx| {
                                            dock.set_panel(new_dock_item.clone(), window, cx);
                                        });
                                        found = true;
                                    }
                                }
                            }

                            if !found {
                                tracing::debug!("SPLIT: WARNING - Could not find TabPanel in any dock location!");
                            }

                            cx.notify();
                        })
                    });

                    tracing::debug!("SPLIT: Created root-level split with StackPanel");
                });

                return;
            }
        };

        let parent_axis = stack_panel.read(cx).axis;

        let ix = stack_panel
            .read(cx)
            .index_of_panel(Arc::new(cx.entity().clone()))
            .unwrap_or_default();

        if parent_axis.is_vertical() && placement.is_vertical() {
            stack_panel.update(cx, |view, cx| {
                view.insert_panel_at(
                    Arc::new(new_tab_panel),
                    ix,
                    placement,
                    size,
                    dock_area.clone(),
                    window,
                    cx,
                );
            });
        } else if parent_axis.is_horizontal() && placement.is_horizontal() {
            stack_panel.update(cx, |view, cx| {
                view.insert_panel_at(
                    Arc::new(new_tab_panel),
                    ix,
                    placement,
                    size,
                    dock_area.clone(),
                    window,
                    cx,
                );
            });
        } else {
            let tab_panel = cx.entity().clone();

            let new_stack_panel = if stack_panel.read(cx).panels_len() <= 1 {
                stack_panel.update(cx, |view, cx| {
                    view.remove_all_panels(window, cx);
                    view.set_axis(placement.axis(), window, cx);
                });
                stack_panel.clone()
            } else {
                cx.new(|cx| {
                    let mut panel = StackPanel::new(placement.axis(), window, cx);
                    panel.parent = Some(stack_panel.downgrade());
                    panel
                })
            };

            new_stack_panel.update(cx, |view, cx| match placement {
                Placement::Left | Placement::Top => {
                    view.add_panel(Arc::new(new_tab_panel), size, dock_area.clone(), window, cx);
                    view.add_panel(
                        Arc::new(tab_panel.clone()),
                        None,
                        dock_area.clone(),
                        window,
                        cx,
                    );
                }
                Placement::Right | Placement::Bottom => {
                    view.add_panel(
                        Arc::new(tab_panel.clone()),
                        None,
                        dock_area.clone(),
                        window,
                        cx,
                    );
                    view.add_panel(Arc::new(new_tab_panel), size, dock_area.clone(), window, cx);
                }
            });

            if stack_panel != new_stack_panel {
                stack_panel.update(cx, |view, cx| {
                    view.replace_panel(
                        Arc::new(tab_panel.clone()),
                        new_stack_panel.clone(),
                        window,
                        cx,
                    );
                });
            }

            cx.spawn_in(window, async move |_, cx| {
                cx.update(|window, cx| {
                    tab_panel.update(cx, |view, cx| view.remove_self_if_empty(window, cx))
                })
                .ok();
            })
            .detach();
        }

        cx.emit(PanelEvent::LayoutChanged);
    }
}
