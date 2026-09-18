//! Spline Tool Mode (Milestone 5 — extensibility demo, design doc §8.5)
//!
//! Proves that a brand-new [`ToolMode`] can be added purely through
//! [`super::ToolModeRegistry::register`] without touching
//! `ToolModeRegistry::builtin()`'s two existing entries, `ToolModeDispatcher`,
//! `ToolModeContext`, or any Milestone 1-4 file's logic (issue #714). See
//! `docs/adding-a-tool-mode.md` for the walkthrough this mode is the worked
//! example for.
//!
//! # What it does
//!
//! Lets the user build a simple in-memory polyline by clicking in the
//! viewport: left-click adds a point on the world `y = 0` ground plane,
//! Shift+left-click clears the path. Point count and total length are read
//! from [`SplineDomain`] by both `toolbar_controls` and `status`. No scene
//! objects, no persistence, no undo -- this is a proof of the registry
//! contract, not a production spline tool.
//!
//! # Why `on_pointer` alone, and not a toolbar `Action`/`Toggle`
//!
//! `ToolWidget::Action`/`Toggle`/`Segmented` clicks are written back to state
//! through `ToolModeDispatcher::dispatch_widget_edit[_with_terrain]`
//! (`tool_modes/dispatcher.rs`), which is a hardcoded switch over specific
//! widget ids belonging to `TerrainDomain` fields -- despite the design
//! doc's §4.1 framing of it as "the" write-back seam, it is not actually
//! generic over `ToolMode` implementors: a new mode's interactive widgets
//! only become functional if a maintainer adds a matching arm there. This
//! milestone's constraint (zero edits to `ToolModeDispatcher`) makes that
//! off-limits, and doing so would only be additive anyway -- not a generic
//! fix. So every real mutation this mode makes goes through `on_pointer`
//! instead, which dispatches generically already (`ToolModeDispatcher::
//! dispatch_pointer` calls `on_pointer` through the trait object with no
//! per-mode branching). `toolbar_controls` below returns widgets that are
//! genuinely dynamic but read-only by construction (a `Slider` with
//! `min == max == value` renders with both its inc/dec buttons disabled --
//! see `ui/toolbar/tool_mode_controls.rs` -- so nothing looks clickable and
//! silently does nothing). This is flagged in the Milestone 5 commit and
//! issue comment as a real gap in the design doc's extensibility story for
//! a future contributor to close.

use gpui::MouseButton;

use super::{
    PointerKind, StatusReadout, ToolMode, ToolModeContext, ToolModeId, ToolPointerEvent,
    ToolPointerResult, ToolWidget,
};

/// Vertical FOV / near / far the ground-plane ray uses.
///
/// Deliberately duplicated from `tool_modes::terrain::ray` rather than
/// imported from it: Spline is meant to demonstrate a mode that depends on
/// nothing but `ToolModeContext`/`CameraFrame`/`ViewportFrame`, not even
/// another mode's internal helpers, so its "modes are self-contained" story
/// stays honest. If a third mode needs the same math, that's the signal to
/// factor a shared `tool_modes::ray` helper -- out of scope here.
const VIEWPORT_FOV_RADIANS: f32 = std::f32::consts::FRAC_PI_4;
const NEAR_PLANE_M: f32 = 0.1;
const FAR_PLANE_M: f32 = 10_000.0;

/// Intersects the pointer ray with the world `y = 0` plane (the editor's
/// default ground plane). Returns `None` for a degenerate viewport, a ray
/// (near-)parallel to the plane, or a plane behind the camera -- all of
/// which fall through to ordinary pick/gizmo behavior rather than placing a
/// point nowhere sensible.
fn ground_plane_hit(
    camera: super::CameraFrame,
    viewport: super::ViewportFrame,
    norm_x: f32,
    norm_y: f32,
) -> Option<[f32; 3]> {
    use glam::{Mat4, Vec3};

    if !(viewport.width > 0.0 && viewport.height > 0.0) {
        return None;
    }
    let ndc_x = norm_x.clamp(0.0, 1.0) * 2.0 - 1.0;
    let ndc_y = 1.0 - norm_y.clamp(0.0, 1.0) * 2.0;

    let position = Vec3::from_array(camera.position);
    let (sin_yaw, cos_yaw) = camera.yaw.sin_cos();
    let (sin_pitch, cos_pitch) = camera.pitch.sin_cos();
    let forward = Vec3::new(sin_yaw * cos_pitch, sin_pitch, -cos_yaw * cos_pitch);

    let projection = Mat4::perspective_rh(
        VIEWPORT_FOV_RADIANS,
        viewport.width / viewport.height,
        NEAR_PLANE_M,
        FAR_PLANE_M,
    );
    let view = Mat4::look_at_rh(position, position + forward, Vec3::Y);
    let inverse = (projection * view).inverse();
    let near = inverse.project_point3(Vec3::new(ndc_x, ndc_y, 0.0));
    let far = inverse.project_point3(Vec3::new(ndc_x, ndc_y, 1.0));
    let direction = (far - near).normalize_or_zero();
    if direction == Vec3::ZERO {
        return None;
    }

    if direction.y.abs() < 1e-5 {
        return None;
    }
    let t = -near.y / direction.y;
    if t <= NEAR_PLANE_M {
        return None;
    }
    Some((near + direction * t).to_array())
}

