//! Painting helpers for charts and diagrams drawn with GPUI paths inside a
//! `canvas`: polylines, polygons, dashes, dots and arrows.

use gpui::*;

/// Colors a chart paints with, taken from the theme by the caller.
#[derive(Clone, Copy)]
pub struct ChartColors {
    pub grid: Hsla,
    pub axis: Hsla,
    pub line: Hsla,
    pub fill: Hsla,
    pub accent: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub good: Hsla,
    pub bad: Hsla,
    pub info: Hsla,
    pub warn: Hsla,
}

pub fn pt(x: f32, y: f32) -> Point<Pixels> {
    point(px(x), px(y))
}

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

pub fn fill_polygon(window: &mut Window, points: &[Point<Pixels>], color: Hsla) {
    if points.len() < 3 {
        return;
    }
    let mut builder = PathBuilder::fill();
    builder.move_to(points[0]);
    for p in &points[1..] {
        builder.line_to(*p);
    }
    builder.close();
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

pub fn dashed_line(window: &mut Window, a: Point<Pixels>, b: Point<Pixels>, width: f32, color: Hsla) {
    let mut builder = PathBuilder::stroke(px(width)).dash_array(&[px(5.0), px(4.0)]);
    builder.move_to(a);
    builder.line_to(b);
    if let Ok(path) = builder.build() {
        window.paint_path(path, color);
    }
}

pub fn dot(window: &mut Window, center: Point<Pixels>, radius: f32, color: Hsla) {
    let bounds = Bounds::new(
        point(center.x - px(radius), center.y - px(radius)),
        size(px(radius * 2.0), px(radius * 2.0)),
    );
    window.paint_quad(quad(
        bounds,
        px(radius),
        color,
        0.0,
        gpui::transparent_black(),
        BorderStyle::default(),
    ));
}

pub fn arrow(window: &mut Window, from: (f32, f32), to: (f32, f32), width: f32, color: Hsla) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let len = (dx * dx + dy * dy).sqrt();
    if len < 3.0 {
        return;
    }
    let (ux, uy) = (dx / len, dy / len);
    let head = (len * 0.35).clamp(5.0, 9.0);
    let left = (to.0 - head * (ux * 0.866 - uy * 0.5), to.1 - head * (uy * 0.866 + ux * 0.5));
    let right = (to.0 - head * (ux * 0.866 + uy * 0.5), to.1 - head * (uy * 0.866 - ux * 0.5));
    stroke_polyline(window, &[pt(from.0, from.1), pt(to.0, to.1)], width, color);
    fill_polygon(window, &[pt(to.0, to.1), pt(left.0, left.1), pt(right.0, right.1)], color);
}

