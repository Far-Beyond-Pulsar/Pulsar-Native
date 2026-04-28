use gpui::{
    anchored, canvas, deferred, div, fill, point, prelude::FluentBuilder as _, px, relative,
    size, App, AppContext, Axis, Bounds, ClickEvent, Context, Corner, ElementId, Entity,
    EventEmitter, FocusHandle, Focusable, Hsla, InteractiveElement as _, IntoElement, KeyBinding,
    MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render,
    RenderOnce, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled, Subscription, Window,
};

use crate::{
    actions::{Cancel, Confirm},
    button::{Button, ButtonVariants},
    divider::Divider,
    h_flex,
    input::{InputEvent, InputState, TextInput},
    tooltip::Tooltip,
    styled::PixelsExt, v_flex, ActiveTheme as _, Colorize as _, FocusableExt as _, Icon,
    IconName, Selectable as _, Sizable, Size, StyleSized, StyledExt,
};

const CONTEXT: &'static str = "ColorPicker";
const PICKER_SIZE: f32 = 224.0;
const HUE_RING_THICKNESS: f32 = 20.0;
const SLIDER_HEIGHT: f32 = 18.0;
const CHECKER_CELL_SIZE: f32 = 8.0;
/// Columns in every row of the All Colors grid.
const ALL_COLORS_COLS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PickerDragTarget {
    HueRing,
    Triangle,
    R,
    G,
    B,
    A,
}

#[derive(Clone, Copy)]
struct PickerGeometry {
    cx: f32,
    cy: f32,
    outer_r: f32,
    inner_r: f32,
}

fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
    let h = h.rem_euclid(1.0);
    let s = clamp01(s);
    let v = clamp01(v);

    let i = (h * 6.0).floor();
    let f = h * 6.0 - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - f * s);
    let t = v * (1.0 - (1.0 - f) * s);

    match (i as i32).rem_euclid(6) {
        0 => (v, t, p),
        1 => (q, v, p),
        2 => (p, v, t),
        3 => (p, q, v),
        4 => (t, p, v),
        _ => (v, p, q),
    }
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let h = h.rem_euclid(1.0);
    let s = clamp01(s);
    let l = clamp01(l);

    if s <= f32::EPSILON {
        return (l, l, l);
    }

    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;

    fn hue_to_channel(p: f32, q: f32, mut t: f32) -> f32 {
        t = t.rem_euclid(1.0);
        if t < 1.0 / 6.0 {
            p + (q - p) * 6.0 * t
        } else if t < 1.0 / 2.0 {
            q
        } else if t < 2.0 / 3.0 {
            p + (q - p) * (2.0 / 3.0 - t) * 6.0
        } else {
            p
        }
    }

    (
        hue_to_channel(p, q, h + 1.0 / 3.0),
        hue_to_channel(p, q, h),
        hue_to_channel(p, q, h - 1.0 / 3.0),
    )
}

fn parse_percent_or_unit(value: &str) -> Option<f32> {
    let trimmed = value.trim();
    if let Some(v) = trimmed.strip_suffix('%') {
        v.trim().parse::<f32>().ok().map(|n| clamp01(n / 100.0))
    } else {
        trimmed.parse::<f32>().ok().map(clamp01)
    }
}

fn parse_rgb_channel(value: &str) -> Option<f32> {
    let trimmed = value.trim();
    if let Some(v) = trimmed.strip_suffix('%') {
        v.trim().parse::<f32>().ok().map(|n| clamp01(n / 100.0))
    } else {
        trimmed
            .parse::<f32>()
            .ok()
            .map(|n| clamp01((n / 255.0).clamp(0.0, 1.0)))
    }
}

fn parse_hue(value: &str) -> Option<f32> {
    let trimmed = value.trim().trim_end_matches("deg").trim();
    trimmed
        .parse::<f32>()
        .ok()
        .map(|degrees| (degrees / 360.0).rem_euclid(1.0))
}

fn parse_color_function_args(input: &str, name: &str) -> Option<Vec<String>> {
    let trimmed = input.trim();
    if !trimmed.to_ascii_lowercase().starts_with(name) {
        return None;
    }
    let open = trimmed.find('(')?;
    let close = trimmed.rfind(')')?;
    if close <= open {
        return None;
    }

    let raw_args = trimmed[(open + 1)..close].trim();
    let mut args = raw_args
        .split(',')
        .map(|part| part.trim().to_string())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();

    if args.is_empty() {
        return None;
    }

    // Accept CSS forms without commas, including slash-alpha variants:
    // rgb(255 0 0), rgba(255 0 0 / 50%), hsl(210 80% 40%), hsla(... / 0.6)
    if args.len() == 1 {
        let single = args.remove(0);
        if single.contains('/') {
            let slash_parts = single
            .split('/')
            .map(|part| part.trim().to_string())
            .collect::<Vec<_>>();
            if slash_parts.len() == 2 {
                let mut space_parts = slash_parts[0]
                    .split_whitespace()
                    .map(|part| part.trim().to_string())
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>();
                space_parts.push(slash_parts[1].clone());
                args = space_parts;
            }
        } else {
            args = single
                .split_whitespace()
                .map(|part| part.trim().to_string())
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>();
        }
    }

    Some(args)
}

fn parse_color_code(input: &str) -> Option<Hsla> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }

    if let Ok(color) = Hsla::parse_hex(trimmed) {
        return Some(color);
    }

    if let Some(args) = parse_color_function_args(trimmed, "rgb") {
        if args.len() == 3 {
            let r = parse_rgb_channel(&args[0])?;
            let g = parse_rgb_channel(&args[1])?;
            let b = parse_rgb_channel(&args[2])?;
            return Some(gpui::Rgba { r, g, b, a: 1.0 }.into());
        }
    }

    if let Some(args) = parse_color_function_args(trimmed, "rgba") {
        if args.len() == 4 {
            let r = parse_rgb_channel(&args[0])?;
            let g = parse_rgb_channel(&args[1])?;
            let b = parse_rgb_channel(&args[2])?;
            let a = parse_percent_or_unit(&args[3])?;
            return Some(gpui::Rgba { r, g, b, a }.into());
        }
    }

    if let Some(args) = parse_color_function_args(trimmed, "hsl") {
        if args.len() == 3 {
            let h = parse_hue(&args[0])?;
            let s = parse_percent_or_unit(&args[1])?;
            let l = parse_percent_or_unit(&args[2])?;
            let (r, g, b) = hsl_to_rgb(h, s, l);
            return Some(gpui::Rgba { r, g, b, a: 1.0 }.into());
        }
    }

    if let Some(args) = parse_color_function_args(trimmed, "hsla") {
        if args.len() == 4 {
            let h = parse_hue(&args[0])?;
            let s = parse_percent_or_unit(&args[1])?;
            let l = parse_percent_or_unit(&args[2])?;
            let a = parse_percent_or_unit(&args[3])?;
            let (r, g, b) = hsl_to_rgb(h, s, l);
            return Some(gpui::Rgba { r, g, b, a }.into());
        }
    }

    None
}

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;

    let hue = if delta <= f32::EPSILON {
        0.0
    } else if (max - r).abs() <= f32::EPSILON {
        ((g - b) / delta).rem_euclid(6.0) / 6.0
    } else if (max - g).abs() <= f32::EPSILON {
        (((b - r) / delta) + 2.0) / 6.0
    } else {
        (((r - g) / delta) + 4.0) / 6.0
    };

    let saturation = if max <= f32::EPSILON { 0.0 } else { delta / max };
    (hue, saturation, max)
}

