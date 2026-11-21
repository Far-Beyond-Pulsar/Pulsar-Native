//! Small painting helpers over GPUI paths.

use gpui::{point, px, Hsla, PathBuilder, Pixels, Point, Window};
use ui::PixelsExt as _;

/// A point from float pixels.
pub fn pt(x: f32, y: f32) -> Point<Pixels> {
    point(px(x), px(y))
}

/// Strokes a polyline.
pub fn stroke_polyline(window: &mut Window, points: &[Point<Pixels>], width: f32, color: Hsla) {
    if points.len() < 2 {
        return;
    }
    let mut builder = PathBuilder::stroke(px(width));
    builder.move_to(points[0]);
    for p in &points[1..] {
        builder.line_to(*p);
    }
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

/// Draws a skeleton seen from the front (camera at +Z looking at the figure),
/// fitted into `bounds`: the floor near the bottom, the figure about 85% of the
/// height. `mirror` flips it to match a mirrored video.
pub fn draw_front_skeleton(
    window: &mut Window,
    bounds: gpui::Bounds<Pixels>,
    joints: &[[f32; 3]],
    bones: &[(usize, usize)],
    mirror: bool,
    color: Hsla,
) {
    let (ox, oy) = (bounds.origin.x.as_f32(), bounds.origin.y.as_f32());
    let (w, h) = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
    let scale = h * 0.85 / 1.9;
    let flip = if mirror { -1.0 } else { 1.0 };
    let project = |p: &[f32; 3]| pt(ox + w * 0.5 + flip * p[0] * scale, oy + h * 0.93 - p[1] * scale);
    for &(a, b) in bones {
        stroke_polyline(window, &[project(&joints[a]), project(&joints[b])], 3.0, color.opacity(0.9));
    }
    for joint in joints {
        let at = project(joint);
        window.paint_quad(gpui::fill(
            gpui::Bounds::new(gpui::point(at.x - px(3.0), at.y - px(3.0)), gpui::size(px(6.0), px(6.0))),
            color,
        ));
    }
}

/// Draws image-space body keypoints (`(x, y, score)` fractions) over the video
/// tile whose NDC rectangle is `rect` (left, top, right, bottom) inside `bounds`.
pub fn draw_keypoints(
    window: &mut Window,
    bounds: gpui::Bounds<Pixels>,
    rect: [f32; 4],
    points: &[[f32; 3]],
    mirror: bool,
    color: Hsla,
) {
    let (ox, oy) = (bounds.origin.x.as_f32(), bounds.origin.y.as_f32());
    let (w, h) = (bounds.size.width.as_f32(), bounds.size.height.as_f32());
    let (left, right) = (ox + (rect[0] + 1.0) * 0.5 * w, ox + (rect[2] + 1.0) * 0.5 * w);
    let (top, bottom) = (oy + (1.0 - rect[1]) * 0.5 * h, oy + (1.0 - rect[3]) * 0.5 * h);
    let project = |p: &[f32; 3]| {
        p[0].is_finite().then(|| {
            let x = if mirror { 1.0 - p[0] } else { p[0] };
            pt(left + x * (right - left), top + p[1] * (bottom - top))
        })
    };
    for &(a, b) in &mocap_capture::COCO17_EDGES {
        if let (Some(pa), Some(pb)) = (points.get(a).and_then(project), points.get(b).and_then(project)) {
            stroke_polyline(window, &[pa, pb], 3.0, color.opacity(0.9));
        }
    }
    for at in points.iter().filter_map(project) {
        window.paint_quad(gpui::fill(
            gpui::Bounds::new(gpui::point(at.x - px(3.5), at.y - px(3.5)), gpui::size(px(7.0), px(7.0))),
            color,
        ));
    }
}
