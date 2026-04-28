use super::math::{barycentric, clamp01, hsva_to_hsla, triangle_vertices};
use super::*;

pub(crate) fn picker_geometry(bounds: Bounds<Pixels>) -> Option<PickerGeometry> {
    let width = bounds.size.width.as_f32();
    let height = bounds.size.height.as_f32();
    if width <= 0.0 || height <= 0.0 {
        return None;
    }

    let radius = width.min(height) * 0.5 - 2.0;
    let inner_r = (radius - HUE_RING_THICKNESS).max(12.0);

    Some(PickerGeometry {
        cx: bounds.origin.x.as_f32() + width * 0.5,
        cy: bounds.origin.y.as_f32() + height * 0.5,
        outer_r: radius,
        inner_r,
    })
}

pub(crate) fn paint_hue_wheel(window: &mut Window, geometry: PickerGeometry) {
    let steps = ((geometry.outer_r * std::f32::consts::TAU) / 1.35)
        .round()
        .clamp(360.0, 960.0) as usize;
    for i in 0..steps {
        let t0 = i as f32 / steps as f32;
        let t1 = (i + 1) as f32 / steps as f32;
        let a0 = std::f32::consts::TAU * t0 - std::f32::consts::FRAC_PI_2;
        let a1 = std::f32::consts::TAU * t1 - std::f32::consts::FRAC_PI_2;

        let outer0 = (geometry.cx + a0.cos() * geometry.outer_r, geometry.cy + a0.sin() * geometry.outer_r);
        let outer1 = (geometry.cx + a1.cos() * geometry.outer_r, geometry.cy + a1.sin() * geometry.outer_r);
        let inner1 = (geometry.cx + a1.cos() * geometry.inner_r, geometry.cy + a1.sin() * geometry.inner_r);
        let inner0 = (geometry.cx + a0.cos() * geometry.inner_r, geometry.cy + a0.sin() * geometry.inner_r);

        let mut builder = gpui::PathBuilder::fill();
        builder.move_to(point(px(outer0.0), px(outer0.1)));
        builder.line_to(point(px(outer1.0), px(outer1.1)));
        builder.line_to(point(px(inner1.0), px(inner1.1)));
        builder.line_to(point(px(inner0.0), px(inner0.1)));
        builder.close();

        if let Ok(path) = builder.build() {
            window.paint_path(path, hsva_to_hsla(t0, 1.0, 1.0, 1.0));
        }
    }

    for (radius, alpha, width) in [
        (geometry.outer_r + 0.6, 0.18, 1.2),
        (geometry.outer_r - 0.4, 0.12, 0.9),
        (geometry.inner_r + 0.4, 0.16, 1.0),
        (geometry.inner_r - 0.4, 0.10, 0.8),
    ] {
        let mut edge = gpui::PathBuilder::stroke(px(width));
        let edge_steps = (steps * 2).clamp(720, 1920);
        for i in 0..=edge_steps {
            let t = i as f32 / edge_steps as f32;
            let a = std::f32::consts::TAU * t - std::f32::consts::FRAC_PI_2;
            let p = point(
                px(geometry.cx + a.cos() * radius),
                px(geometry.cy + a.sin() * radius),
            );
            if i == 0 {
                edge.move_to(p);
            } else {
                edge.line_to(p);
            }
        }
        if let Ok(path) = edge.build() {
            window.paint_path(path, gpui::black().opacity(alpha));
        }
    }
}

