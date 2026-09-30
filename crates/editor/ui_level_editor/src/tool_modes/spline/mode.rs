//! Viewport tools for scene splines. All controls live in the Spline panel.
use super::super::{
    CameraFrame, PointerKind, StatusReadout, ToolMode, ToolModeContext, ToolModeId,
    ToolPointerEvent, ToolPointerResult, ViewportFrame,
};
use crate::{
    core::splines,
    state::spline::{DrawingPlane, SplinePoint, SplineTool},
};
use glam::{Mat4, Vec3};
use gpui::AppContext as _;
use gpui::MouseButton;
use rust_i18n::t;

pub fn view_projection(camera: CameraFrame, viewport: ViewportFrame) -> Mat4 {
    let (sy, cy) = camera.yaw.sin_cos();
    let (sp, cp) = camera.pitch.sin_cos();
    let position = Vec3::from(camera.position);
    let direction = Vec3::new(sy * cp, sp, -cy * cp);
    let up = if direction.dot(Vec3::Y).abs() > 0.999 {
        Vec3::Z
    } else {
        Vec3::Y
    };
    Mat4::perspective_rh(
        std::f32::consts::FRAC_PI_4,
        viewport.width / viewport.height.max(1.),
        0.1,
        10000.,
    ) * Mat4::look_at_rh(position, position + direction, up)
}
pub fn project(camera: CameraFrame, viewport: ViewportFrame, point: [f32; 3]) -> Option<[f32; 2]> {
    let clip = view_projection(camera, viewport) * Vec3::from(point).extend(1.);
    if clip.w <= 0. || clip.z < 0. {
        return None;
    }
    let ndc = clip.truncate() / clip.w;
    Some([
        (ndc.x + 1.) * 0.5 * viewport.width,
        (1. - ndc.y) * 0.5 * viewport.height,
    ])
}
fn plane_hit(
    camera: CameraFrame,
    viewport: ViewportFrame,
    x: f32,
    y: f32,
    plane: DrawingPlane,
    offset: f32,
) -> Option<[f32; 3]> {
    if viewport.width <= 0. || viewport.height <= 0. {
        return None;
    }
    let inverse = view_projection(camera, viewport).inverse();
    let near = inverse.project_point3(Vec3::new(x * 2. - 1., 1. - y * 2., 0.));
    let far = inverse.project_point3(Vec3::new(x * 2. - 1., 1. - y * 2., 1.));
    let ray = (far - near).normalize_or_zero();
    let axis = plane.normal_axis();
    if ray[axis].abs() < 1e-5 {
        return None;
    }
    let distance = (offset - near[axis]) / ray[axis];
    (distance > 0.).then(|| (near + ray * distance).to_array())
}