fn hsva_to_hsla(h: f32, s: f32, v: f32, a: f32) -> Hsla {
    let (r, g, b) = hsv_to_rgb(h, s, v);
    gpui::Rgba {
        r,
        g,
        b,
        a: clamp01(a),
    }
    .into()
}

fn hsla_to_hsva(color: Hsla) -> (f32, f32, f32, f32) {
    let rgba: gpui::Rgba = color.into();
    let (h, s, v) = rgb_to_hsv(rgba.r, rgba.g, rgba.b);
    (h, s, v, rgba.a)
}

fn picker_geometry(bounds: Bounds<Pixels>) -> Option<PickerGeometry> {
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

fn paint_hue_wheel(window: &mut Window, geometry: PickerGeometry) {
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

    // Soft anti-alias edge passes to reduce visible stair-stepping on
    // the inner/outer ring boundaries.
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

fn triangle_vertices(geometry: PickerGeometry, hue: f32) -> [(f32, f32); 3] {
    let tri_r = geometry.inner_r * 0.92;
    let base = hue * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;

    let hue_v = (
        geometry.cx + base.cos() * tri_r,
        geometry.cy + base.sin() * tri_r,
    );
    let white_v = (
        geometry.cx + (base + (2.0 * std::f32::consts::PI / 3.0)).cos() * tri_r,
        geometry.cy + (base + (2.0 * std::f32::consts::PI / 3.0)).sin() * tri_r,
    );
    let black_v = (
        geometry.cx + (base - (2.0 * std::f32::consts::PI / 3.0)).cos() * tri_r,
        geometry.cy + (base - (2.0 * std::f32::consts::PI / 3.0)).sin() * tri_r,
    );

    [hue_v, white_v, black_v]
}

fn barycentric(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> (f32, f32, f32) {
    let v0 = (b.0 - a.0, b.1 - a.1);
    let v1 = (c.0 - a.0, c.1 - a.1);
    let v2 = (p.0 - a.0, p.1 - a.1);

    let d00 = v0.0 * v0.0 + v0.1 * v0.1;
    let d01 = v0.0 * v1.0 + v0.1 * v1.1;
    let d11 = v1.0 * v1.0 + v1.1 * v1.1;
    let d20 = v2.0 * v0.0 + v2.1 * v0.1;
    let d21 = v2.0 * v1.0 + v2.1 * v1.1;

    let denom = d00 * d11 - d01 * d01;
    if denom.abs() <= f32::EPSILON {
        return (0.0, 0.0, 0.0);
    }

    let v = (d11 * d20 - d01 * d21) / denom;
    let w = (d00 * d21 - d01 * d20) / denom;
    let u = 1.0 - v - w;
    (u, v, w)
}

fn point_in_triangle(weights: (f32, f32, f32)) -> bool {
    weights.0 >= 0.0 && weights.1 >= 0.0 && weights.2 >= 0.0
}

fn closest_point_on_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    let ab = (b.0 - a.0, b.1 - a.1);
    let ap = (p.0 - a.0, p.1 - a.1);
    let ab_len_sq = ab.0 * ab.0 + ab.1 * ab.1;
    if ab_len_sq <= f32::EPSILON {
        return a;
    }

    let t = clamp01((ap.0 * ab.0 + ap.1 * ab.1) / ab_len_sq);
    (a.0 + ab.0 * t, a.1 + ab.1 * t)
}

fn clamp_point_to_triangle(
    p: (f32, f32),
    a: (f32, f32),
    b: (f32, f32),
    c: (f32, f32),
) -> (f32, f32) {
    let weights = barycentric(p, a, b, c);
    if point_in_triangle(weights) {
        return p;
    }

    let ab = closest_point_on_segment(p, a, b);
    let bc = closest_point_on_segment(p, b, c);
    let ca = closest_point_on_segment(p, c, a);

    let d_ab = (p.0 - ab.0).powi(2) + (p.1 - ab.1).powi(2);
    let d_bc = (p.0 - bc.0).powi(2) + (p.1 - bc.1).powi(2);
    let d_ca = (p.0 - ca.0).powi(2) + (p.1 - ca.1).powi(2);

    if d_ab <= d_bc && d_ab <= d_ca {
        ab
    } else if d_bc <= d_ca {
        bc
    } else {
        ca
    }
}

fn paint_sv_triangle(window: &mut Window, geometry: PickerGeometry, hue: f32) {
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
                let (w_h, w_w, w_b) = barycentric(center, a, b, c);
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
                    let (w_h, w_w, w_b) = barycentric(center, a, b, c);
                    let v = clamp01(w_h + w_w);
                    let s = if v <= 0.0001 { 0.0 } else { clamp01(w_h / v) };
                    window.paint_path(path, hsva_to_hsla(hue, s, v, 1.0));
                }
            }
        }
    }

    // Light edge feathering to smooth triangle silhouette over the ring.
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

