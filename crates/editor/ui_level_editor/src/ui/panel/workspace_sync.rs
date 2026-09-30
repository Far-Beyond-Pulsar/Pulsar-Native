use super::*;

impl LevelEditorPanel {
    pub(super) fn initialize_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.workspace.is_some() {
            return;
        }

        let workspace = cx.new(|cx| {
            Workspace::new_with_channel(
                "level-editor-workspace",
                ui::dock::DockChannel(3),
                window,
                cx,
            )
        });

        let shared_state = self.shared_state.clone();
        let gpu = self.gpu_engine.clone();
        let viewport = self.viewport.clone();
        let render_enabled = self.render_enabled.clone();

        workspace.update(cx, |workspace, cx| {
            let dock_area = workspace.dock_area().downgrade();

            // Create viewport in center
            let viewport_panel_inner =
                ViewportPanel::new(viewport.clone(), render_enabled.clone(), window, cx);
            let viewport_panel = cx.new(|cx| {
                use crate::ViewportPanelWrapper;
                ViewportPanelWrapper::new(viewport_panel_inner, shared_state.clone(), gpu.clone(), cx)
            });

            // Create right dock panels
            let hierarchy_panel = cx.new(|cx| {
                use crate::HierarchyPanelWrapper;
                HierarchyPanelWrapper::new(shared_state.clone(), window, cx)
            });
            let properties_panel = cx.new(|cx| {
                use crate::PropertiesPanelWrapper;
                PropertiesPanelWrapper::new(shared_state.clone(), window, cx)
            });
            let world_settings_panel = cx.new(|cx| {
                use crate::WorldSettingsPanel;
                WorldSettingsPanel::new(shared_state.clone(), window, cx)
            });

            // NOTE: Panels are self-invalidating — each owns a frame pump
            // that watches the state it renders (`frame_pump`, signatures in
            // `workspace::panels` / `toolbar::view` / `status_bar_view`).
            // There is deliberately no observe/notify wiring between panels
            // or from this panel to them; forwarding notifications here used
            // to turn every scene edit into a whole-tree invalidation.

            // Bottom right: tabs for Properties and World Settings
            let bottom_tabs = DockItem::tabs(
                vec![
                    std::sync::Arc::new(properties_panel)
                        as std::sync::Arc<dyn ui::dock::PanelView>,
                    std::sync::Arc::new(world_settings_panel)
                        as std::sync::Arc<dyn ui::dock::PanelView>,
                ],
                Some(0),
                &dock_area,
                window,
                cx,
            );

            // Top right: hierarchy panel (as a single-tab TabPanel)
            let top_hierarchy = DockItem::tabs(
                vec![std::sync::Arc::new(hierarchy_panel)
                    as std::sync::Arc<dyn ui::dock::PanelView>],
                Some(0),
                &dock_area,
                window,
                cx,
            );

            // Compose right dock as a vertical split: top = hierarchy (25%), bottom = tabs (75%)
            // Hierarchy gets smaller fixed size, Properties/World gets larger
            let right = ui::dock::DockItem::split_with_sizes(
                gpui::Axis::Vertical,
                vec![top_hierarchy, bottom_tabs],
                vec![Some(px(150.0)), Some(px(550.0))], // 150px hierarchy, 550px for Properties/World
                &dock_area,
                window,
                cx,
            );

            // Set center and right dock only (no left dock, matching DAW approach).
            // The Game tab (Play In Editor, issue #243) is added dynamically
            // when the game starts and removed on stop — see `sync_game_tab`.
            let center_tabs = DockItem::tabs(
                vec![std::sync::Arc::new(viewport_panel)
                    as std::sync::Arc<dyn ui::dock::PanelView>],
                Some(0),
                &dock_area,
                window,
                cx,
            );
            let _ = dock_area.update(cx, |dock_area, cx| {
                dock_area.set_center(center_tabs, window, cx);
                dock_area.set_right_dock(right, Some(px(400.0)), true, window, cx);
            });
        });