#[derive(Clone, Copy, Default)]
pub struct SplineMode;
impl ToolMode for SplineMode {
    fn build_panels(&self, ctx: &mut super::super::ModePanelContext<'_, '_>) -> Vec<std::sync::Arc<dyn ui::dock::PanelView>> {
        let state = ctx.state.clone();
        let owner = ctx.owner.clone();
        let panel = {
            let window = &mut *ctx.window;
            ctx.cx.new(|cx| crate::ui::panel::spline::SplinePanel::new(state, owner, window, cx))
        };
        vec![std::sync::Arc::new(panel)]
    }
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
    fn on_mode_entered(&mut self, ctx: &mut ToolModeContext) {
        splines::sync_selection(ctx.state);
    }
    fn on_mode_exited(&mut self, _: &mut ToolModeContext) {}
    fn status(&self, _: &ToolModeContext) -> Option<StatusReadout> {
        Some(StatusReadout {
            text: t!("LevelEditor.SplinePanel.ViewportHint").to_string(),
            tooltip: None,
        })
    }
    fn on_pointer(
        &mut self,
        event: &ToolPointerEvent,
        ctx: &mut ToolModeContext,
    ) -> ToolPointerResult {
        if event.kind != PointerKind::Down
            || event.button != Some(MouseButton::Left)
            || event.holding_mods.alt
            || event.holding_mods.control
            || event.holding_mods.platform
        {
            return ToolPointerResult::PassThrough;
        }
        splines::sync_selection(ctx.state);
        let settings = ctx.state.editor.spline.clone();
        if settings.tool == SplineTool::Navigate {
            return ToolPointerResult::PassThrough;
        }
        if matches!(settings.tool, SplineTool::Select | SplineTool::Delete) {
            let mouse = glam::Vec2::new(
                event.norm_x * ctx.viewport.width,
                event.norm_y * ctx.viewport.height,
            );
            let selected = ctx.state.scene.selected_object();
            let mut closest = None;
            let mut distance = 14.;
            for (object, curve) in splines::all(ctx.state) {
                if !object.visible || (!settings.show_all && selected.as_ref() != Some(&object.id))
                {
                    continue;
                }
                let m = splines::matrix(&object);
                for (index, p) in curve.points.iter().enumerate() {
                    if let Some(screen) = project(
                        ctx.camera,
                        ctx.viewport,
                        m.transform_point3(Vec3::from(p.position)).to_array(),
                    ) {
                        let d = mouse.distance(glam::Vec2::from(screen));
                        if d < distance {
                            distance = d;
                            closest = Some((object.id.clone(), index, object.locked));
                        }
                    }
                }
            }
            if let Some((id, index, locked)) = closest {
                splines::select(ctx.state, id);
                ctx.state.editor.spline.selected_point = Some(index);
                if settings.tool == SplineTool::Delete && !locked {
                    splines::edit(ctx.state, |d| {
                        d.points.remove(index);
                    });
                }
                return ToolPointerResult::Consumed;
            }
            return ToolPointerResult::PassThrough;
        }
        let Some(mut hit) = plane_hit(
            ctx.camera,
            ctx.viewport,
            event.norm_x,
            event.norm_y,
            settings.plane,
            settings.plane_offset,
        ) else {
            return ToolPointerResult::PassThrough;
        };
        if settings.snap {
            let (a, b) = settings.plane.axes();
            for axis in [a, b] {
                hit[axis] = (hit[axis] / settings.snap_step.max(0.001)).round()
                    * settings.snap_step.max(0.001);
            }
        }
        if settings.tool == SplineTool::Draw && splines::selected(ctx.state).is_none() {
            splines::create(ctx.state, Default::default());
        }
        let Some((object, curve)) = splines::selected(ctx.state) else {
            return ToolPointerResult::PassThrough;
        };
        if object.locked {
            return ToolPointerResult::PassThrough;
        }
        let Some(local) = splines::local_point(&object, hit) else {
            return ToolPointerResult::PassThrough;
        };
        match settings.tool {
            SplineTool::Draw => {
                let n = curve.points.len();
                splines::edit(ctx.state, |d| {
                    d.points.push(SplinePoint::new(local));
                    d.auto_tangents();
                });
                ctx.state.editor.spline.selected_point = Some(n);
            }
            SplineTool::Move => {
                if let Some(i) = settings.selected_point {
                    splines::edit(ctx.state, |d| {
                        if let Some(p) = d.points.get_mut(i) {
                            p.position = local;
                        }
                    });
                }
            }
            SplineTool::Insert => {
                let count =
                    (curve.segment_count() * curve.resolution.clamp(4, 128) as usize).max(1);
                let m = splines::matrix(&object);
                let nearest = (0..=count)
                    .filter_map(|i| {
                        let t = i as f32 / count as f32;
                        let screen = project(
                            ctx.camera,
                            ctx.viewport,
                            m.transform_point3(Vec3::from(curve.evaluate(t))).to_array(),
                        )?;
                        let d = glam::Vec2::from(screen).distance(glam::Vec2::new(
                            event.norm_x * ctx.viewport.width,
                            event.norm_y * ctx.viewport.height,
                        ));
                        Some((d, t))
                    })
                    .min_by(|a, b| a.0.total_cmp(&b.0));
                if let Some((_, t)) = nearest.filter(|(d, _)| *d < 18.) {
                    let mut inserted = None;
                    splines::edit(ctx.state, |d| {
                        inserted = d.insert_at(t);
                    });
                    ctx.state.editor.spline.selected_point = inserted;
                }
            }
            _ => {}
        }
        ToolPointerResult::Consumed
    }
    fn clone_box(&self) -> Box<dyn ToolMode> {
        Box::new(*self)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn drawing_plane_is_used() {
        let camera = CameraFrame {
            position: [0., 10., 0.],
            yaw: 0.,
            pitch: -std::f32::consts::FRAC_PI_2,
            fov: 45.,
        };
        let viewport = ViewportFrame {
            width: 100.,
            height: 100.,
        };
        let p = plane_hit(camera, viewport, 0.5, 0.5, DrawingPlane::XZ, 3.).unwrap();
        assert!((p[1] - 3.).abs() < 0.001);
    }
    #[test]
    fn invalid_viewport_cannot_place_points() {
        assert!(
            plane_hit(
                CameraFrame::default(),
                ViewportFrame {
                    width: 0.,
                    height: 0.
                },
                0.5,
                0.5,
                DrawingPlane::XZ,
                0.
            )
            .is_none()
        );
    }
}
