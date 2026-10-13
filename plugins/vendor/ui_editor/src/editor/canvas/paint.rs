//! Painting a display list with GPUI.

use std::path::Path;

use gpui::*;

use crate::display::{fit_image, Cmd, Fill, Justify, PlaceholderKind};
use crate::model::{Color, Rect, Vec2};

use super::metrics::paint_text;

/// Where and how large the widget is drawn on the canvas.
#[derive(Clone, Copy, Debug)]
pub struct Surface {
    /// The canvas's top-left corner in window pixels.
    pub origin: Point<Pixels>,
    /// Window pixels per widget pixel.
    pub scale: f32,
    /// The widget's corner, in canvas pixels from `origin`.
    pub pan: Vec2,
}

impl Surface {
    pub fn point(&self, p: Vec2) -> Point<Pixels> {
        point(self.origin.x + px(self.pan.x + p.x * self.scale), self.origin.y + px(self.pan.y + p.y * self.scale))
    }

    pub fn bounds(&self, r: Rect) -> Bounds<Pixels> {
        Bounds::new(self.point(r.origin()), size(px(r.w * self.scale), px(r.h * self.scale)))
    }
}

pub fn hsla(c: Color) -> Hsla {
    Rgba { r: c.r, g: c.g, b: c.b, a: c.a }.into()
}

fn corners(r: [f32; 4], k: f32) -> Corners<Pixels> {
    Corners { top_left: px(r[0] * k), top_right: px(r[1] * k), bottom_right: px(r[2] * k), bottom_left: px(r[3] * k) }
}

fn background(fill: Fill) -> Background {
    match fill {
        Fill::Solid(c) => hsla(c).into(),
        Fill::Gradient { angle, from, to } => linear_gradient(angle, linear_color_stop(hsla(from), 0.0), linear_color_stop(hsla(to), 1.0)),
    }
}

/// Everything a painter needs besides the commands.
pub struct Painter<'a> {
    pub surface: Surface,
    pub font_family: SharedString,
    /// Where project-relative image paths are rooted.
    pub project_root: &'a Path,
    /// Colors for placeholders, from the theme.
    pub problem: Hsla,
    pub hint: Hsla,
    pub text: Hsla,
}

