//! Terrain Tool Mode
//!
//! Authoring mode for voxel terrain sculpting. Surfaces sculpt mode
//! (Raise/Lower/Flatten/Paint), brush radius, strength and falloff, and turns
//! pointer input into canonical `EditOp` stamps against the active planet
//! through [`TerrainEditApi`] (design doc §5.3/§5.4).
//!
//! # Pointer contract
//!
//! `Consumed` is returned only when the brush actually had terrain under it.
//! Clicking empty space (or clicking with no planet in the scene) falls
//! through to the ordinary pick/gizmo path, so the user can still select
//! objects and frame the camera without leaving the mode. `Hover` always falls
//! through — it only refreshes the brush ring, and the renderer still needs it
//! for its own hover feedback.

pub mod ray;
pub mod scene_planets;
pub mod sculpt;

use engine_backend::services::terrain_edit::{
    BrushCursorRequest, TerrainEditApi, TerrainHit, TerrainTarget as PlanetTarget,
};

use super::{
    BrushCursor, PointerKind, StatusReadout, ToolMode, ToolModeContext, ToolModeId,
    ToolPointerEvent, ToolPointerResult, ToolWidget,
};
use crate::level_editor::state::terrain::{SculptMode, TerrainTarget};

/// Brush ring colour per sculpt mode, so the mode is readable at a glance
/// without looking back at the toolbar.
const RAISE_COLOR: [f32; 4] = [0.35, 0.95, 0.45, 0.9];
const LOWER_COLOR: [f32; 4] = [0.95, 0.45, 0.35, 0.9];
const FLATTEN_COLOR: [f32; 4] = [0.45, 0.65, 0.95, 0.9];
const PAINT_COLOR: [f32; 4] = [0.95, 0.85, 0.35, 0.9];

/// Tool mode for voxel terrain editing and foliage painting.
#[derive(Clone, Default)]
pub struct TerrainMode {
    /// Hit point of the last committed stamp, for drag coalescing. Cleared at
    /// stroke boundaries so the first stamp of every stroke always commits.
    last_stamp_center_m: Option<[f32; 3]>,
    /// Latest brush ring, refreshed on every pointer event that hit terrain.
    cursor: Option<BrushCursor>,
    /// Surface normal at `cursor`, needed to orient the drawn ring. Not part
    /// of the shell-facing [`BrushCursor`], which is plane-agnostic.
    cursor_normal: [f32; 3],
    /// Monotonic stroke id handed to `TerrainDomain::begin_stroke`.
    next_stroke_id: u64,
}

impl TerrainMode {
    /// Ray-cast the pointer position against the active terrain.
    fn hit_at(
        &self,
        api: &TerrainEditApi,
        ctx: &ToolModeContext,
        event: &ToolPointerEvent,
    ) -> Option<TerrainHit> {
        let ray = ray::viewport_ray(ctx.camera, ctx.viewport, event.norm_x, event.norm_y)?;
        api.hit_terrain(ray)
    }

    /// Publish the brush ring for this hit, both to the mode (for
    /// [`ToolMode::brush_cursor`]) and to the renderer's debug-draw mailbox.
    fn update_cursor(&mut self, api: &TerrainEditApi, ctx: &ToolModeContext, hit: &TerrainHit) {
        let brush = &ctx.state.editor.terrain.sculpt;
        let radius_m = sculpt::effective_radius_m(brush);
        let color = mode_color(brush.mode);
        self.cursor = Some(BrushCursor {
            center: hit.position_m,
            radius: radius_m,
            color,
        });
        self.cursor_normal = hit.normal;
        api.set_brush_cursor(Some(BrushCursorRequest {
            center_m: hit.position_m,
            normal: hit.normal,
            radius_m,
            color,
        }));
    }

    fn clear_cursor(&mut self, api: Option<&TerrainEditApi>) {
        self.cursor = None;
        if let Some(api) = api {
            api.set_brush_cursor(None);
        }
    }

    /// Apply one brush stamp. Returns `true` when an op was actually
    /// committed — a coalesced-away drag returns `false` but is still
    /// `Consumed`, because the user *is* sculpting, just not far enough yet.
    fn stamp(&mut self, api: &TerrainEditApi, ctx: &mut ToolModeContext, hit: &TerrainHit) -> bool {
        let brush = ctx.state.editor.terrain.sculpt;
        let radius_m = sculpt::effective_radius_m(&brush);
        if !sculpt::should_stamp(self.last_stamp_center_m, hit.position_m, radius_m) {
            return false;
        }
        let Some(definition) = api.body_definition(hit.target) else {
            return false;
        };
        let anchor = ctx
            .state
            .editor
            .terrain
            .active_stroke
            .map(|stroke| stroke.start_height);
        let stamp = sculpt::build_stamp(hit, &definition, &brush, anchor);

        match api.apply_edit(hit.target, stamp.op) {
            Ok(committed) => {
                ctx.state.editor.terrain_undo.record_stamp(committed);
                self.last_stamp_center_m = Some(hit.position_m);
                true
            }
            Err(error) => {
                tracing::warn!(%error, "terrain sculpt stamp was rejected");
                false
            }
        }
    }