        self.workspace = Some(workspace);
    }

    /// Open the Play-In-Editor Game tab when the game starts (build finished /
    /// running) — `add_panel` auto-activates it, so it autofocuses — and remove
    /// it on stop. Also surfaces build errors, since the tab may not exist when a
    /// build fails.
    ///
    /// Called from `render`, but guarded: this panel is invalidated several
    /// times a second (every viewport publish dirties the ancestor chain), and
    /// without the check each of those renders would take a write lock on the
    /// shared state for nothing. The lock below is now taken once per actual
    /// PiE state transition.
    pub(super) fn sync_game_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use crate::ui::viewport::game_viewport::GameViewport;

        // Cheap read-only guard first: no PiE transition since we last acted.
        let current = self.pie_signature();
        if Some(current) == self.applied_pie_signature {
            return;
        }

        let (should_open, error) = {
            let mut st = self.shared_state.write();
            // Open once the build hands off a game to run (or it is already
            // running); close when neither is true.
            let should_open = st.play.pie.active || st.play.pie.pending_start.is_some();
            let error = st.play.pie.last_error.take();
            (should_open, error)
        };

        if let Some(err) = error {
            window.push_notification(
                Notification::error(t!("Notification.Title.PlayInEditorBuildFailed").to_string())
                    .message(err),
                cx,
            );
        }

        let Some(workspace) = self.workspace.clone() else {
            return;
        };

        if should_open && self.game_panel.is_none() {
            let shared = self.shared_state.clone();
            let game_panel = cx.new(|cx| GameViewport::new(shared, cx));
            self.game_panel = Some(game_panel.clone());
            let panel_view: std::sync::Arc<dyn ui::dock::PanelView> =
                std::sync::Arc::new(game_panel);
            workspace.update(cx, |ws, cx| {
                let dock_area = ws.dock_area().clone();
                dock_area.update(cx, |da, cx| {
                    da.add_panel_to_center(panel_view, window, cx);
                });
            });
        } else if !should_open {
            if let Some(game_panel) = self.game_panel.take() {
                let panel_view: std::sync::Arc<dyn ui::dock::PanelView> =
                    std::sync::Arc::new(game_panel);
                workspace.update(cx, |ws, cx| {
                    let dock_area = ws.dock_area().clone();
                    dock_area.update(cx, |da, cx| {
                        da.items().remove_panel(panel_view, window, cx);
                    });
                });
            }
        }

        // Record AFTER acting: `last_error` was taken above, so the fresh
        // tuple differs from the pre-act snapshot whenever an error was
        // consumed, and storing the pre-take value would loop.
        self.applied_pie_signature = Some(self.pie_signature());
    }

    /// Reconcile the dock area against the active tool mode's [`ModeLayout`].
    ///
    /// Same shape as [`Self::sync_game_tab`]: a cheap signature guard first
    /// (render runs several times a second), then act only on a real change
    /// — i.e. an actual mode switch, a rare, deliberate user action.
    ///
    /// The shell maps the active mode to a named panel layout. Modes do not
    /// declare or construct their own dock panels.
    pub(super) fn sync_mode_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use ui::dock::{DockItem, DockPlacement};

        let current = self
            .shared_state
            .read()
            .editor
            .tool_mode_registry
            .selected_id();
        if Some(current) == self.applied_mode_layout {
            return;
        }

        let (layout, mode_id, mode) = {
            let state = self.shared_state.read();
            let layout = state.editor.tool_mode_registry.selected().layout();
            let mode = state.editor.tool_mode_registry.selected().clone_box();
            (layout, state.editor.tool_mode_registry.selected_id(), mode)
        };

        let mut panel_context = crate::tool_modes::ModePanelContext {
            state: self.shared_state.clone(),
            owner: cx.entity().downgrade(),
            window,
            cx,
        };
        let layout_panels = mode.build_panels(&mut panel_context);
        drop(panel_context);

        let Some(workspace) = self.workspace.clone() else {
            return;
        };

        workspace.update(cx, |ws, cx| {
            let dock_area = ws.dock_area().clone();
            let dock_area_weak = dock_area.downgrade();

            let right_open = dock_area.read(cx).is_dock_open(DockPlacement::Right, cx);
            if right_open != layout.show_right_dock {
                dock_area.update(cx, |da, cx| {
                    da.toggle_dock(DockPlacement::Right, window, cx);
                });
            }

            // The layout owns the left dock's panel set.
            if !layout_panels.is_empty() {
                let item = DockItem::tabs(layout_panels, Some(0), &dock_area_weak, window, cx);
                dock_area.update(cx, |da, cx| {
                    let width = if current == crate::tool_modes::ToolModeId::SPLINE { 360.0 } else { 280.0 };
                    da.set_left_dock(item, Some(px(width)), true, window, cx);
                });
            } else {
                let left_open = dock_area.read(cx).is_dock_open(DockPlacement::Left, cx);
                if left_open {
                    dock_area.update(cx, |da, cx| {
                        da.toggle_dock(DockPlacement::Left, window, cx);
                    });
                }
            }
        });

        self.applied_mode_layout = Some(current);
    }
}
