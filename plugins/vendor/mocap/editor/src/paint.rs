//! Small painting helpers over GPUI paths.

use gpui::{point, px, Hsla, PathBuilder, Pixels, Point, Window};

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