    /// Open a stroke: capture the undo anchor and the flatten reference
    /// altitude, and remember which planet is being edited.
    fn begin_stroke(&mut self, api: &TerrainEditApi, ctx: &mut ToolModeContext, hit: &TerrainHit) {
        let start_height = api
            .body_definition(hit.target)
            .map(|definition| sculpt::altitude_m(&definition, hit.position_m))
            .unwrap_or_default();

        // The undo anchor must exist before the first stamp lands, or the
        // stroke would be unrevertable.
        if !ctx
            .state
            .editor
            .terrain_undo
            .begin_stroke(api, hit.target)
        {
            tracing::warn!("terrain stroke could not capture an undo anchor; sculpting anyway");
        }

        self.next_stroke_id = self.next_stroke_id.wrapping_add(1);
        let stroke_id = self.next_stroke_id;
        ctx.state
            .editor
            .terrain
            .begin_stroke(stroke_id, start_height);
        ctx.state.editor.terrain.set_target(editor_target(hit.target));
        self.last_stamp_center_m = None;
    }

    /// Close a stroke and commit it to terrain undo history.
    fn end_stroke(&mut self, ctx: &mut ToolModeContext) {
        ctx.state.editor.terrain.end_stroke();
        ctx.state.editor.terrain_undo.end_stroke();
        self.last_stamp_center_m = None;
    }
}

fn mode_color(mode: SculptMode) -> [f32; 4] {
    match mode {
        SculptMode::Raise => RAISE_COLOR,
        SculptMode::Lower => LOWER_COLOR,
        SculptMode::Flatten => FLATTEN_COLOR,
        SculptMode::Paint => PAINT_COLOR,
    }
}

/// Engine-side target → the editor domain's display form.
fn editor_target(target: PlanetTarget) -> TerrainTarget {
    match target {
        PlanetTarget::Planet(_) => TerrainTarget::Planet(target.to_hex()),
        PlanetTarget::Volume(_) => TerrainTarget::Volume(target.to_hex()),
    }
}

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
        // Adopt whichever planet the runtime has, so the status bar is honest
        // before the user's first click.
        if let Some(api) = ctx.terrain {
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
        self.clear_cursor(ctx.terrain);
    }

    fn brush_cursor(&self, _ctx: &ToolModeContext) -> Option<BrushCursor> {
        self.cursor
    }

    fn toolbar_controls(&self, ctx: &ToolModeContext) -> Vec<ToolWidget> {
        let sculpt = &ctx.state.editor.terrain.sculpt;
        let selected_mode_str = match sculpt.mode {
            SculptMode::Raise => "raise",
            SculptMode::Lower => "lower",
            SculptMode::Flatten => "flatten",
            SculptMode::Paint => "paint",
        };

        vec![
            // Creating terrain comes before shaping it, so it leads.
            ToolWidget::Action {
                id: super::dispatcher::CREATE_FLAT_WORLD,
                label_key: "LevelEditor.Terrain.CreateFlatWorld",
            },
            ToolWidget::Divider,
            ToolWidget::Segmented {
                id: "sculpt_mode",
                options: vec![
                    ("LevelEditor.Terrain.Raise", "raise"),
                    ("LevelEditor.Terrain.Lower", "lower"),
                    ("LevelEditor.Terrain.Flatten", "flatten"),
                    ("LevelEditor.Terrain.Paint", "paint"),
                ],
                selected: selected_mode_str,
            },
            ToolWidget::Divider,
            ToolWidget::Slider {
                id: "radius",
                label_key: "LevelEditor.Terrain.Radius",
                value: sculpt.radius_m,
                min: 1.0,
                max: 64.0,
                step: 0.5,
            },
            ToolWidget::Slider {
                id: "strength",
                label_key: "LevelEditor.Terrain.Strength",
                value: sculpt.strength,
                min: 0.1,
                max: 10.0,
                step: 0.1,
            },
            ToolWidget::Slider {
                id: "falloff",
                label_key: "LevelEditor.Terrain.Falloff",
                value: sculpt.falloff,
                min: 0.0,
                max: 1.0,
                step: 0.05,
            },
        ]
    }

    fn status(&self, ctx: &ToolModeContext) -> Option<StatusReadout> {
        let sculpt = &ctx.state.editor.terrain.sculpt;
        let text = format!(
            "Radius: {:.1}m | Strength: {:.1}",
            sculpt.radius_m, sculpt.strength
        );
        // Read from the domain's target rather than from `ctx.terrain`: the
        // toolbar and status bar build a context without the seam (they only
        // need widget data), and a seam-derived readout would flicker between
        // "active" and "no runtime" depending on which caller rendered it.
        // `on_mode_entered`/`begin_stroke` keep the target current.
        let tooltip = match &ctx.state.editor.terrain.target {
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
                self.begin_stroke(api, ctx, &hit);
                self.stamp(api, ctx, &hit);
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
                self.stamp(api, ctx, &hit);
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
