//! Plain geometry and color types shared by the model, layout and painting.

use serde_json::Value as Json;

/// A point or size in widget space.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

impl std::ops::Add for Vec2 {
    type Output = Vec2;
    fn add(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x + o.x, self.y + o.y)
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Vec2;
    fn sub(self, o: Vec2) -> Vec2 {
        Vec2::new(self.x - o.x, self.y - o.y)
    }
}

impl std::ops::Mul<f32> for Vec2 {
    type Output = Vec2;
    fn mul(self, k: f32) -> Vec2 {
        Vec2::new(self.x * k, self.y * k)
    }
}

/// An axis-aligned rectangle: origin and size.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn from_corners(a: Vec2, b: Vec2) -> Self {
        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
        Self::new(x0, y0, x1 - x0, y1 - y0)
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn origin(&self) -> Vec2 {
        Vec2::new(self.x, self.y)
    }

    pub fn size(&self) -> Vec2 {
        Vec2::new(self.w, self.h)
    }

    pub fn center(&self) -> Vec2 {
        Vec2::new(self.x + self.w * 0.5, self.y + self.h * 0.5)
    }

    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.x && p.x < self.right() && p.y >= self.y && p.y < self.bottom()
    }

    pub fn intersects(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }

    pub fn intersection(&self, o: &Rect) -> Option<Rect> {
        let x0 = self.x.max(o.x);
        let y0 = self.y.max(o.y);
        let x1 = self.right().min(o.right());
        let y1 = self.bottom().min(o.bottom());
        (x1 > x0 && y1 > y0).then(|| Rect::new(x0, y0, x1 - x0, y1 - y0))
    }

    pub fn union(&self, o: &Rect) -> Rect {
        let x0 = self.x.min(o.x);
        let y0 = self.y.min(o.y);
        Rect::new(x0, y0, self.right().max(o.right()) - x0, self.bottom().max(o.bottom()) - y0)
    }

    /// Shrunk by `i` on each side; never negative in size.
    pub fn inset(&self, i: Insets) -> Rect {
        Rect::new(
            self.x + i.left,
            self.y + i.top,
            (self.w - i.left - i.right).max(0.0),
            (self.h - i.top - i.bottom).max(0.0),
        )
    }

    pub fn translate(&self, d: Vec2) -> Rect {
        Rect::new(self.x + d.x, self.y + d.y, self.w, self.h)
    }

    /// Scaled about `pivot`, a point in the same space as the rectangle.
    pub fn scale_about(&self, pivot: Vec2, k: Vec2) -> Rect {
        Rect::new(
            pivot.x + (self.x - pivot.x) * k.x,
            pivot.y + (self.y - pivot.y) * k.y,
            self.w * k.x,
            self.h * k.y,
        )
    }

    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
}

/// Space on each side of something.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Insets {
    pub const ZERO: Insets = Insets { left: 0.0, top: 0.0, right: 0.0, bottom: 0.0 };

    pub const fn new(left: f32, top: f32, right: f32, bottom: f32) -> Self {
        Self { left, top, right, bottom }
    }

    pub fn horizontal(&self) -> f32 {
        self.left + self.right
    }

    pub fn vertical(&self) -> f32 {
        self.top + self.bottom
    }
}

/// Straight (non-premultiplied) RGBA, each channel 0..=1.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const WHITE: Color = Color::rgba(1.0, 1.0, 1.0, 1.0);
    pub const BLACK: Color = Color::rgba(0.0, 0.0, 0.0, 1.0);
    pub const TRANSPARENT: Color = Color::rgba(0.0, 0.0, 0.0, 0.0);

    pub const fn rgba(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    /// `#rgb`, `#rrggbb` or `#rrggbbaa`.
    pub fn from_hex(text: &str) -> Option<Color> {
        let hex = text.trim().strip_prefix('#')?;
        if !hex.is_ascii() {
            return None;
        }
        let byte = |s: &str| u8::from_str_radix(s, 16).ok().map(|v| v as f32 / 255.0);
        match hex.len() {
            3 => {
                let n = |i: usize| u8::from_str_radix(&hex[i..i + 1], 16).ok().map(|v| (v * 17) as f32 / 255.0);
                Some(Color::rgba(n(0)?, n(1)?, n(2)?, 1.0))
            }
            6 => Some(Color::rgba(byte(&hex[0..2])?, byte(&hex[2..4])?, byte(&hex[4..6])?, 1.0)),
            8 => Some(Color::rgba(byte(&hex[0..2])?, byte(&hex[2..4])?, byte(&hex[4..6])?, byte(&hex[6..8])?)),
            _ => None,
        }
    }

    /// `#rrggbb`, or `#rrggbbaa` when not fully opaque.
    pub fn to_hex(&self) -> String {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        if q(self.a) == 255 {
            format!("#{:02x}{:02x}{:02x}", q(self.r), q(self.g), q(self.b))
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", q(self.r), q(self.g), q(self.b), q(self.a))
        }
    }

    pub fn with_alpha(self, a: f32) -> Color {
        Color { a, ..self }
    }

    /// The alpha multiplied by `k`.
    pub fn faded(self, k: f32) -> Color {
        Color { a: self.a * k, ..self }
    }

    pub fn lerp(self, o: Color, t: f32) -> Color {
        let m = |a: f32, b: f32| a + (b - a) * t;
        Color::rgba(m(self.r, o.r), m(self.g, o.g), m(self.b, o.b), m(self.a, o.a))
    }

    pub fn is_transparent(&self) -> bool {
        self.a <= 0.0
    }
}