fn paint_slider_gradient(
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

fn paint_alpha_checkerboard(window: &mut Window, bounds: Bounds<Pixels>) {
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

fn alpha_to_text(alpha: f32) -> String {
    format!("{alpha:.3}").trim_end_matches('0').trim_end_matches('.').to_string()
}

pub fn init(cx: &mut App) {
    cx.bind_keys([KeyBinding::new("escape", Cancel, Some(CONTEXT))])
}

struct ColorPickerInit;
impl crate::registry::UiComponentInit for ColorPickerInit {
    fn init(&self, cx: &mut App) {
        init(cx);
    }
}
crate::register_ui_component!(ColorPickerInit);

#[derive(Clone)]
pub enum ColorPickerEvent {
    Change(Option<Hsla>),
}

fn color_palettes() -> Vec<Vec<Hsla>> {
    use crate::theme::DEFAULT_COLORS;
    use itertools::Itertools as _;

    macro_rules! c {
        ($color:tt) => {
            DEFAULT_COLORS
                .$color
                .keys()
                .sorted()
                .map(|k| DEFAULT_COLORS.$color.get(k).map(|c| c.hsla).unwrap())
                .collect::<Vec<_>>()
        };
    }

    vec![
        c!(stone),
        c!(red),
        c!(orange),
        c!(yellow),
        c!(green),
        c!(cyan),
        c!(blue),
        c!(purple),
        c!(pink),
    ]
}

fn named_color_palettes() -> Vec<(&'static str, Vec<Hsla>)> {
    let palettes = color_palettes();
    let names = [
        "Stone",
        "Red",
        "Orange",
        "Yellow",
        "Green",
        "Cyan",
        "Blue",
        "Purple",
        "Pink",
    ];

    names
        .into_iter()
        .zip(palettes)
        .collect::<Vec<(&'static str, Vec<Hsla>)>>()
}

/// State of the [`ColorPicker`].
pub struct ColorPickerState {
    focus_handle: FocusHandle,
    value: Option<Hsla>,
    hovered_color: Option<Hsla>,
    state: Entity<InputState>,
    syncing_inputs: bool,
    open: bool,
    bounds: Bounds<Pixels>,
    picker_bounds: Bounds<Pixels>,
    slider_bounds: [Bounds<Pixels>; 4],
    active_drag: Option<PickerDragTarget>,
    triangle_drag_hue_lock: Option<f32>,
    selected_palette_index: usize,
    palette_switcher_open: bool,
    palette_header_bounds: Bounds<Pixels>,
    rgba_input_states: [Entity<InputState>; 4],
    hue: f32,
    saturation: f32,
    value_channel: f32,
    alpha: f32,
    recent_colors: Vec<Hsla>,
    _subscriptions: Vec<Subscription>,
}

impl ColorPickerState {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| {
            InputState::new(window, cx).placeholder("#RRGGBB, rgba(...), hsl(...)")
        });
        let rgba_input_states = std::array::from_fn(|_| cx.new(|cx| InputState::new(window, cx)));

        let mut _subscriptions = vec![cx.subscribe_in(
            &state,
            window,
            |this, state, ev: &InputEvent, window, cx| match ev {
                InputEvent::Change => {
                    if this.syncing_inputs {
                        return;
                    }
                    let value = state.read(cx).value();
                    if let Some(color) = parse_color_code(value.as_str()) {
                        this.apply_external_color(color, true, window, cx);
                    }
                }
                InputEvent::PressEnter { .. } => {
                    let val = this.state.read(cx).value();
                    if let Some(color) = parse_color_code(&val) {
                        this.open = false;
                        this.apply_external_color(color, true, window, cx);
                    }
                }
                _ => {}
            },
        )];

        for channel in 0..4 {
            let input_state = rgba_input_states[channel].clone();
            _subscriptions.push(cx.subscribe_in(
                &input_state,
                window,
                move |this, _state, ev: &InputEvent, window, cx| match ev {
                    InputEvent::Change => this.apply_numeric_input(channel, false, window, cx),
                    InputEvent::PressEnter { .. } => {
                        this.apply_numeric_input(channel, true, window, cx)
                    }
                    _ => {}
                },
            ));
        }

        Self {
            focus_handle: cx.focus_handle(),
            value: None,
            hovered_color: None,
            state,
            syncing_inputs: false,
            open: false,
            bounds: Bounds::default(),
            picker_bounds: Bounds::default(),
            slider_bounds: [
                Bounds::default(),
                Bounds::default(),
                Bounds::default(),
                Bounds::default(),
            ],
            active_drag: None,
            triangle_drag_hue_lock: None,
            selected_palette_index: 0,
            palette_switcher_open: false,
            palette_header_bounds: Bounds::default(),
            rgba_input_states,
            hue: 0.0,
            saturation: 0.0,
            value_channel: 1.0,
            alpha: 1.0,
            recent_colors: Vec::new(),
            _subscriptions,
        }
    }

    /// Set default color value.
    pub fn default_value(mut self, value: Hsla) -> Self {
        self.value = Some(value);
        self.sync_hsva_from_color(value);
        self
    }

    /// Set current color value.
    pub fn set_value(&mut self, value: Hsla, window: &mut Window, cx: &mut Context<Self>) {
        self.apply_external_color(value, false, window, cx);
    }

    /// Get current color value.
    pub fn value(&self) -> Option<Hsla> {
        self.value
    }

    fn on_escape(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if !self.open {
            cx.propagate();
        }

        self.open = false;
        self.palette_switcher_open = false;
        cx.notify();
    }

    fn on_confirm(&mut self, _: &Confirm, _: &mut Window, cx: &mut Context<Self>) {
        self.open = !self.open;
        if !self.open {
            self.palette_switcher_open = false;
        }
        cx.notify();
    }

    fn toggle_picker(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.open = !self.open;
        if !self.open {
            self.palette_switcher_open = false;
        }
        cx.notify();
    }

    fn toggle_palette_switcher(&mut self, _: &ClickEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.palette_switcher_open = !self.palette_switcher_open;
        cx.notify();
    }

    fn select_palette(
        &mut self,
        palette_index: usize,
        _: &ClickEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_palette_index = palette_index;
        self.palette_switcher_open = false;
        cx.notify();
    }

    fn sync_hsva_from_color(&mut self, color: Hsla) {
        let (h, s, v, a) = hsla_to_hsva(color);
        self.hue = h;
        self.saturation = s;
        self.value_channel = v;
        self.alpha = a;
    }

    fn push_recent_color(&mut self, color: Hsla) {
        let hex = color.to_hex();
        self.recent_colors.retain(|c| c.to_hex() != hex);
        self.recent_colors.insert(0, color);
        self.recent_colors.truncate(12);
    }

    fn drag_target_for_point(&self, position: Point<Pixels>) -> Option<PickerDragTarget> {
        if let Some(geometry) = picker_geometry(self.picker_bounds) {
            if self.picker_bounds.contains(&position) {
                let dx = position.x.as_f32() - geometry.cx;
                let dy = position.y.as_f32() - geometry.cy;
                let distance = (dx * dx + dy * dy).sqrt();

                if distance >= geometry.inner_r && distance <= geometry.outer_r {
                    return Some(PickerDragTarget::HueRing);
                }

                // The entire inner disc is the SV/triangle zone — no gap between
                // the triangle vertex hull and the ring's inner edge.
                if distance < geometry.inner_r {
                    return Some(PickerDragTarget::Triangle);
                }
            }
        }

        for (index, bounds) in self.slider_bounds.iter().enumerate() {
            if bounds.contains(&position) {
                return Some(match index {
                    0 => PickerDragTarget::R,
                    1 => PickerDragTarget::G,
                    2 => PickerDragTarget::B,
                    _ => PickerDragTarget::A,
                });
            }
        }

        None
    }

    fn apply_picker_point(
        &mut self,
        target: PickerDragTarget,
        position: Point<Pixels>,
        emit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(target, PickerDragTarget::HueRing | PickerDragTarget::Triangle) {
            return;
        }

        let Some(geometry) = picker_geometry(self.picker_bounds) else {
            return;
        };

        let x = position.x.as_f32();
        let y = position.y.as_f32();

        let dx = x - geometry.cx;
        let dy = y - geometry.cy;
        let distance = (dx * dx + dy * dy).sqrt();

        match target {
            PickerDragTarget::HueRing => {
                // No distance guard here — during drag we only need the angle.
                let angle = dy.atan2(dx);
                self.hue = ((angle + std::f32::consts::FRAC_PI_2) / std::f32::consts::TAU)
                    .rem_euclid(1.0);
                let color = hsva_to_hsla(self.hue, self.saturation, self.value_channel, self.alpha);
                self.update_value(Some(color), emit, window, cx);
            }
            PickerDragTarget::Triangle => {
                // No distance guard — clamp_point_to_triangle handles out-of-bounds.
                let drag_hue = self.triangle_drag_hue_lock.unwrap_or(self.hue);
                let [a, b, c] = triangle_vertices(geometry, drag_hue);
                let p = clamp_point_to_triangle((x, y), a, b, c);
                let (w_h, w_w, _w_b) = barycentric(p, a, b, c);

                let v = clamp01(w_h + w_w);
                let s = if v <= 0.0001 { 0.0 } else { clamp01(w_h / v) };

                // Set HSV directly — update_value will not touch these.
                self.hue = drag_hue;
                self.saturation = s;
                self.value_channel = v;
                let color = hsva_to_hsla(drag_hue, s, v, self.alpha);
                self.update_value(Some(color), emit, window, cx);
            }
            _ => {}
        }
    }

    fn apply_slider_point(
        &mut self,
        channel: PickerDragTarget,
        position: Point<Pixels>,
        emit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let slider_index = match channel {
            PickerDragTarget::R => 0,
            PickerDragTarget::G => 1,
            PickerDragTarget::B => 2,
            PickerDragTarget::A => 3,
            PickerDragTarget::HueRing | PickerDragTarget::Triangle => return,
        };

        let bounds = self.slider_bounds[slider_index];
        if bounds.size.width <= px(0.0) {
            return;
        }

        let t = clamp01(
            (position.x.as_f32() - bounds.origin.x.as_f32()) / bounds.size.width.as_f32(),
        );

        let color = self.value.unwrap_or_else(|| hsva_to_hsla(self.hue, self.saturation, self.value_channel, self.alpha));
        let mut rgba: gpui::Rgba = color.into();

        match channel {
            PickerDragTarget::R => rgba.r = t,
            PickerDragTarget::G => rgba.g = t,
            PickerDragTarget::B => rgba.b = t,
            PickerDragTarget::A => rgba.a = t,
            PickerDragTarget::HueRing | PickerDragTarget::Triangle => {}
        }

        let (h, s, v) = rgb_to_hsv(rgba.r, rgba.g, rgba.b);
        if s > 0.0001 {
            self.hue = h;
        }
        self.saturation = s;
        self.value_channel = v;
        self.alpha = rgba.a;

        self.update_value(Some(rgba.into()), emit, window, cx);
    }

    fn start_drag(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let position = event.position;
        self.active_drag = self.drag_target_for_point(position);
        self.triangle_drag_hue_lock = match self.active_drag {
            Some(PickerDragTarget::Triangle) => Some(self.hue),
            _ => None,
        };
        if let Some(target) = self.active_drag {
            match target {
                PickerDragTarget::HueRing | PickerDragTarget::Triangle => {
                    self.apply_picker_point(target, position, true, window, cx)
                }
                _ => self.apply_slider_point(target, position, true, window, cx),
            }
        }
    }

    fn drag_move(&mut self, position: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(target) = self.active_drag {
            match target {
                PickerDragTarget::HueRing | PickerDragTarget::Triangle => {
                    self.apply_picker_point(target, position, true, window, cx)
                }
                _ => self.apply_slider_point(target, position, true, window, cx),
            }
        }
    }

    fn stop_drag_mouse(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.active_drag = None;
        self.triangle_drag_hue_lock = None;
        cx.notify();
    }

    fn sync_numeric_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let color = self
            .value
            .unwrap_or_else(|| hsva_to_hsla(self.hue, self.saturation, self.value_channel, self.alpha));
        let rgba: gpui::Rgba = color.into();

        let texts = [
            (rgba.r * 255.0).round().clamp(0.0, 255.0).to_string(),
            (rgba.g * 255.0).round().clamp(0.0, 255.0).to_string(),
            (rgba.b * 255.0).round().clamp(0.0, 255.0).to_string(),
            alpha_to_text(rgba.a),
        ];

        for (index, text) in texts.iter().enumerate() {
            self.rgba_input_states[index].update(cx, |input, cx| {
                if input.value() != *text {
                    input.set_value(text, window, cx);
                }
            });
        }
    }

    fn apply_numeric_input(
        &mut self,
        channel: usize,
        emit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.syncing_inputs {
            return;
        }

        let raw = self.rgba_input_states[channel].read(cx).value();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return;
        }

        let color = self
            .value
            .unwrap_or_else(|| hsva_to_hsla(self.hue, self.saturation, self.value_channel, self.alpha));
        let mut rgba: gpui::Rgba = color.into();

        let parsed_ok = if channel <= 2 {
            match trimmed.parse::<i32>() {
                Ok(v) => {
                    let clamped = v.clamp(0, 255) as f32 / 255.0;
                    match channel {
                        0 => rgba.r = clamped,
                        1 => rgba.g = clamped,
                        _ => rgba.b = clamped,
                    }
                    true
                }
                Err(_) => false,
            }
        } else {
            match trimmed.parse::<f32>() {
                Ok(v) => {
                    rgba.a = clamp01(v);
                    true
                }
                Err(_) => false,
            }
        };

        if parsed_ok {
            let (h, s, v) = rgb_to_hsv(rgba.r, rgba.g, rgba.b);
            if s > 0.0001 {
                self.hue = h;
            }
            self.saturation = s;
            self.value_channel = v;
            self.alpha = rgba.a;

            self.update_value(Some(rgba.into()), emit, window, cx);
        }
    }

    /// Apply a color that came from outside the picker's own HSV state
    /// (palette swatch, hex field, public set_value). Syncs HSV first, then
    /// records the value. Internal drag/slider code must NOT use this.
    fn apply_external_color(
        &mut self,
        color: Hsla,
        emit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.sync_hsva_from_color(color);
        self.update_value(Some(color), emit, window, cx);
    }

    /// Record the final color, sync text inputs, and emit. Never touches
    /// hue/saturation/value_channel/alpha — callers own those fields.
    fn update_value(
        &mut self,
        value: Option<Hsla>,
        emit: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.value = value;
        self.hovered_color = value;
        if let Some(color) = value {
            self.push_recent_color(color);
        }
        self.syncing_inputs = true;
        self.state.update(cx, |view, cx| {
            if let Some(value) = value {
                let hex = value.to_hex();
                if view.value() != hex {
                    view.set_value(hex, window, cx);
                }
            } else {
                if !view.value().is_empty() {
                    view.set_value("", window, cx);
                }
            }
        });
        self.sync_numeric_inputs(window, cx);
        self.syncing_inputs = false;
        if emit {
            cx.emit(ColorPickerEvent::Change(value));
        }
        cx.notify();
    }
}
impl EventEmitter<ColorPickerEvent> for ColorPickerState {}
impl Render for ColorPickerState {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        self.state.clone()
    }
}
impl Focusable for ColorPickerState {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[derive(IntoElement)]
pub struct ColorPicker {
    id: ElementId,
    style: StyleRefinement,
    state: Entity<ColorPickerState>,
    featured_colors: Option<Vec<Hsla>>,
    label: Option<SharedString>,
    icon: Option<Icon>,
    size: Size,
    anchor: Corner,
}

impl ColorPicker {
    pub fn new(state: &Entity<ColorPickerState>) -> Self {
        Self {
            id: ("color-picker", state.entity_id()).into(),
            style: StyleRefinement::default(),
            state: state.clone(),
            featured_colors: None,
            size: Size::Medium,
            label: None,
            icon: None,
            anchor: Corner::TopLeft,
        }
    }

