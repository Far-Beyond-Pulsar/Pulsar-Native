//! What is drawn on the canvas besides the widget itself: the screen, the
//! grid, outlines, selection handles, anchors, guides and drop previews.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::*;

use crate::model::{NodeId, Rect, Vec2};

use super::super::canvas::geometry::{Guide, GuideAxis, Handle};
use super::super::canvas::metrics::paint_text;
use super::super::canvas::paint::{Painter, Surface};
use super::super::frame::Frame;
use crate::editor::canvas::paint::hsla;

/// Size of a selection handle, in canvas pixels.
pub const HANDLE_SIZE: f32 = 9.0;

/// Where a dragged item would land.
#[derive(Clone, Debug, PartialEq)]
pub struct DropPreview {
    /// The container that would take it, in layout space.
    pub container: Rect,
    /// A line showing the position among a box's children (layout space).
    pub line: Option<Rect>,
    /// For canvases: where the item's corner would be (layout space).
    pub at: Option<Vec2>,
    pub label: String,
}

/// Theme colours the overlay uses.
#[derive(Clone, Copy)]
pub struct Colors {
    pub background: Hsla,
    pub screen: Hsla,
    pub accent: Hsla,
    pub muted: Hsla,
    pub text: Hsla,
    pub warning: Hsla,
    pub danger: Hsla,
    pub popover: Hsla,
}

/// The view, as the overlay needs it.
#[derive(Clone, Copy, Debug)]
pub struct ViewShot {
    pub scale: f32,
    pub pan: Vec2,
    pub grid: bool,
    pub grid_size: f32,
    pub outlines: bool,
    pub safe_zone: bool,
}

pub struct OverlayData {
    pub frame: Arc<Frame>,
    pub view: ViewShot,
    pub selection: Vec<NodeId>,
    pub primary: Option<NodeId>,
    pub primary_name: String,
    pub hover: Option<NodeId>,
    pub locked: HashSet<NodeId>,
    /// Rectangle of the primary's parent, to outline it.
    pub parent_rect: Option<Rect>,
    /// Whether the primary can be resized by its handles.
    pub resizable: bool,
    /// Anchors of the primary canvas child: `(min, max)` and its panel.
    pub anchors: Option<(Vec2, Vec2, Rect)>,
    pub guides: Vec<Guide>,
    pub marquee: Option<Rect>,
    pub drop: Option<DropPreview>,
    pub colors: Colors,
    pub font_family: SharedString,
    pub project_root: PathBuf,
}

fn px_rect(origin: Point<Pixels>, r: Rect) -> Bounds<Pixels> {
    Bounds::new(point(origin.x + px(r.x), origin.y + px(r.y)), size(px(r.w), px(r.h)))
}

/// A layout-space rectangle in canvas pixels.
pub fn to_canvas_rect(view: &ViewShot, r: Rect) -> Rect {
    Rect::new(r.x * view.scale + view.pan.x, r.y * view.scale + view.pan.y, r.w * view.scale, r.h * view.scale)
}

/// The handle squares for a selection rectangle given in canvas pixels.
pub fn handle_bounds(rect: Rect) -> Vec<(Handle, Rect)> {
    Handle::ALL
        .into_iter()
        .map(|h| {
            let p = h.position(rect);
            (h, Rect::new(p.x - HANDLE_SIZE * 0.5, p.y - HANDLE_SIZE * 0.5, HANDLE_SIZE, HANDLE_SIZE))
        })
        .collect()
}

fn outline(window: &mut Window, bounds: Bounds<Pixels>, color: Hsla, width: f32, dashed: bool) {
    window.paint_quad(PaintQuad {
        bounds,
        corner_radii: Corners::all(px(0.0)),
        background: gpui::transparent_black().into(),
        border_widths: Edges::all(px(width)),
        border_color: color,
        border_style: if dashed { BorderStyle::Dashed } else { BorderStyle::Solid },
    });
}

fn fill(window: &mut Window, bounds: Bounds<Pixels>, color: Hsla) {
    window.paint_quad(gpui::fill(bounds, color));
}