impl Painter<'_> {
    pub fn paint(&self, cmds: &[Cmd], window: &mut Window, cx: &mut App) {
        let mut at = 0;
        self.run(cmds, &mut at, window, cx);
    }

    /// Run commands until the clip this call was opened by ends (or the list does).
    fn run(&self, cmds: &[Cmd], at: &mut usize, window: &mut Window, cx: &mut App) {
        while *at < cmds.len() {
            let cmd = &cmds[*at];
            *at += 1;
            match cmd {
                Cmd::PushClip(rect) => {
                    let mask = ContentMask { bounds: self.surface.bounds(*rect) };
                    window.with_content_mask(Some(mask), |window| self.run(cmds, at, window, cx));
                }
                Cmd::PopClip => return,
                other => self.draw(other, window, cx),
            }
        }
    }

    fn draw(&self, cmd: &Cmd, window: &mut Window, cx: &mut App) {
        let k = self.surface.scale;
        match cmd {
            Cmd::PushClip(_) | Cmd::PopClip => {}
            Cmd::Shadow { rect, radii, blur, color } => {
                window.paint_shadows(
                    self.surface.bounds(*rect),
                    corners(*radii, k),
                    &[BoxShadow { color: hsla(*color), offset: point(px(0.0), px(0.0)), blur_radius: px(blur * k), spread_radius: px(0.0) }],
                );
            }
            Cmd::Quad { rect, radii, fill, border } => {
                let (width, color) = border.map_or((0.0, Color::TRANSPARENT), |(w, c)| (w, c));
                window.paint_quad(PaintQuad {
                    bounds: self.surface.bounds(*rect),
                    corner_radii: corners(*radii, k),
                    background: background(*fill),
                    border_widths: Edges::all(px(width * k)),
                    border_color: hsla(color),
                    border_style: BorderStyle::Solid,
                });
            }
            Cmd::Text { rect, text, size, weight, italic, color, justify, wrap, line_height } => {
                let align = match justify {
                    Justify::Left => TextAlign::Left,
                    Justify::Center => TextAlign::Center,
                    Justify::Right => TextAlign::Right,
                };
                paint_text(
                    window,
                    cx,
                    &self.font_family,
                    self.surface.bounds(*rect),
                    text,
                    size * k,
                    weight,
                    *italic,
                    hsla(*color),
                    align,
                    *wrap,
                    *line_height,
                );
            }
            Cmd::Image { rect, path, tint, stretch, radii, natural } => self.draw_image(*rect, path, *tint, stretch, *radii, *natural, window, cx),
            Cmd::Check { rect, color } => {
                let b = self.surface.bounds(*rect);
                let (x, y, w, h) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
                let mut path = PathBuilder::stroke(px((w * 0.16).max(1.5)));
                path.move_to(gpui::point(px(x + w * 0.05), px(y + h * 0.55)));
                path.line_to(gpui::point(px(x + w * 0.38), px(y + h * 0.88)));
                path.line_to(gpui::point(px(x + w * 0.95), px(y + h * 0.15)));
                if let Ok(path) = path.build() {
                    window.paint_path(path, hsla(*color));
                }
            }
            Cmd::Chevron { center, size: s, color } => {
                let c = self.surface.point(*center);
                let (cx0, cy0, r) = (f32::from(c.x), f32::from(c.y), s * k);
                let mut path = PathBuilder::stroke(px((r * 0.32).max(1.2)));
                path.move_to(gpui::point(px(cx0 - r), px(cy0 - r * 0.5)));
                path.line_to(gpui::point(px(cx0), px(cy0 + r * 0.5)));
                path.line_to(gpui::point(px(cx0 + r), px(cy0 - r * 0.5)));
                if let Ok(path) = path.build() {
                    window.paint_path(path, hsla(*color));
                }
            }
            Cmd::Placeholder { rect, label, kind } => {
                let color = if *kind == PlaceholderKind::Problem { self.problem } else { self.hint };
                let b = self.surface.bounds(*rect);
                window.paint_quad(PaintQuad {
                    bounds: b,
                    corner_radii: Corners::all(px(4.0 * k)),
                    background: color.opacity(0.08).into(),
                    border_widths: Edges::all(px(1.0)),
                    border_color: color.opacity(0.9),
                    border_style: BorderStyle::Dashed,
                });
                let pad = 6.0 * k;
                paint_text(
                    window,
                    cx,
                    &self.font_family,
                    Bounds::new(point(b.origin.x + px(pad), b.origin.y + px(pad)), size((b.size.width - px(pad * 2.0)).max(px(0.0)), (b.size.height - px(pad * 2.0)).max(px(0.0)))),
                    label,
                    (12.0 * k).max(1.0),
                    "medium",
                    false,
                    color,
                    TextAlign::Center,
                    true,
                    1.2,
                );
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_image(&self, rect: Rect, path: &str, tint: Color, stretch: &str, radii: [f32; 4], natural: Vec2, window: &mut Window, cx: &mut App) {
        let k = self.surface.scale;
        let image = (!path.is_empty()).then(|| {
            let full = self.project_root.join(path);
            window.use_asset::<ImgResourceLoader>(&Resource::Path(full.into()), cx)
        });
        match image {
            Some(Some(Ok(data))) => {
                let size = data.size(0);
                let natural = Vec2::new(f32::from(size.width.0 as f32), f32::from(size.height.0 as f32));
                let dest = fit_image(rect, natural, stretch);
                let _ = tint;
                // Parts beyond the box (cover mode) are clipped to it.
                let mask = ContentMask { bounds: self.surface.bounds(rect) };
                window.with_content_mask(Some(mask), |window| {
                    let _ = window.paint_image(self.surface.bounds(dest), corners(radii, k), data.clone(), 0, false);
                });
            }
            // Loading: nothing yet; the asset system re-renders when it arrives.
            Some(None) => {}
            // No image chosen or it can't be read: a hatched stand-in.
            _ => {
                let b = self.surface.bounds(fit_image(rect, natural, stretch));
                window.paint_quad(PaintQuad {
                    bounds: b,
                    corner_radii: corners(radii, k),
                    background: self.hint.opacity(0.10).into(),
                    border_widths: Edges::all(px(1.0)),
                    border_color: self.hint.opacity(0.6),
                    border_style: BorderStyle::Dashed,
                });
                let (x, y, w, h) = (f32::from(b.origin.x), f32::from(b.origin.y), f32::from(b.size.width), f32::from(b.size.height));
                let mut line = PathBuilder::stroke(px(1.0));
                line.move_to(gpui::point(px(x), px(y)));
                line.line_to(gpui::point(px(x + w), px(y + h)));
                line.move_to(gpui::point(px(x + w), px(y)));
                line.line_to(gpui::point(px(x), px(y + h)));
                if let Ok(path) = line.build() {
                    window.paint_path(path, self.hint.opacity(0.35));
                }
            }
        }
        let _ = &self.text;
    }
}