    /// Set the featured colors to be displayed in the color picker.
    ///
    /// This is used to display a set of colors that the user can quickly select from,
    /// for example provided user's last used colors.
    pub fn featured_colors(mut self, colors: Vec<Hsla>) -> Self {
        self.featured_colors = Some(colors);
        self
    }

    /// Set the size of the color picker, default is `Size::Medium`.
    pub fn size(mut self, size: Size) -> Self {
        self.size = size;
        self
    }

    /// Set the icon to the color picker button.
    ///
    /// If this is set the color picker button will display the icon.
    /// Else it will display the square color of the current value.
    pub fn icon(mut self, icon: impl Into<Icon>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Set the label to be displayed above the color picker.
    ///
    /// Default is `None`.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Set the anchor corner of the color picker.
    ///
    /// Default is `Corner::TopLeft`.
    pub fn anchor(mut self, anchor: Corner) -> Self {
        self.anchor = anchor;
        self
    }

    fn render_item(
        &self,
        color: Hsla,
        clickable: bool,
        window: &mut Window,
        _: &mut App,
    ) -> impl IntoElement {
        let state = self.state.clone();
        div()
            .id(SharedString::from(format!("color-{}", color.to_hex())))
            .h_5()
            .w_5()
            .bg(color)
            .border_1()
            .border_color(color.darken(0.1))
            .when(clickable, |this| {
                this.hover(|this| {
                    this.border_color(color.darken(0.3))
                        .bg(color.lighten(0.1))
                        .shadow_xs()
                })
                .active(|this| this.border_color(color.darken(0.5)).bg(color.darken(0.2)))
                .on_click(window.listener_for(
                    &state,
                    move |state, _, window, cx| {
                        state.apply_external_color(color, true, window, cx);
                    },
                ))
            })
    }

    fn render_palette_switcher_popout(&self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (selected_palette_index, palette_switcher_open, palette_header_bounds) = {
            let state = self.state.read(cx);
            (state.selected_palette_index, state.palette_switcher_open, state.palette_header_bounds)
        };
        let named_palettes = named_color_palettes();
        let safe_palette_index = selected_palette_index.min(named_palettes.len().saturating_sub(1));

        div()
            .when(palette_switcher_open, |this| {
                this.child(
                    deferred(
                        anchored()
                            .position(palette_header_bounds.corner(Corner::BottomLeft))
                            .snap_to_window_with_margin(px(8.))
                            .child(
                                div()
                                    .occlude()
                                    .mt_1p5()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .shadow_lg()
                                    .bg(cx.theme().background)
                                    .w(px(300.0))
                                    .child(
                                        v_flex()
                                            .max_h(px(300.0))
                                            .scrollable(Axis::Vertical)
                                            .child(
                                                v_flex().gap_px().children(
                                                    named_palettes
                                                        .iter()
                                                        .enumerate()
                                                        .map(|(ix, (name, colors))| {
                                                            let swatches = colors.iter().copied().take(9).collect::<Vec<_>>();
                                                            h_flex()
                                                                .w_full()
                                                                .items_center()
                                                                .justify_between()
                                                                .gap_2()
                                                                .px_3()
                                                                .py_2()
                                                                .when(ix == safe_palette_index, |this| {
                                                                    this.bg(cx.theme().accent.opacity(0.16))
                                                                })
                                                                .hover(|this| this.bg(cx.theme().muted.opacity(0.45)))
                                                                .child(
                                                                    div()
                                                                        .text_sm()
                                                                        .font_semibold()
                                                                        .text_color(cx.theme().foreground)
                                                                        .child((*name).to_string()),
                                                                )
                                                                .child(
                                                                    h_flex().gap_1().children(swatches.into_iter().map(|color| {
                                                                        div()
                                                                            .h_4()
                                                                            .w_4()
                                                                            .bg(color)
                                                                            .border_1()
                                                                            .border_color(color.darken(0.2))
                                                                    })),
                                                                )
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    window.listener_for(
                                                                        &self.state,
                                                                        move |state, _, window, cx| {
                                                                            state.selected_palette_index = ix;
                                                                            state.palette_switcher_open = false;
                                                                            cx.notify();
                                                                        },
                                                                    ),
                                                                )
                                                        }),
                                                ),
                                            ),
                                    )
                                    .on_mouse_down_out(
                                        window.listener_for(&self.state, |state, _, _window, cx| {
                                            state.palette_switcher_open = false;
                                            cx.notify();
                                        }),
                                    ),
                            ),
                    )
                    .with_priority(2),
                )
            })
    }

