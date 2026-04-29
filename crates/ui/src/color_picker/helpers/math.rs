use super::*;

pub(crate) fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

pub(crate) fn hsv_to_rgb(h: f32, s: f32, v: f32) -> (f32, f32, f32) {
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

pub(crate) fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
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

pub(crate) fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
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

    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    (hue, saturation, max)
}

pub(crate) fn hsva_to_hsla(h: f32, s: f32, v: f32, a: f32) -> Hsla {
    let (r, g, b) = hsv_to_rgb(h, s, v);
    gpui::Rgba {
        r,
        g,
        b,
        a: clamp01(a),
    }
    .into()
}

pub(crate) fn hsla_to_hsva(color: Hsla) -> (f32, f32, f32, f32) {
    let rgba: gpui::Rgba = color.into();
    let (h, s, v) = rgb_to_hsv(rgba.r, rgba.g, rgba.b);
    (h, s, v, rgba.a)
}

pub(crate) fn triangle_vertices(geometry: PickerGeometry, hue: f32) -> [(f32, f32); 3] {
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

pub(crate) fn barycentric(
    p: (f32, f32),
    a: (f32, f32),
    b: (f32, f32),
    c: (f32, f32),
) -> (f32, f32, f32) {
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

pub(crate) fn point_in_triangle(weights: (f32, f32, f32)) -> bool {
    weights.0 >= 0.0 && weights.1 >= 0.0 && weights.2 >= 0.0
}

pub(crate) fn closest_point_on_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> (f32, f32) {
    let ab = (b.0 - a.0, b.1 - a.1);
    let ap = (p.0 - a.0, p.1 - a.1);
    let ab_len_sq = ab.0 * ab.0 + ab.1 * ab.1;
    if ab_len_sq <= f32::EPSILON {
        return a;
    }

    let t = clamp01((ap.0 * ab.0 + ap.1 * ab.1) / ab_len_sq);
    (a.0 + ab.0 * t, a.1 + ab.1 * t)
}

pub(crate) fn clamp_point_to_triangle(
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

pub(crate) fn alpha_to_text(alpha: f32) -> String {
    format!("{alpha:.3}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}
