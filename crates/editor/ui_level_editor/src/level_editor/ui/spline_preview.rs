//! Curve previews shared by the Spline panel and the viewport overlay.
use crate::level_editor::{
    core::splines,
    state::{
        LevelEditorState,
        spline::{SplineData, SplineDomain},
    },
    tool_modes::{CameraFrame, ViewportFrame, spline::project},
};
use glam::Vec3;
use gpui::*;

fn line(
    window: &mut Window,
    points: impl IntoIterator<Item = Option<Point<Pixels>>>,
    color: Hsla,
    width: f32,
) {
    let mut path = PathBuilder::stroke(px(width));
    let mut connected = false;
    for p in points {
        if let Some(p) = p {
            if connected {
                path.line_to(p);
            } else {
                path.move_to(p);
            }
            connected = true;
        } else {
            connected = false;
        }
    }
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}
fn marker(window: &mut Window, p: Point<Pixels>, color: Hsla, selected: bool) {
    let r = if selected { 4.5 } else { 3. };
    window.paint_quad(fill(
        Bounds::new(
            point(p.x - px(r), p.y - px(r)),
            size(px(r * 2.), px(r * 2.)),
        ),
        color,
    ));
}

pub(super) fn panel(
    curve: SplineData,
    settings: SplineDomain,
    color: Hsla,
    muted: Hsla,
) -> impl IntoElement {
    let samples = curve.samples();
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let (a, b) = settings.plane.axes();
            let mut low = [f32::INFINITY; 2];
            let mut high = [f32::NEG_INFINITY; 2];
            for p in samples
                .iter()
                .copied()
                .chain(curve.points.iter().map(|p| p.position))
            {
                for (i, axis) in [a, b].into_iter().enumerate() {
                    low[i] = low[i].min(p[axis]);
                    high[i] = high[i].max(p[axis]);
                }
            }
            if samples.is_empty() {
                return;
            }
            let w = f32::from(bounds.size.width);
            let h = f32::from(bounds.size.height);
            let scale = ((w - 24.) / (high[0] - low[0]).max(1.))
                .min((h - 24.) / (high[1] - low[1]).max(1.));
            let center = [(high[0] + low[0]) * 0.5, (high[1] + low[1]) * 0.5];
            let map = |p: [f32; 3]| {
                point(
                    bounds.origin.x + px(w * 0.5 + (p[a] - center[0]) * scale),
                    bounds.origin.y + px(h * 0.5 - (p[b] - center[1]) * scale),
                )
            };
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                if settings.show_polygon {
                    line(
                        window,
                        curve.points.iter().map(|p| Some(map(p.position))),
                        muted,
                        1.,
                    );
                }
                line(
                    window,
                    samples.iter().map(|p| Some(map(*p))),
                    color,
                    settings.line_width,
                );
                if settings.show_points {
                    for (i, p) in curve.points.iter().enumerate() {
                        marker(
                            window,
                            map(p.position),
                            color,
                            settings.selected_point == Some(i),
                        );
                    }
                }
            });
        },
    )
    .w_full()
    .h(px(150.))
}

pub(super) fn viewport(
    state: &LevelEditorState,
    engine: std::sync::Arc<std::sync::Mutex<engine_backend::services::gpu_renderer::GpuRenderer>>,
    color: Hsla,
    muted: Hsla,
) -> impl IntoElement {
    let active = state.scene.selected_object();
    let settings = state.editor.spline.clone();
    let curves: Vec<_> = splines::all(state)
        .into_iter()
        .filter(|(o, _)| o.visible && (settings.show_all || active.as_ref() == Some(&o.id)))
        .map(|(o, d)| {
            let samples = d.samples();
            (o, d, samples)
        })
        .collect();
    canvas(
        |_, _, _| (),
        move |bounds, (), window, _| {
            let Some(camera) = engine.try_lock().ok().and_then(|e| e.editor_camera_state()) else {
                return;
            };
            let frame = CameraFrame {
                position: camera.position,
                yaw: camera.yaw,
                pitch: camera.pitch,
                fov: 45.,
            };
            let viewport = ViewportFrame {
                width: bounds.size.width.into(),
                height: bounds.size.height.into(),
            };
            window.with_content_mask(Some(ContentMask { bounds }), |window| {
                for (object, curve, samples) in &curves {
                    let is_active = active.as_ref() == Some(&object.id);
                    let tint = if is_active { color } else { muted };
                    let matrix = splines::matrix(object);
                    let map = |p: [f32; 3]| {
                        project(
                            frame,
                            viewport,
                            matrix.transform_point3(Vec3::from(p)).to_array(),
                        )
                        .map(|p| point(bounds.origin.x + px(p[0]), bounds.origin.y + px(p[1])))
                    };
                    line(
                        window,
                        samples.iter().map(|p| map(*p)),
                        tint,
                        settings.line_width,
                    );
                    if is_active && settings.show_polygon {
                        line(
                            window,
                            curve.points.iter().map(|p| map(p.position)),
                            muted.opacity(0.5),
                            1.,
                        );
                    }
                    if settings.show_points {
                        for (i, p) in curve.points.iter().enumerate() {
                            if let Some(at) = map(p.position) {
                                marker(
                                    window,
                                    at,
                                    tint,
                                    is_active && settings.selected_point == Some(i),
                                );
                            }
                        }
                    }
                    if is_active && settings.show_tangents {
                        for p in &curve.points {
                            let position = Vec3::from(p.position);
                            for offset in [-Vec3::from(p.arrive), Vec3::from(p.leave)] {
                                line(
                                    window,
                                    [map(p.position), map((position + offset).to_array())],
                                    tint.opacity(0.65),
                                    1.,
                                );
                            }
                        }
                    }
                }
            });
        },
    )
    .absolute()
    .size_full()
}