pub(crate) fn paint_sv_triangle(window: &mut Window, geometry: PickerGeometry, hue: f32) {
    let [a, b, c] = triangle_vertices(geometry, hue);
    let subdivisions = (geometry.inner_r / 2.0).round().clamp(42.0, 72.0) as usize;

    let point_from_uv = |u: f32, v: f32| {
        let wa = 1.0 - u - v;
        let wb = u;
        let wc = v;
        (
            wa * a.0 + wb * b.0 + wc * c.0,
            wa * a.1 + wb * b.1 + wc * c.1,
        )
    };

    for i in 0..subdivisions {
        for j in 0..(subdivisions - i) {
            let u0 = i as f32 / subdivisions as f32;
            let v0 = j as f32 / subdivisions as f32;
            let u1 = (i + 1) as f32 / subdivisions as f32;
            let v1 = (j + 1) as f32 / subdivisions as f32;

            let p0 = point_from_uv(u0, v0);
            let p1 = point_from_uv(u1, v0);
            let p2 = point_from_uv(u0, v1);

            let mut tri0 = gpui::PathBuilder::fill();
            tri0.move_to(point(px(p0.0), px(p0.1)));
            tri0.line_to(point(px(p1.0), px(p1.1)));
            tri0.line_to(point(px(p2.0), px(p2.1)));
            tri0.close();

            if let Ok(path) = tri0.build() {
                let center = ((p0.0 + p1.0 + p2.0) / 3.0, (p0.1 + p1.1 + p2.1) / 3.0);
                let (w_h, w_w, _w_b) = barycentric(center, a, b, c);
                let v = clamp01(w_h + w_w);
                let s = if v <= 0.0001 { 0.0 } else { clamp01(w_h / v) };
                window.paint_path(path, hsva_to_hsla(hue, s, v, 1.0));
            }

            if i + j + 1 < subdivisions {
                let p3 = point_from_uv(u1, v1);
                let mut tri1 = gpui::PathBuilder::fill();
                tri1.move_to(point(px(p1.0), px(p1.1)));
                tri1.line_to(point(px(p3.0), px(p3.1)));
                tri1.line_to(point(px(p2.0), px(p2.1)));
                tri1.close();

                if let Ok(path) = tri1.build() {
                    let center = ((p1.0 + p3.0 + p2.0) / 3.0, (p1.1 + p3.1 + p2.1) / 3.0);
                    let (w_h, w_w, _w_b) = barycentric(center, a, b, c);
                    let v = clamp01(w_h + w_w);
                    let s = if v <= 0.0001 { 0.0 } else { clamp01(w_h / v) };
                    window.paint_path(path, hsva_to_hsla(hue, s, v, 1.0));
                }
            }
        }
    }

    let mut tri_edge_outer = gpui::PathBuilder::stroke(px(1.2));
    tri_edge_outer.move_to(point(px(a.0), px(a.1)));
    tri_edge_outer.line_to(point(px(b.0), px(b.1)));
    tri_edge_outer.line_to(point(px(c.0), px(c.1)));
    tri_edge_outer.close();
    if let Ok(path) = tri_edge_outer.build() {
        window.paint_path(path, gpui::black().opacity(0.25));
    }

    let mut tri_edge_inner = gpui::PathBuilder::stroke(px(0.8));
    tri_edge_inner.move_to(point(px(a.0), px(a.1)));
    tri_edge_inner.line_to(point(px(b.0), px(b.1)));
    tri_edge_inner.line_to(point(px(c.0), px(c.1)));
    tri_edge_inner.close();
    if let Ok(path) = tri_edge_inner.build() {
        window.paint_path(path, gpui::white().opacity(0.16));
    }
}

pub(crate) fn paint_slider_gradient(
    window: &mut Window,
    bounds: Bounds<Pixels>,
    channel: usize,
    rgba: gpui::Rgba,
    value_01: f32,
) {
    let width = bounds.size.width.as_f32();
    let height = bounds.size.height.as_f32();
    let x0 = bounds.origin.x.as_f32();
    let y0 = bounds.origin.y.as_f32();

    let steps = (width / 2.0).max(48.0) as usize;

    for i in 0..steps {
        let t = i as f32 / (steps.saturating_sub(1).max(1)) as f32;

        let color = match channel {
            0 => gpui::Rgba { r: t, g: rgba.g, b: rgba.b, a: 1.0 },
            1 => gpui::Rgba { r: rgba.r, g: t, b: rgba.b, a: 1.0 },
            2 => gpui::Rgba { r: rgba.r, g: rgba.g, b: t, a: 1.0 },
            _ => gpui::Rgba {
                r: rgba.r * t + 0.18 * (1.0 - t),
                g: rgba.g * t + 0.18 * (1.0 - t),
                b: rgba.b * t + 0.18 * (1.0 - t),
                a: 1.0,
            },
        };

        let cell_w = width / steps as f32;
        let rect = Bounds {
            origin: point(px(x0 + i as f32 * cell_w), px(y0)),
            size: size(px(cell_w + 0.6), px(height)),
        };
        window.paint_quad(fill(rect, color));
    }

    let thumb_x = x0 + clamp01(value_01) * width;
    let thumb = Bounds {
        origin: point(px(thumb_x - 1.0), px(y0 - 2.0)),
        size: size(px(2.0), px(height + 4.0)),
    };
    window.paint_quad(fill(thumb, gpui::white()));
}

pub(crate) fn paint_alpha_checkerboard(window: &mut Window, bounds: Bounds<Pixels>) {
    let width = bounds.size.width.as_f32();
    let height = bounds.size.height.as_f32();
    let x0 = bounds.origin.x.as_f32();
    let y0 = bounds.origin.y.as_f32();

    let light = gpui::Rgba {
        r: 0.30,
        g: 0.30,
        b: 0.30,
        a: 1.0,
    };
    let dark = gpui::Rgba {
        r: 0.18,
        g: 0.18,
        b: 0.18,
        a: 1.0,
    };

    let cols = (width / CHECKER_CELL_SIZE).ceil() as i32;
    let rows = (height / CHECKER_CELL_SIZE).ceil() as i32;

    for row in 0..rows {
        for col in 0..cols {
            let color = if (row + col) % 2 == 0 { light } else { dark };
            let rect = Bounds {
                origin: point(
                    px(x0 + col as f32 * CHECKER_CELL_SIZE),
                    px(y0 + row as f32 * CHECKER_CELL_SIZE),
                ),
                size: size(px(CHECKER_CELL_SIZE + 0.6), px(CHECKER_CELL_SIZE + 0.6)),
            };
            window.paint_quad(fill(rect, color));
        }
    }
}
