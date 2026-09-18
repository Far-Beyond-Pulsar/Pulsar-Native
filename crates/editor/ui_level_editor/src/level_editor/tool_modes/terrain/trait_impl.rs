use super::*;

impl ToolMode for TerrainMode {
    fn id(&self) -> ToolModeId {
        ToolModeId::TERRAIN
    }

    fn label_key(&self) -> &'static str {
        "LevelEditor.ToolMode.Terrain"
    }

    fn icon(&self) -> ui::IconName {
        ui::IconName::Globe
    }

    fn description_key(&self) -> &'static str {
        "LevelEditor.ToolMode.TerrainDesc"
    }

    fn on_mode_entered(&mut self, ctx: &mut ToolModeContext) {
        self.last_stamp_center_m = None;
        self.last_foliage_stamp_center_m = None;
        // Adopt whichever planet the runtime has, so the status bar is honest
        // before the user's first click.
        if let Some(api) = ctx.terrain {
            // Mode-owned panels only receive the shared state, so leave them
            // a handle to the terrain seam (Manage tab: list/create bodies).
            ctx.state.editor.terrain_api = Some(api.clone());
            if let Some(target) = api.default_target() {
                ctx.state.editor.terrain.set_target(editor_target(target));
            }
        }
    }

    fn on_mode_exited(&mut self, ctx: &mut ToolModeContext) {
        // Leaving mid-drag must not strand an open stroke: drop both the
        // domain's stroke marker and the undo anchor together.
        ctx.state.editor.terrain.end_stroke();
        ctx.state.editor.terrain_undo.abort_stroke();
        self.last_stamp_center_m = None;
        self.last_foliage_stamp_center_m = None;
        self.clear_cursor(ctx.terrain);
    }

    fn brush_cursor(&self, _ctx: &ToolModeContext) -> Option<BrushCursor> {
        self.cursor
    }

    fn layout(&self) -> ModeLayout {
        // Terrain's controls live in its own real panels (`contributes_panels`
        // below). The right dock stays: picking objects and inspecting World
        // Settings while sculpting is a normal part of the workflow.
        ModeLayout {
            show_right_dock: true,
        }
    }

    fn contributes_panels(&self) -> Vec<ModePanelDescriptor> {
        // Declarative half of the mode's own dock contributions — ids, tabs,
        // placements. The GPUI half lives in `super::panels`; see that file
        // and the design doc's §11 for why the two are split.
        layout::contributed_panels()
    }

    fn build_panel(
        &self,
        state: std::sync::Arc<parking_lot::RwLock<crate::level_editor::state::LevelEditorState>>,
        panel: &ModePanelDescriptor,
        window: &mut gpui::Window,
        cx: &mut gpui::App,
    ) -> Option<Box<dyn ui::dock::PanelView>> {
        match panel.id {
            layout::TERRAIN_PANEL => {
                let view = cx.new(|cx| panels::TerrainPanel::new(state.clone(), window, cx));
                Some(Box::new(view) as Box<dyn ui::dock::PanelView>)
            }
            layout::TERRAIN_FOLIAGE => {
                let view = cx.new(|cx| panels::FoliageSetsPanel::new(state.clone(), window, cx));
                Some(Box::new(view) as Box<dyn ui::dock::PanelView>)
            }
            _ => None,
        }
    }

    fn status(&self, ctx: &ToolModeContext) -> Option<StatusReadout> {
        let terrain = &ctx.state.editor.terrain;
        let text = if terrain.paint_foliage {
            format!(
                "Foliage | Radius: {:.1}m | Density: {:.0}% | {} meshes painting",
                terrain.foliage.radius_m,
                terrain.foliage_paint_density.0 * 100.0,
                terrain.foliage_sets.paintable_members().count()
            )
        } else {
            format!(
                "Radius: {:.1}m | Strength: {:.1}",
                terrain.sculpt.radius_m, terrain.sculpt.strength
            )
        };
        // Read from the domain's target rather than from `ctx.terrain`: the
        // toolbar and status bar build a context without the seam (they only
        // need widget data), and a seam-derived readout would flicker between
        // "active" and "no runtime" depending on which caller rendered it.
        // `on_mode_entered`/`begin_stroke` keep the target current.
        let tooltip = match &terrain.target {
            TerrainTarget::Planet(id) => Some(format!("Editing planet {id}")),
            TerrainTarget::Volume(id) => Some(format!("Editing volume {id}")),
            TerrainTarget::None => {
                Some("No terrain target — add a PlanetTerrainComponent to the scene".to_string())
            }
        };
        Some(StatusReadout { text, tooltip })
    }

    fn on_pointer(
        &mut self,
        event: &ToolPointerEvent,
        ctx: &mut ToolModeContext,
    ) -> ToolPointerResult {
        let Some(api) = ctx.terrain else {
            return ToolPointerResult::PassThrough;
        };

        match event.kind {
            PointerKind::Hover => {
                // Cursor feedback only; the renderer still wants this event.
                match self.hit_at(api, ctx, event) {
                    Some(hit) => self.update_cursor(api, ctx, &hit),
                    None => self.clear_cursor(Some(api)),
                }
                ToolPointerResult::PassThrough
            }

            PointerKind::Down => {
                if event.button != Some(gpui::MouseButton::Left) {
                    return ToolPointerResult::PassThrough;
                }
                let Some(hit) = self.hit_at(api, ctx, event) else {
                    // Nothing under the brush: let the click select objects.
                    self.clear_cursor(Some(api));
                    return ToolPointerResult::PassThrough;
                };
                self.update_cursor(api, ctx, &hit);
                if ctx.state.editor.terrain.paint_foliage {
                    self.begin_foliage_stroke(ctx, &hit);
                    self.stamp_foliage(api, ctx, &hit);
                } else {
                    self.begin_stroke(api, ctx, &hit);
                    self.stamp(api, ctx, &hit);
                }
                ToolPointerResult::Consumed
            }

            PointerKind::Drag => {
                if ctx.state.editor.terrain.active_stroke.is_none() {
                    return ToolPointerResult::PassThrough;
                }
                let Some(hit) = self.hit_at(api, ctx, event) else {
                    // Dragged off the planet: hold the stroke open (the user
                    // may drag back on) but stop drawing a ring nowhere.
                    self.clear_cursor(Some(api));
                    return ToolPointerResult::Consumed;
                };
                self.update_cursor(api, ctx, &hit);
                if ctx.state.editor.terrain.paint_foliage {
                    self.stamp_foliage(api, ctx, &hit);
                } else {
                    self.stamp(api, ctx, &hit);
                }
                ToolPointerResult::Consumed
            }

            PointerKind::Up => {
                if ctx.state.editor.terrain.active_stroke.is_none() {
                    return ToolPointerResult::PassThrough;
                }
                self.end_stroke(ctx);
                ToolPointerResult::Consumed
            }

            PointerKind::Scroll { .. } => ToolPointerResult::PassThrough,
        }
    }

    fn clone_box(&self) -> Box<dyn ToolMode> {
        Box::new(self.clone())
    }
}