fn line(window: &mut Window, a: Point<Pixels>, b: Point<Pixels>, width: f32, color: Hsla) {
    let mut path = PathBuilder::stroke(px(width));
    path.move_to(a);
    path.line_to(b);
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

pub fn paint_designer(bounds: Bounds<Pixels>, data: &OverlayData, window: &mut Window, cx: &mut App) {
    let origin = bounds.origin;
    let view = data.view;
    let colors = data.colors;
    let surface = Surface { origin, scale: view.scale, pan: view.pan };

    // The pasteboard.
    fill(window, bounds, colors.background);

    // The screen the widget is drawn on, with a soft shadow.
    let screen = Rect::new(0.0, 0.0, data.frame.size.x, data.frame.size.y);
    let screen_canvas = to_canvas_rect(&view, screen);
    window.paint_shadows(
        px_rect(origin, screen_canvas),
        Corners::all(px(0.0)),
        &[BoxShadow { color: gpui::black().opacity(0.45), offset: point(px(0.0), px(6.0)), blur_radius: px(24.0), spread_radius: px(0.0) }],
    );
    fill(window, px_rect(origin, screen_canvas), colors.screen);

    // Everything on the screen is clipped to it, as in the running game.
    let mask = ContentMask { bounds: px_rect(origin, screen_canvas) };
    window.with_content_mask(Some(mask), |window| {
        if view.grid {
            paint_grid(window, origin, &view, screen_canvas, colors);
        }
        let painter = Painter {
            surface,
            font_family: data.font_family.clone(),
            project_root: &data.project_root,
            problem: colors.danger,
            hint: colors.muted,
            text: colors.text,
        };
        painter.paint(&data.frame.cmds, window, cx);
    });

    outline(window, px_rect(origin, screen_canvas), colors.muted.opacity(0.5), 1.0, false);

    // Safe zone: the part of the screen a console TV is guaranteed to show.
    if view.safe_zone {
        for (inset, color) in [(0.05, colors.warning), (0.10, colors.danger)] {
            let r = Rect::new(screen.w * inset, screen.h * inset, screen.w * (1.0 - 2.0 * inset), screen.h * (1.0 - 2.0 * inset));
            outline(window, px_rect(origin, to_canvas_rect(&view, r)), color.opacity(0.7), 1.0, true);
        }
    }

    // Every widget's outline, when asked.
    if view.outlines {
        for (id, rect) in &data.frame.screen {
            if *id == NodeId(0) {
                continue;
            }
            outline(window, px_rect(origin, to_canvas_rect(&view, *rect)), colors.muted.opacity(0.35), 1.0, false);
        }
    }

    // The container the selection sits in.
    if let Some(parent) = data.parent_rect {
        outline(window, px_rect(origin, to_canvas_rect(&view, parent)), colors.muted.opacity(0.8), 1.0, true);
    }

    // Hover.
    if let Some(rect) = data.hover.filter(|h| !data.selection.contains(h)).and_then(|h| data.frame.screen.get(&h)) {
        let b = px_rect(origin, to_canvas_rect(&view, *rect));
        fill(window, b, colors.accent.opacity(0.08));
        outline(window, b, colors.accent.opacity(0.7), 1.0, false);
    }

    // Selection.
    for id in &data.selection {
        let Some(rect) = data.frame.screen.get(id) else { continue };
        let canvas = to_canvas_rect(&view, *rect);
        let primary = data.primary == Some(*id);
        let color = if data.locked.contains(id) { colors.warning } else { colors.accent };
        outline(window, px_rect(origin, canvas), color.opacity(if primary { 1.0 } else { 0.75 }), if primary { 1.5 } else { 1.0 }, false);
    }
    if let Some(rect) = data.primary.and_then(|p| data.frame.screen.get(&p)) {
        let canvas = to_canvas_rect(&view, *rect);
        paint_label(window, cx, origin, canvas, &data.primary_name, rect, colors, &data.font_family);
        if data.selection.len() == 1 {
            paint_handles(window, origin, canvas, data.resizable, colors);
        }
        if let Some((amin, amax, panel)) = data.anchors {
            paint_anchors(window, origin, &view, amin, amax, panel, canvas, colors);
        }
    }

    // Snap guides.
    for guide in &data.guides {
        let (a, b) = match guide.axis {
            GuideAxis::X => (Vec2::new(guide.at, guide.from), Vec2::new(guide.at, guide.to)),
            GuideAxis::Y => (Vec2::new(guide.from, guide.at), Vec2::new(guide.to, guide.at)),
        };
        let guide_color = hsla(crate::model::Color::rgba(1.0, 0.25, 0.75, 0.9));
        line(window, surface.point(a), surface.point(b), 1.0, guide_color);
    }

    // Marquee.
    if let Some(area) = data.marquee {
        let b = px_rect(origin, to_canvas_rect(&view, area));
        fill(window, b, colors.accent.opacity(0.12));
        outline(window, b, colors.accent.opacity(0.9), 1.0, false);
    }

    // Where a drop would land.
    if let Some(drop) = &data.drop {
        let container = px_rect(origin, to_canvas_rect(&view, drop.container));
        outline(window, container, colors.accent, 2.0, false);
        fill(window, container, colors.accent.opacity(0.06));
        if let Some(line_rect) = drop.line {
            fill(window, px_rect(origin, to_canvas_rect(&view, line_rect)), colors.accent);
        }
        if let Some(at) = drop.at {
            let p = surface.point(at);
            let marker = Bounds::new(point(p.x - px(4.0), p.y - px(4.0)), size(px(8.0), px(8.0)));
            window.paint_quad(PaintQuad {
                bounds: marker,
                corner_radii: Corners::all(px(4.0)),
                background: colors.accent.into(),
                border_widths: Edges::all(px(1.0)),
                border_color: gpui::white(),
                border_style: BorderStyle::Solid,
            });
        }
        if !drop.label.is_empty() {
            let c = to_canvas_rect(&view, drop.container);
            paint_text(
                window,
                cx,
                &data.font_family,
                px_rect(origin, Rect::new(c.x + 6.0, c.y + c.h + 4.0, 320.0, 16.0)),
                &drop.label,
                11.0,
                "medium",
                false,
                colors.accent,
                TextAlign::Left,
                false,
                1.2,
            );
        }
    }
}

fn paint_grid(window: &mut Window, origin: Point<Pixels>, view: &ViewShot, screen: Rect, colors: Colors) {
    let step = view.grid_size * view.scale;
    // Skip the grid when it would be a smear.
    if step < 5.0 {
        return;
    }
    let minor = colors.text.opacity(0.045);
    let major = colors.text.opacity(0.09);
    let mut n = 0u32;
    let mut x = screen.x;
    while x <= screen.right() && n < 4000 {
        let c = if n.is_multiple_of(8) { major } else { minor };
        fill(window, px_rect(origin, Rect::new(x, screen.y, 1.0, screen.h)), c);
        x += step;
        n += 1;
    }
    let mut n = 0u32;
    let mut y = screen.y;
    while y <= screen.bottom() && n < 4000 {
        let c = if n.is_multiple_of(8) { major } else { minor };
        fill(window, px_rect(origin, Rect::new(screen.x, y, screen.w, 1.0)), c);
        y += step;
        n += 1;
    }
}

fn paint_handles(window: &mut Window, origin: Point<Pixels>, canvas: Rect, active: bool, colors: Colors) {
    for (_, h) in handle_bounds(canvas) {
        window.paint_quad(PaintQuad {
            bounds: px_rect(origin, h),
            corner_radii: Corners::all(px(2.0)),
            background: if active { gpui::white().into() } else { colors.popover.into() },
            border_widths: Edges::all(px(1.5)),
            border_color: if active { colors.accent } else { colors.muted },
            border_style: BorderStyle::Solid,
        });
    }
}

/// The name of the selected widget and its size, above its top-left corner.
#[allow(clippy::too_many_arguments)]
fn paint_label(window: &mut Window, cx: &mut App, origin: Point<Pixels>, canvas: Rect, name: &str, rect: &Rect, colors: Colors, family: &SharedString) {
    let text = format!("{name}  {} \u{d7} {}", rect.w.round(), rect.h.round());
    let width = (text.chars().count() as f32 * 6.4 + 12.0).min(360.0);
    let y = if canvas.y > 22.0 { canvas.y - 20.0 } else { canvas.bottom() + 4.0 };
    let chip = Rect::new(canvas.x, y, width, 17.0);
    window.paint_quad(PaintQuad {
        bounds: px_rect(origin, chip),
        corner_radii: Corners::all(px(4.0)),
        background: colors.accent.into(),
        border_widths: Edges::all(px(0.0)),
        border_color: gpui::transparent_black(),
        border_style: BorderStyle::Solid,
    });
    paint_text(
        window,
        cx,
        family,
        px_rect(origin, Rect::new(chip.x + 6.0, chip.y + 2.0, chip.w - 8.0, chip.h)),
        &text,
        11.0,
        "medium",
        false,
        gpui::white(),
        TextAlign::Left,
        false,
        1.2,
    );
}

/// Diamonds at the anchor points on the panel, with lines to the widget.
#[allow(clippy::too_many_arguments)]
fn paint_anchors(window: &mut Window, origin: Point<Pixels>, view: &ViewShot, amin: Vec2, amax: Vec2, panel: Rect, widget: Rect, colors: Colors) {
    let point_at = |a: Vec2| Vec2::new(panel.x + a.x * panel.w, panel.y + a.y * panel.h);
    let corners = [
        point_at(Vec2::new(amin.x, amin.y)),
        point_at(Vec2::new(amax.x, amin.y)),
        point_at(Vec2::new(amax.x, amax.y)),
        point_at(Vec2::new(amin.x, amax.y)),
    ];
    let mut seen: Vec<Vec2> = Vec::new();
    for corner in corners {
        if seen.iter().any(|s| (s.x - corner.x).abs() < 0.01 && (s.y - corner.y).abs() < 0.01) {
            continue;
        }
        seen.push(corner);
        let c = Vec2::new(corner.x * view.scale + view.pan.x, corner.y * view.scale + view.pan.y);
        let center = widget.center();
        let target = Vec2::new(center.x.clamp(widget.x, widget.right()), center.y.clamp(widget.y, widget.bottom()));
        line(
            window,
            point(origin.x + px(c.x), origin.y + px(c.y)),
            point(origin.x + px(target.x), origin.y + px(target.y)),
            1.0,
            colors.warning.opacity(0.55),
        );
        // A diamond.
        let r = 5.0;
        let mut d = PathBuilder::fill();
        d.move_to(point(origin.x + px(c.x), origin.y + px(c.y - r)));
        d.line_to(point(origin.x + px(c.x + r), origin.y + px(c.y)));
        d.line_to(point(origin.x + px(c.x), origin.y + px(c.y + r)));
        d.line_to(point(origin.x + px(c.x - r), origin.y + px(c.y)));
        d.close();
        if let Ok(path) = d.build() {
            window.paint_path(path, colors.warning);
        }
    }
}