    fn render_all_colors_grid(&self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        // Each named palette is one row, sorted dark → light (value ascending).
        let color_rows = named_color_palettes()
            .into_iter()
            .map(|(_, mut palette_colors)| {
                palette_colors.sort_by(|a, b| {
                    let (_, _, v_a, _) = hsla_to_hsva(*a);
                    let (_, _, v_b, _) = hsla_to_hsva(*b);
                    v_a.partial_cmp(&v_b).unwrap_or(std::cmp::Ordering::Equal)
                });
                let row_colors: Vec<Hsla> = palette_colors.into_iter().take(ALL_COLORS_COLS).collect();
                h_flex().gap_1().children(row_colors.into_iter().map(|color| {
                    let state = self.state.clone();
                    div()
                        .id(SharedString::from(format!("all-color-{}", color.to_hex())))
                        .h_5()
                        .w_5()
                        .bg(color)
                        .border_1()
                        .border_color(color.darken(0.1))
                        .hover(|this| {
                            this.border_color(color.darken(0.3))
                                .bg(color.lighten(0.1))
                                .shadow_xs()
                        })
                        .active(|this| this.border_color(color.darken(0.5)).bg(color.darken(0.2)))
                        .on_click(window.listener_for(&state, move |state, _, window, cx| {
                            state.apply_external_color(color, true, window, cx);
                        }))
                }))
            })
            .collect::<Vec<_>>();

        v_flex()
            .gap_px()
            .child(
                div()
                    .text_xs()
                    .font_semibold()
                    .pb_1()
                    .text_color(cx.theme().muted_foreground)
                    .child("All Colors"),
            )
            .children(color_rows)
    }