/// Milestone 5's extensibility-demo tool mode. Holds no fields of its own --
/// all real state lives in [`SplineDomain`] on `EditorDomain` (see that
/// module's doc for why).
#[derive(Clone, Copy, Default)]
pub struct SplineMode;

impl ToolMode for SplineMode {
    fn id(&self) -> ToolModeId {
        ToolModeId::SPLINE
    }

    fn label_key(&self) -> &'static str {
        "LevelEditor.ToolMode.Spline"
    }

    fn icon(&self) -> ui::IconName {
        ui::IconName::MapPin
    }

    fn description_key(&self) -> &'static str {
        "LevelEditor.ToolMode.SplineDesc"
    }

    fn on_mode_entered(&mut self, _ctx: &mut ToolModeContext) {
        // The path deliberately survives a mode switch (like Terrain's brush
        // settings do) -- nothing to reset here.
    }

    fn on_mode_exited(&mut self, _ctx: &mut ToolModeContext) {}

    fn toolbar_controls(&self, ctx: &ToolModeContext) -> Vec<ToolWidget> {
        let spline = &ctx.state.editor.spline;
        let point_count = spline.points.len() as f32;
        let length_m = spline.total_length_m();
        vec![
            ToolWidget::Divider,
            // `min == max == value`: a read-only numeric chip, not an
            // editable control (see this module's top-level doc comment for
            // why these can't be made to actually write back).
            ToolWidget::Slider {
                id: "spline_point_count",
                label_key: "LevelEditor.Spline.PointCount",
                value: point_count,
                min: point_count,
                max: point_count,
                step: 1.0,
            },
            ToolWidget::Slider {
                id: "spline_length_m",
                label_key: "LevelEditor.Spline.Length",
                value: length_m,
                min: length_m,
                max: length_m,
                step: 0.1,
            },
        ]
    }

    fn status(&self, ctx: &ToolModeContext) -> Option<StatusReadout> {
        let spline = &ctx.state.editor.spline;
        let point_count = spline.points.len();
        let length_m = spline.total_length_m();
        let text = format!(
            "Spline: {point_count} pt{plural}, {length_m:.1}m — Click to add, Shift+Click to clear",
            plural = if point_count == 1 { "" } else { "s" },
        );
        Some(StatusReadout {
            text,
            tooltip: Some(
                "Left-click: add point to the in-memory polyline · Shift+Left-click: clear it"
                    .to_string(),
            ),
        })
    }

    fn on_pointer(
        &mut self,
        event: &ToolPointerEvent,
        ctx: &mut ToolModeContext,
    ) -> ToolPointerResult {
        if event.kind != PointerKind::Down || event.button != Some(MouseButton::Left) {
            return ToolPointerResult::PassThrough;
        }

        if event.holding_mods.shift {
            if ctx.state.editor.spline.points.is_empty() {
                // Nothing to clear -- let the click behave normally rather
                // than eating a plain select click for no reason.
                return ToolPointerResult::PassThrough;
            }
            ctx.state.editor.spline.clear();
            return ToolPointerResult::Consumed;
        }

        let Some(point) = ground_plane_hit(ctx.camera, ctx.viewport, event.norm_x, event.norm_y)
        else {
            // Camera can't see the ground plane from here: let the click
            // fall through to ordinary pick/gizmo behavior, same contract
            // `TerrainMode::on_pointer` uses for "nothing under the brush".
            return ToolPointerResult::PassThrough;
        };
        ctx.state.editor.spline.push_point(point);
        ToolPointerResult::Consumed
    }

    fn clone_box(&self) -> Box<dyn ToolMode> {
        Box::new(*self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level_editor::state::spline::SplineDomain;
    use crate::level_editor::tool_modes::{CameraFrame, ViewportFrame};

    fn viewport() -> ViewportFrame {
        ViewportFrame {
            width: 1920.0,
            height: 1080.0,
        }
    }

    #[test]
    fn a_camera_looking_straight_down_hits_the_ground_plane_below_it() {
        let camera = CameraFrame {
            position: [0.0, 10.0, 0.0],
            yaw: 0.0,
            pitch: -std::f32::consts::FRAC_PI_2,
            fov: 60.0,
        };
        let hit = ground_plane_hit(camera, viewport(), 0.5, 0.5).expect("should hit the ground");
        assert!(hit[1].abs() < 1e-3, "hit should be on y=0, got {hit:?}");
    }

    #[test]
    fn a_camera_looking_at_the_horizon_never_hits_the_ground_plane() {
        let camera = CameraFrame {
            position: [0.0, 10.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            fov: 60.0,
        };
        assert!(ground_plane_hit(camera, viewport(), 0.5, 0.5).is_none());
    }

    #[test]
    fn a_degenerate_viewport_yields_no_hit() {
        let camera = CameraFrame::default();
        assert!(ground_plane_hit(
            camera,
            ViewportFrame {
                width: 0.0,
                height: 0.0
            },
            0.5,
            0.5
        )
        .is_none());
    }

    #[test]
    fn total_length_of_a_two_point_path_is_the_straight_line_distance() {
        let mut domain = SplineDomain::default();
        domain.push_point([0.0, 0.0, 0.0]);
        domain.push_point([3.0, 0.0, 4.0]);
        assert!((domain.total_length_m() - 5.0).abs() < 1e-4);
    }

    #[test]
    fn clearing_an_empty_path_leaves_it_empty() {
        let mut domain = SplineDomain::default();
        domain.clear();
        assert!(domain.points.is_empty());
        assert_eq!(domain.total_length_m(), 0.0);
    }
}