/// A scale then a translation: `p -> p * scale + offset`. Enough for the render
/// transforms widgets have (translate and scale), and closed under composition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Affine {
    pub sx: f32,
    pub sy: f32,
    pub tx: f32,
    pub ty: f32,
}

impl Affine {
    pub const IDENTITY: Affine = Affine { sx: 1.0, sy: 1.0, tx: 0.0, ty: 0.0 };

    /// Scale by `k` about `pivot`, then move by `d`.
    pub fn about(pivot: Vec2, k: Vec2, d: Vec2) -> Affine {
        Affine { sx: k.x, sy: k.y, tx: pivot.x - pivot.x * k.x + d.x, ty: pivot.y - pivot.y * k.y + d.y }
    }

    pub fn apply(&self, p: Vec2) -> Vec2 {
        Vec2::new(p.x * self.sx + self.tx, p.y * self.sy + self.ty)
    }

    pub fn apply_rect(&self, r: Rect) -> Rect {
        let a = self.apply(r.origin());
        Rect::new(a.x, a.y, r.w * self.sx, r.h * self.sy)
    }

    /// Apply `inner` first, then `self`.
    pub fn after(&self, inner: &Affine) -> Affine {
        Affine { sx: self.sx * inner.sx, sy: self.sy * inner.sy, tx: self.sx * inner.tx + self.tx, ty: self.sy * inner.ty + self.ty }
    }

    pub fn inverse(&self) -> Affine {
        let (sx, sy) = (if self.sx == 0.0 { 1.0 } else { 1.0 / self.sx }, if self.sy == 0.0 { 1.0 } else { 1.0 / self.sy });
        Affine { sx, sy, tx: -self.tx * sx, ty: -self.ty * sy }
    }

    /// The mean scale, for sizes that can't stretch unevenly (text, strokes).
    pub fn uniform(&self) -> f32 {
        (self.sx.abs() + self.sy.abs()) * 0.5
    }
}

// ── JSON helpers ───────────────────────────────────────────────────────────

pub fn json_f32(v: &Json) -> Option<f32> {
    v.as_f64().filter(|n| n.is_finite()).map(|n| n as f32)
}

pub fn json_array4(v: &Json) -> Option<[f32; 4]> {
    let a = v.as_array()?;
    if a.len() != 4 {
        return None;
    }
    Some([json_f32(&a[0])?, json_f32(&a[1])?, json_f32(&a[2])?, json_f32(&a[3])?])
}

pub fn json_array2(v: &Json) -> Option<[f32; 2]> {
    let a = v.as_array()?;
    if a.len() != 2 {
        return None;
    }
    Some([json_f32(&a[0])?, json_f32(&a[1])?])
}

/// A float as JSON, rounded to six places so files diff cleanly.
pub fn num(v: f32) -> Json {
    let rounded = (v as f64 * 1e6).round() / 1e6;
    serde_json::Number::from_f64(rounded).map_or(Json::Null, Json::Number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn colors_round_trip_through_hex() {
        assert_eq!(Color::from_hex("#ff8000"), Some(Color::rgba(1.0, 128.0 / 255.0, 0.0, 1.0)));
        assert_eq!(Color::from_hex("#f80").unwrap().to_hex(), "#ff8800");
        assert_eq!(Color::from_hex("#11223380").unwrap().to_hex(), "#11223380");
        assert_eq!(Color::from_hex("112233"), None);
        assert_eq!(Color::from_hex("#12"), None);
        assert_eq!(Color::from_hex("#ééé"), None);
    }

    #[test]
    fn rect_operations() {
        let r = Rect::new(10.0, 20.0, 100.0, 50.0);
        assert!(r.contains(Vec2::new(10.0, 20.0)));
        assert!(!r.contains(Vec2::new(110.0, 20.0)));
        assert_eq!(r.inset(Insets::new(5.0, 5.0, 5.0, 5.0)), Rect::new(15.0, 25.0, 90.0, 40.0));
        assert_eq!(r.inset(Insets::new(80.0, 0.0, 80.0, 0.0)).w, 0.0, "insets never go negative");
        let o = Rect::new(60.0, 0.0, 100.0, 30.0);
        assert_eq!(r.intersection(&o), Some(Rect::new(60.0, 20.0, 50.0, 10.0)));
        assert_eq!(r.intersection(&Rect::new(200.0, 0.0, 5.0, 5.0)), None);
        assert_eq!(r.union(&o), Rect::new(10.0, 0.0, 150.0, 70.0));
        assert_eq!(
            Rect::new(0.0, 0.0, 10.0, 10.0).scale_about(Vec2::new(5.0, 5.0), Vec2::new(2.0, 2.0)),
            Rect::new(-5.0, -5.0, 20.0, 20.0)
        );
    }
}