    fn render_rgba_slider(
        &self,
        channel: usize,
        label: &'static str,
        value_255: u8,
        alpha_value: f32,
        rgba: gpui::Rgba,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let state = self.state.clone();
        let numeric_input_state = {
            let picker = state.read(cx);
            picker.rgba_input_states[channel].clone()
        };
        let value_01 = value_255 as f32 / 255.0;

        h_flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .w(px(18.0))
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(label),
            )
            .child(
                div()
                    .relative()
                    .h(px(SLIDER_HEIGHT))
                    .flex_1()
                    .rounded_md()
                    .overflow_hidden()
                    .border_1()
                    .border_color(cx.theme().border.opacity(0.6))
                    .child(
                        canvas(
                            {
                                let state = state.clone();
                                move |bounds, _, cx| {
                                    state.update(cx, |picker, _| picker.slider_bounds[channel] = bounds);
                                    bounds
                                }
                            },
                            move |bounds, _, window, _| {
                                paint_slider_gradient(window, bounds, channel, rgba, value_01);
                            },
                        )
                        .size_full(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        window.listener_for(&state, move |picker, event, window, cx| {
                            picker.active_drag = Some(match channel {
                                0 => PickerDragTarget::R,
                                1 => PickerDragTarget::G,
                                2 => PickerDragTarget::B,
                                _ => PickerDragTarget::A,
                            });
                            picker.start_drag(event, window, cx);
                        }),
                    )
                    .on_mouse_move(window.listener_for(&state, move |picker, event: &MouseMoveEvent, window, cx| {
                        if picker.active_drag.is_some() {
                            picker.drag_move(event.position, window, cx);
                        }
                    }))
                    .on_mouse_up(MouseButton::Left, window.listener_for(&state, ColorPickerState::stop_drag_mouse))
                    .on_mouse_up_out(MouseButton::Left, window.listener_for(&state, ColorPickerState::stop_drag_mouse)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        TextInput::new(&numeric_input_state)
                            .xsmall()
                            .w(px(52.0))
                            .font_family("JetBrainsMono-Regular")
                            .text_xs(),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground.opacity(0.6))
                            .child(if channel == 3 { "0-1" } else { "0-255" }),
                    ),
            )
    }

    fn render_relation_row(
        &self,
        title: &'static str,
        colors: Vec<Hsla>,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .items_center()
            .justify_between()
            .child(
                div()
                    .w(px(108.0))
                    .text_xs()
                    .font_semibold()
                    .text_color(cx.theme().muted_foreground)
                    .child(title),
            )
            .child(
                h_flex()
                    .gap_1()
                    .children(colors.into_iter().map(|color| self.render_item(color, true, window, cx))),
            )
    }

