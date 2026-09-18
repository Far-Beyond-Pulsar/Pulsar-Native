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

pub mod author;
pub mod layout;
pub mod panels;
pub mod ray;
pub mod scatter;
pub mod scene_planets;
pub mod sculpt;
mod trait_impl;

use engine_backend::services::terrain_edit::{
    BrushCursorRequest, TerrainEditApi, TerrainHit, TerrainTarget as PlanetTarget,
};

use gpui::AppContext;

use super::{
    BrushCursor, ModeLayout, ModePanelDescriptor, PointerKind, StatusReadout, ToolMode,
    ToolModeContext, ToolModeId, ToolPointerEvent, ToolPointerResult, ToolWidget,
};
use crate::level_editor::state::terrain::{SculptMode, TerrainTarget};

/// Brush ring colour per sculpt mode, so the mode is readable at a glance
/// without looking back at the toolbar.
const RAISE_COLOR: [f32; 4] = [0.35, 0.95, 0.45, 0.9];
const LOWER_COLOR: [f32; 4] = [0.95, 0.45, 0.35, 0.9];
const FLATTEN_COLOR: [f32; 4] = [0.45, 0.65, 0.95, 0.9];
const PAINT_COLOR: [f32; 4] = [0.95, 0.85, 0.35, 0.9];
/// Brush ring colour while the Foliage sub-mode is active, distinct from
/// every sculpt-mode colour (which are all green/red/blue/yellow) so the
/// toggle is readable at a glance.
const FOLIAGE_COLOR: [f32; 4] = [0.85, 0.35, 0.85, 0.9];

/// Tool mode for voxel terrain editing and foliage painting.
#[derive(Clone, Default)]
pub struct TerrainMode {
    /// Hit point of the last committed sculpt stamp, for drag coalescing.
    /// Cleared at stroke boundaries so the first stamp of every stroke
    /// always commits.
    last_stamp_center_m: Option<[f32; 3]>,
    /// Hit point of the last committed foliage stamp. Tracked separately
    /// from `last_stamp_center_m` even though the two brushes are mutually
    /// exclusive (the `paint_foliage` toggle), so switching sub-modes
    /// mid-drag can never let one brush's coalescing state leak into the
    /// other's.
    last_foliage_stamp_center_m: Option<[f32; 3]>,
    /// Latest brush ring, refreshed on every pointer event that hit terrain.
    cursor: Option<BrushCursor>,
    /// Surface normal at `cursor`, needed to orient the drawn ring. Not part
    /// of the shell-facing [`BrushCursor`], which is plane-agnostic.
    cursor_normal: [f32; 3],
    /// Monotonic stroke id handed to `TerrainDomain::begin_stroke`.
    next_stroke_id: u64,
    /// Stamps placed in the current foliage stroke; mixed into the scatter
    /// seed so consecutive stamps differ.
    foliage_stamp_index: u32,
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
        let terrain = &ctx.state.editor.terrain;
        let (radius_m, color) = if terrain.paint_foliage {
            (terrain.foliage.radius_m.max(1.0), FOLIAGE_COLOR)
        } else {
            (
                sculpt::effective_radius_m(&terrain.sculpt),
                mode_color(terrain.sculpt.mode),
            )
        };
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
    ///
    /// Shared by both brushes: `terrain_undo.end_stroke()` is a no-op when
    /// no anchor was ever opened (see [`Self::begin_foliage_stroke`]), so
    /// calling it unconditionally after a foliage stroke is harmless.
    fn end_stroke(&mut self, ctx: &mut ToolModeContext) {
        ctx.state.editor.terrain.end_stroke();
        ctx.state.editor.terrain_undo.end_stroke();
        self.last_stamp_center_m = None;
        self.last_foliage_stamp_center_m = None;
    }

    /// Open a foliage stroke: unlike [`Self::begin_stroke`], this never
    /// touches `TerrainUndoDomain` -- foliage painting mutates no voxels, so
    /// there is nothing for that history to snapshot. Each stamp is its own
    /// `SceneCommand::AddObject`, already undo-tracked by the scene's own
    /// snapshot undo (`execute_command`). See this module's `foliage`
    /// submodule doc and issue #713's "undo pairing" question for why the
    /// two undo systems are deliberately kept independent rather than
    /// merged into one stroke-level unit here.
    fn begin_foliage_stroke(&mut self, ctx: &mut ToolModeContext, hit: &TerrainHit) {
        self.next_stroke_id = self.next_stroke_id.wrapping_add(1);
        let stroke_id = self.next_stroke_id;
        ctx.state.editor.terrain.begin_stroke(stroke_id, 0.0);
        ctx.state.editor.terrain.set_target(editor_target(hit.target));
        self.last_foliage_stamp_center_m = None;
    }

    /// Apply one foliage brush stamp: coalesce like sculpt does (reusing
    /// `sculpt::should_stamp` rather than a second coalescing rule), then
    /// add a `FoliageComponent`-carrying scene object at the hit through the
    /// ordinary undo-tracked command path. Returns `true` only when an
    /// object was actually added.
    fn stamp_foliage(
        &mut self,
        api: &TerrainEditApi,
        ctx: &mut ToolModeContext,
        hit: &TerrainHit,
    ) -> bool {
        let radius_m = ctx.state.editor.terrain.foliage.radius_m.max(1.0);
        if !sculpt::should_stamp(self.last_foliage_stamp_center_m, hit.position_m, radius_m) {
            return false;
        }
        // Seed from the stroke and a per-stroke stamp counter so a stamp's
        // scatter is reproducible but no two stamps in a stroke repeat.
        self.foliage_stamp_index = self.foliage_stamp_index.wrapping_add(1);
        let seed = self.next_stroke_id.wrapping_mul(0x9E37_79B9) ^ u64::from(self.foliage_stamp_index);
        let placed = match ctx.state.editor.terrain.foliage_tool {
            crate::level_editor::state::terrain::FoliageTool::Paint => {
                author::stamp_foliage_sets(ctx.state, api, hit, seed)
            }
            crate::level_editor::state::terrain::FoliageTool::Erase => {
                let density = ctx.state.editor.terrain.foliage_erase_density.0;
                author::erase_foliage(ctx.state, hit, radius_m, density, seed)
            }
        };
        self.last_foliage_stamp_center_m = Some(hit.position_m);
        placed > 0
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