    fn render_advanced_picker(&self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (current, hue_value, sat_value, val_value, alpha_value, recent_colors, selected_palette_index, palette_switcher_open, code_input_state) = {
            let state = self.state.read(cx);
            (
                state
                    .value
                    .unwrap_or_else(|| hsva_to_hsla(state.hue, state.saturation, state.value_channel, state.alpha)),
                state.hue,
                state.saturation,
                state.value_channel,
                state.alpha,
                state.recent_colors.clone(),
                state.selected_palette_index,
                state.palette_switcher_open,
                state.state.clone(),
            )
        };
        let named_palettes = named_color_palettes();
        let safe_palette_index = selected_palette_index.min(named_palettes.len().saturating_sub(1));
        let (selected_palette_name, selected_palette_colors) = named_palettes
            .get(safe_palette_index)
            .cloned()
            .unwrap_or(("Palette", Vec::new()));

        let rgba: gpui::Rgba = current.into();
        let r_u8 = (rgba.r * 255.0).round() as u8;
        let g_u8 = (rgba.g * 255.0).round() as u8;
        let b_u8 = (rgba.b * 255.0).round() as u8;
        let a_u8 = (rgba.a * 255.0).round() as u8;

        let complementary = hsva_to_hsla(
            (hue_value + 0.5).rem_euclid(1.0),
            sat_value,
            val_value,
            alpha_value,
        );
        let triad_a = hsva_to_hsla(
            (hue_value + (1.0 / 3.0)).rem_euclid(1.0),
            sat_value,
            val_value,
            alpha_value,
        );
        let triad_b = hsva_to_hsla(
            (hue_value + (2.0 / 3.0)).rem_euclid(1.0),
            sat_value,
            val_value,
            alpha_value,
        );

        let state_entity = self.state.clone();
        let hue = hue_value;
        let sat = sat_value;
        let val = val_value;

        let analogous_l = hsva_to_hsla(
            (hue_value - (1.0 / 12.0)).rem_euclid(1.0),
            sat_value,
            val_value,
            alpha_value,
        );
        let analogous_r = hsva_to_hsla(
            (hue_value + (1.0 / 12.0)).rem_euclid(1.0),
            sat_value,
            val_value,
            alpha_value,
        );
        let split_l = hsva_to_hsla(
            (hue_value + 0.5 - (1.0 / 12.0)).rem_euclid(1.0),
            sat_value,
            val_value,
            alpha_value,
        );
        let split_r = hsva_to_hsla(
            (hue_value + 0.5 + (1.0 / 12.0)).rem_euclid(1.0),
            sat_value,
            val_value,
            alpha_value,
        );

        v_flex()
            .gap_3()
            .child(
                h_flex()
                    .w_full()
                    .items_start()
                    .gap_3()
                    .child(
                        div()
                            .relative()
                            .flex_shrink_0()
                            .size(px(PICKER_SIZE))
                            .rounded_lg()
                            .overflow_hidden()
                            .border_1()
                            .border_color(cx.theme().border.opacity(0.65))
                            .child(
                                canvas(
                                    {
                                        let state = state_entity.clone();
                                        move |bounds, _, cx| {
                                            state.update(cx, |picker, _| picker.picker_bounds = bounds);
                                            bounds
                                        }
                                    },
                                    move |bounds, _, window, _| {
                                        let Some(geometry) = picker_geometry(bounds) else {
                                            return;
                                        };

                                        paint_hue_wheel(window, geometry);
                                        paint_sv_triangle(window, geometry, hue);

                                        let ring_angle =
                                            hue * std::f32::consts::TAU - std::f32::consts::FRAC_PI_2;
                                        let ring_radius = (geometry.outer_r + geometry.inner_r) * 0.5;
                                        let ring_x = geometry.cx + ring_angle.cos() * ring_radius;
                                        let ring_y = geometry.cy + ring_angle.sin() * ring_radius;

                                        let ring_marker = Bounds {
                                            origin: point(px(ring_x - 4.0), px(ring_y - 4.0)),
                                            size: size(px(8.0), px(8.0)),
                                        };
                                        window.paint_quad(fill(ring_marker, gpui::white()));

                                        let [a, b, c] = triangle_vertices(geometry, hue);
                                        let w_h = sat * val;
                                        let w_w = (1.0 - sat) * val;
                                        let w_b = 1.0 - val;

                                        let tri_x = w_h * a.0 + w_w * b.0 + w_b * c.0;
                                        let tri_y = w_h * a.1 + w_w * b.1 + w_b * c.1;

                                        let tri_marker = Bounds {
                                            origin: point(px(tri_x - 5.0), px(tri_y - 5.0)),
                                            size: size(px(10.0), px(10.0)),
                                        };
                                        window.paint_quad(fill(tri_marker, gpui::black().opacity(0.65)));
                                        let inner = Bounds {
                                            origin: point(px(tri_x - 3.0), px(tri_y - 3.0)),
                                            size: size(px(6.0), px(6.0)),
                                        };
                                        window.paint_quad(fill(inner, gpui::white()));
                                    },
                                )
                                .size_full(),
                            )
                            .on_mouse_down(
                                MouseButton::Left,
                                window.listener_for(&state_entity, |picker, event, window, cx| {
                                    picker.start_drag(event, window, cx);
                                }),
                            )
                            .on_mouse_move(window.listener_for(&state_entity, move |picker, event: &MouseMoveEvent, window, cx| {
                                if picker.active_drag.is_some() {
                                    picker.drag_move(event.position, window, cx);
                                }
                            }))
                            .on_mouse_up(MouseButton::Left, window.listener_for(&state_entity, ColorPickerState::stop_drag_mouse))
                            .on_mouse_up_out(MouseButton::Left, window.listener_for(&state_entity, ColorPickerState::stop_drag_mouse)),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_2()
                            .child(
                                div()
                                    .relative()
                                    .w_full()
                                    .h(px(52.0))
                                    .rounded_md()
                                    .overflow_hidden()
                                    .border_1()
                                    .border_color(current.darken(0.35))
                                    .child(
                                        canvas(
                                            |bounds, _, _| bounds,
                                            |bounds, _, window, _| {
                                                paint_alpha_checkerboard(window, bounds);
                                            },
                                        )
                                        .size_full()
                                        .absolute()
                                        .inset_0(),
                                    )
                                    .child(div().absolute().inset_0().bg(current)),
                            )
                            .child(
                                v_flex()
                                    .gap_1()
                                    .p_2()
                                    .rounded_md()
                                    .border_1()
                                    .border_color(cx.theme().border.opacity(0.55))
                                    .bg(cx.theme().muted.opacity(0.25))
                                    .child(
                                        v_flex()
                                            .gap_px()
                                            .text_xs()
                                            .font_family("JetBrainsMono-Regular")
                                            .text_color(cx.theme().muted_foreground)
                                            .child(format!("HEX  {}", current.to_hex()))
                                            .child(format!("RGBA {}, {}, {}, {}", r_u8, g_u8, b_u8, a_u8)),
                                    )
                                    .child(
                                        h_flex()
                                            .items_center()
                                            .gap_2()
                                            .child(
                                                div()
                                                    .w(px(34.0))
                                                    .text_xs()
                                                    .font_semibold()
                                                    .text_color(cx.theme().muted_foreground)
                                                    .child("Code"),
                                            )
                                            .child(
                                                TextInput::new(&code_input_state)
                                                    .xsmall()
                                                    .w_full()
                                                    .cleanable()
                                                    .font_family("JetBrainsMono-Regular"),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_family("JetBrainsMono-Regular")
                                            .text_color(cx.theme().muted_foreground.opacity(0.7))
                                            .child("HEX, RGB(A), HSL(A)"),
                                    ),
                            )
                            .child(self.render_rgba_slider(0, "R", r_u8, alpha_value, rgba, window, cx))
                            .child(self.render_rgba_slider(1, "G", g_u8, alpha_value, rgba, window, cx))
                            .child(self.render_rgba_slider(2, "B", b_u8, alpha_value, rgba, window, cx))
                            .child(self.render_rgba_slider(3, "A", a_u8, alpha_value, rgba, window, cx)),
                    )
                    .on_mouse_move(window.listener_for(&state_entity, move |picker, event: &MouseMoveEvent, window, cx| {
                        if picker.active_drag.is_some() {
                            picker.drag_move(event.position, window, cx);
                        }
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        window.listener_for(&state_entity, ColorPickerState::stop_drag_mouse),
                    )
                    .on_mouse_up_out(
                        MouseButton::Left,
                        window.listener_for(&state_entity, ColorPickerState::stop_drag_mouse),
                    ),
            )
            .child(
                v_flex()
                    .gap_2()
                    .child(Divider::horizontal())
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(cx.theme().muted_foreground)
                            .child("Color Relations"),
                    )
                    .child(self.render_relation_row("Complementary", vec![current, complementary], window, cx))
                    .child(self.render_relation_row("Analogous", vec![analogous_l, current, analogous_r], window, cx))
                    .child(self.render_relation_row("Split-Comp", vec![current, split_l, split_r], window, cx))
                    .child(self.render_relation_row("Triadic", vec![current, triad_a, triad_b], window, cx)),
            )
            .when(!recent_colors.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .font_semibold()
                                .text_color(cx.theme().muted_foreground)
                                .child("Recent"),
                        )
                        .child(
                            h_flex().gap_1().children(
                                recent_colors
                                    .iter()
                                    .copied()
                                    .map(|color| self.render_item(color, true, window, cx)),
                            ),
                        ),
                )
            })
            .child(
                v_flex()
                    .gap_1()
                    .child(Divider::horizontal())
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .justify_between()
                            .relative()
                            .child(
                                canvas(
                                    {
                                        let state = self.state.clone();
                                        move |bounds, _, cx| state.update(cx, |r, _| r.palette_header_bounds = bounds)
                                    },
                                    |_, _, _, _| {},
                                )
                                .absolute()
                                .size_full(),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .font_semibold()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("Palette: {}", selected_palette_name)),
                            )
                            .child(
                                Button::new("palette-switcher")
                                    .ghost()
                                    .xsmall()
                                    .icon(if palette_switcher_open {
                                        Icon::new(IconName::ChevronUp)
                                    } else {
                                        Icon::new(IconName::ChevronDown)
                                    })
                                    .on_click(window.listener_for(
                                        &self.state,
                                        ColorPickerState::toggle_palette_switcher,
                                    )),
                            ),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .flex_wrap()
                            .gap_1()
                            .children(
                                selected_palette_colors
                                    .iter()
                                    .copied()
                                    .map(|color| self.render_item(color, true, window, cx)),
                            ),
                    )
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(Divider::horizontal())
                    .child(self.render_all_colors_grid(window, cx)),
            )
    }

    fn resolved_corner(&self, bounds: Bounds<Pixels>) -> Point<Pixels> {
        bounds.corner(match self.anchor {
            Corner::TopLeft => Corner::BottomLeft,
            Corner::TopRight => Corner::BottomRight,
            Corner::BottomLeft => Corner::TopLeft,
            Corner::BottomRight => Corner::TopRight,
        })
    }
}

impl Sizable for ColorPicker {
    fn with_size(mut self, size: impl Into<Size>) -> Self {
        self.size = size.into();
        self
    }
}

impl Focusable for ColorPicker {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle.clone()
    }
}

impl Styled for ColorPicker {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for ColorPicker {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let (bounds, current_value, is_open, is_dragging, is_focused, focus_handle, palette_switcher_open) = {
            let state = self.state.read(cx);
            (
                state.bounds,
                state.value,
                state.open,
                state.active_drag.is_some(),
                state.focus_handle.is_focused(window),
                state.focus_handle.clone().tab_stop(true),
                state.palette_switcher_open,
            )
        };
        let display_title: SharedString = if let Some(value) = current_value {
            value.to_hex()
        } else {
            "".to_string()
        }
        .into();

        div()
            .id(self.id.clone())
            .key_context(CONTEXT)
            .track_focus(&focus_handle)
            .on_action(window.listener_for(&self.state, ColorPickerState::on_escape))
            .on_action(window.listener_for(&self.state, ColorPickerState::on_confirm))
            .child(
                h_flex()
                    .id("color-picker-input")
                    .gap_2()
                    .items_center()
                    .input_text_size(self.size)
                    .line_height(relative(1.))
                    .refine_style(&self.style)
                    .when_some(self.icon.clone(), |this, icon| {
                        this.child(
                            Button::new("btn")
                                .track_focus(&focus_handle)
                                .ghost()
                                .selected(is_open)
                                .with_size(self.size)
                                .icon(icon.clone()),
                        )
                    })
                    .when_none(&self.icon, |this| {
                        this.child(
                            div()
                                .id("color-picker-square")
                                .bg(cx.theme().background)
                                .border_1()
                                .m_1()
                                .border_color(cx.theme().input)
                                .rounded(cx.theme().radius)
                                .shadow_xs()
                                .rounded(cx.theme().radius)
                                .overflow_hidden()
                                .size_with(self.size)
                                .when_some(current_value, |this, value| {
                                    this.bg(value)
                                        .border_color(value.darken(0.3))
                                        .when(is_open, |this| this.border_2())
                                })
                                .when(!display_title.is_empty(), |this| {
                                    this.tooltip(move |_, cx| {
                                        cx.new(|_| Tooltip::new(display_title.clone())).into()
                                    })
                                }),
                        )
                        .focus_ring(is_focused, px(0.), window, cx)
                    })
                    .when_some(self.label.clone(), |this, label| this.child(label))
                    .on_click(window.listener_for(&self.state, ColorPickerState::toggle_picker))
                    .child(
                        canvas(
                            {
                                let state = self.state.clone();
                                move |bounds, _, cx| state.update(cx, |r, _| r.bounds = bounds)
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    ),
            )
            .when(is_open, |this| {
                this.child(
                    deferred(
                        anchored()
                            .anchor(self.anchor)
                            .snap_to_window_with_margin(px(8.))
                            .position(self.resolved_corner(bounds))
                            .child(
                                div()
                                    .occlude()
                                    .map(|this| match self.anchor {
                                        Corner::TopLeft | Corner::TopRight => this.mt_1p5(),
                                        Corner::BottomLeft | Corner::BottomRight => this.mb_1p5(),
                                    })
                                    .w(px(480.0))
                                    .rounded(cx.theme().radius)
                                    .p_3()
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .shadow_lg()
                                    .rounded(cx.theme().radius)
                                    .bg(cx.theme().background)
                                    .relative()
                                    .child(
                                        v_flex()
                                            .w_full()
                                            .gap_3()
                                            .child(self.render_advanced_picker(window, cx))
                                    )
                                    .on_mouse_up_out(
                                        MouseButton::Left,
                                        window.listener_for(&self.state, |state, _, window, cx| {
                                            if state.active_drag.is_some() {
                                                state.active_drag = None;
                                                cx.notify();
                                            } else {
                                                state.on_escape(&Cancel, window, cx);
                                            }
                                        }),
                                    ),
                            ),
                    )
                    .with_priority(1),
                )
                .when(palette_switcher_open, |this| {
                    this.child(self.render_palette_switcher_popout(window, cx))
                })
                .when(is_dragging, |this| {
                    this.child(
                        deferred(
                            anchored().snap_to_window_with_margin(px(0.)).child(
                                div()
                                    .size_full()
                                    .on_mouse_move(window.listener_for(
                                        &self.state,
                                        |picker, event: &MouseMoveEvent, window, cx| {
                                            if picker.active_drag.is_some() {
                                                picker.drag_move(event.position, window, cx);
                                            }
                                        },
                                    ))
                                    .on_mouse_up(
                                        MouseButton::Left,
                                        window.listener_for(
                                            &self.state,
                                            ColorPickerState::stop_drag_mouse,
                                        ),
                                    )
                                    .on_mouse_up_out(
                                        MouseButton::Left,
                                        window.listener_for(
                                            &self.state,
                                            ColorPickerState::stop_drag_mouse,
                                        ),
                                    ),
                            ),
                        )
                        .with_priority(3),
                    )
                })
            })
    }
}
