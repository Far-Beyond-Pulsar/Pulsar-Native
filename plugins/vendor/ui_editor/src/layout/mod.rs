//! Laying out a resolved widget tree.
//!
//! Two passes, as in most UI systems: [`measure`] asks each node how big it
//! wants to be given the space on offer, then [`arrange`] hands each node its
//! final rectangle. Rectangles are in the layout space of the root (the
//! widget's own coordinates before any render transform); `world` on each
//! node maps that to the space the widget is drawn in.
//!
//! Text size depends on the font, so the caller supplies [`TextMetrics`]: the
//! editor measures with the real text system, tests with a fixed-width fake.

pub(crate) mod panels;

use crate::model::geom::{Affine, Insets, Rect, Vec2};
use crate::model::schema::Kind;
use crate::resolve::ResolvedNode;

/// What to measure.
#[derive(Clone, Debug)]
pub struct TextRequest {
    pub text: String,
    pub size: f32,
    pub weight: String,
    pub italic: bool,
    /// Wrap at this width; `None` for one line per paragraph.
    pub wrap_width: Option<f32>,
    /// Line height as a multiple of the size.
    pub line_height: f32,
}

pub trait TextMetrics {
    /// The width and height the text takes.
    fn measure(&self, request: &TextRequest) -> Vec2;
}

/// A fake with fixed-width glyphs, for tests and headless tools.
pub struct FixedMetrics;

impl TextMetrics for FixedMetrics {
    fn measure(&self, r: &TextRequest) -> Vec2 {
        let advance = r.size * 0.5;
        let line = r.size * r.line_height;
        let mut lines = 0usize;
        let mut widest = 0.0f32;
        for paragraph in r.text.split('\n') {
            let chars = paragraph.chars().count();
            let width = chars as f32 * advance;
            match r.wrap_width.filter(|w| *w > 0.0 && width > *w) {
                Some(limit) => {
                    let per_line = ((limit / advance).floor() as usize).max(1);
                    lines += chars.div_ceil(per_line);
                    widest = widest.max(per_line as f32 * advance);
                }
                None => {
                    lines += 1;
                    widest = widest.max(width);
                }
            }
        }
        Vec2::new(widest, lines.max(1) as f32 * line)
    }
}

/// Size of an unfilled named slot in the widget that declares it, so it can be seen.
pub const EMPTY_SLOT_SIZE: Vec2 = Vec2::new(120.0, 36.0);
/// Size of an instance that can't be resolved.
pub const PLACEHOLDER_SIZE: Vec2 = Vec2::new(160.0, 48.0);
const INF: f32 = f32::INFINITY;

/// Lay out `root` to fill `viewport`.
pub fn layout(root: &mut ResolvedNode, viewport: Vec2, metrics: &dyn TextMetrics) {
    measure(root, viewport, metrics);
    arrange(root, Rect::new(0.0, 0.0, viewport.x, viewport.y), Affine::IDENTITY);
    finalize(root, Affine::IDENTITY);
}

/// Fill in `world` down the tree.
fn finalize(node: &mut ResolvedNode, parent: Affine) {
    node.world = parent.after(&node.local);
    let world = node.world;
    for child in &mut node.children {
        finalize(child, world);
    }
}

pub fn is_collapsed(node: &ResolvedNode) -> bool {
    node.design_hidden || node.view().choice("visibility") == "collapsed"
}

/// Whether the node is drawn (it may still take up space).
pub fn is_drawn(node: &ResolvedNode) -> bool {
    !node.design_hidden && !matches!(node.view().choice("visibility").as_str(), "collapsed" | "hidden")
}

/// Whether clicks can land on the node.
pub fn is_hit_testable(node: &ResolvedNode) -> bool {
    !node.design_hidden && matches!(node.view().choice("visibility").as_str(), "visible")
}

/// A node's slot padding.
pub(crate) fn pad(node: &ResolvedNode) -> Insets {
    node.slot_view().insets("padding")
}

/// Children in the order they are drawn: canvas children by `z_order`
/// (stable), everything else as written.
pub fn draw_order(node: &ResolvedNode) -> Vec<&ResolvedNode> {
    let mut children: Vec<&ResolvedNode> = node.children.iter().collect();
    if node.kind == Kind::Canvas {
        children.sort_by_key(|c| c.slot_view().i32("z_order"));
    }
    children
}

// ── Measure ────────────────────────────────────────────────────────────────

/// Work out `node.desired` (and its children's) given `avail` space. A
/// dimension may be infinite: the node can be as big as it likes.
pub fn measure(node: &mut ResolvedNode, avail: Vec2, m: &dyn TextMetrics) {
    if is_collapsed(node) {
        node.desired = Vec2::ZERO;
        return;
    }
    node.desired = match node.kind {
        Kind::Canvas => panels::measure_canvas(node, avail, m),
        Kind::VerticalBox => panels::measure_box(node, avail, false, m),
        Kind::HorizontalBox => panels::measure_box(node, avail, true, m),
        Kind::ScrollBox => panels::measure_scroll(node, avail, m),
        Kind::Overlay | Kind::NamedSlot => measure_stack(node, avail, m),
        Kind::Grid => panels::measure_grid(node, avail, m),
        Kind::WrapBox => panels::measure_wrap(node, avail, m),
        Kind::SizeBox => measure_size_box(node, avail, m),
        Kind::ScaleBox | Kind::Border => measure_single(node, avail, Insets::ZERO, m),
        Kind::Button => {
            let padding = node.view().insets("padding");
            measure_single(node, avail, padding, m)
        }
        Kind::Instance => match node.children.first() {
            Some(_) => measure_single(node, avail, Insets::ZERO, m),
            None => PLACEHOLDER_SIZE,
        },
        Kind::Text => text_size(node, avail.x, m),
        Kind::Image => node.view().vec2("image_size"),
        Kind::Spacer => node.view().vec2("size"),
        Kind::CheckBox => {
            let v = node.view();
            let label = m.measure(&TextRequest {
                text: v.text("label"),
                size: v.f32("font_size"),
                weight: "regular".into(),
                italic: false,
                wrap_width: None,
                line_height: 1.25,
            });
            let b = v.f32("box_size");
            Vec2::new(b + v.f32("spacing") + label.x, b.max(label.y))
        }
        Kind::Slider => {
            let v = node.view();
            let thickness = v.f32("handle_size").max(v.f32("thickness"));
            if v.choice("orientation") == "vertical" {
                Vec2::new(thickness, 160.0)
            } else {
                Vec2::new(160.0, thickness)
            }
        }
        Kind::ProgressBar => {
            let v = node.view();
            if v.choice("orientation") == "vertical" {
                Vec2::new(v.f32("thickness"), 160.0)
            } else {
                Vec2::new(160.0, v.f32("thickness"))
            }
        }
        Kind::TextInput => {
            let v = node.view();
            let p = v.insets("padding");
            let size = v.f32("font_size");
            let lines = if v.bool("multiline") { 3.0 } else { 1.0 };
            Vec2::new(180.0, size * 1.25 * lines + p.vertical())
        }
        Kind::ComboBox => {
            let v = node.view();
            let p = v.insets("padding");
            let size = v.f32("font_size");
            let widest = v
                .text("options")
                .lines()
                .map(|line| {
                    m.measure(&TextRequest {
                        text: line.to_string(),
                        size,
                        weight: "regular".into(),
                        italic: false,
                        wrap_width: None,
                        line_height: 1.25,
                    })
                    .x
                })
                .fold(0.0, f32::max);
            Vec2::new(widest + p.horizontal() + 28.0, size * 1.25 + p.vertical())
        }
        Kind::Unknown => Vec2::ZERO,
    };
    if node.is_empty_slot && node.own && node.kind == Kind::NamedSlot {
        node.desired = EMPTY_SLOT_SIZE;
    }
}

fn text_size(node: &ResolvedNode, avail_w: f32, m: &dyn TextMetrics) -> Vec2 {
    let v = node.view();
    let wrap = v.bool("wrap") && avail_w.is_finite() && avail_w > 0.0;
    m.measure(&TextRequest {
        text: v.text("text"),
        size: v.f32("font_size"),
        weight: v.choice("weight"),
        italic: v.bool("italic"),
        wrap_width: wrap.then_some(avail_w),
        line_height: v.f32("line_height"),
    })
}

/// A node with at most one child, plus `extra` padding of its own.
fn measure_single(node: &mut ResolvedNode, avail: Vec2, extra: Insets, m: &dyn TextMetrics) -> Vec2 {
    let mut size = Vec2::new(extra.horizontal(), extra.vertical());
    if let Some(child) = node.children.iter_mut().find(|c| !is_collapsed(c)) {
        let p = pad(child);
        let inner = Vec2::new(avail.x - extra.horizontal() - p.horizontal(), avail.y - extra.vertical() - p.vertical());
        measure(child, inner, m);
        size.x += child.desired.x + p.horizontal();
        size.y += child.desired.y + p.vertical();
    }
    size
}

/// Children stacked on one another; as big as the biggest.
fn measure_stack(node: &mut ResolvedNode, avail: Vec2, m: &dyn TextMetrics) -> Vec2 {
    let mut size = Vec2::ZERO;
    for child in node.children.iter_mut().filter(|c| !is_collapsed(c)) {
        let p = pad(child);
        measure(child, Vec2::new(avail.x - p.horizontal(), avail.y - p.vertical()), m);
        size.x = size.x.max(child.desired.x + p.horizontal());
        size.y = size.y.max(child.desired.y + p.vertical());
    }
    size
}

fn measure_size_box(node: &mut ResolvedNode, avail: Vec2, m: &dyn TextMetrics) -> Vec2 {
    let (w, h, min_w, min_h, max_w, max_h) = {
        let v = node.view();
        (v.opt_f32("width"), v.opt_f32("height"), v.opt_f32("min_width"), v.opt_f32("min_height"), v.opt_f32("max_width"), v.opt_f32("max_height"))
    };
    let inner_avail = Vec2::new(w.or(max_w).unwrap_or(avail.x).min(avail.x), h.or(max_h).unwrap_or(avail.y).min(avail.y));
    let content = measure_single(node, inner_avail, Insets::ZERO, m);
    let clamp = |value: f32, forced: Option<f32>, lo: Option<f32>, hi: Option<f32>| {
        let mut v = forced.unwrap_or(value);
        if let Some(lo) = lo {
            v = v.max(lo);
        }
        if let Some(hi) = hi {
            v = v.min(hi);
        }
        v
    };
    Vec2::new(clamp(content.x, w, min_w, max_w), clamp(content.y, h, min_h, max_h))
}

// ── Arrange ────────────────────────────────────────────────────────────────

/// Give `node` its rectangle (in layout space) and arrange what is inside.
/// `extra` is a transform a parent applies to it (a scale box's scaling).
pub fn arrange(node: &mut ResolvedNode, rect: Rect, extra: Affine) {
    node.rect = rect;
    let v = node.view();
    let (translate, scale, pivot) = (v.vec2("translate"), v.vec2("scale"), v.vec2("pivot"));
    let pivot_abs = Vec2::new(rect.x + pivot.x * rect.w, rect.y + pivot.y * rect.h);
    node.local = extra.after(&Affine::about(pivot_abs, scale, translate));

    match node.kind {
        Kind::Canvas => panels::arrange_canvas(node),
        Kind::VerticalBox => panels::arrange_box(node, false),
        Kind::HorizontalBox => panels::arrange_box(node, true),
        Kind::ScrollBox => panels::arrange_scroll(node),
        Kind::Overlay | Kind::NamedSlot | Kind::Instance => arrange_stack(node, rect),
        Kind::Grid => panels::arrange_grid(node),
        Kind::WrapBox => panels::arrange_wrap(node),
        Kind::SizeBox | Kind::Border => arrange_stack(node, rect),
        Kind::Button => {
            let inner = rect.inset(node.view().insets("padding"));
            arrange_stack(node, inner)
        }
        Kind::ScaleBox => arrange_scale_box(node),
        _ => {}
    }
}

/// Place every child in `area`, each by its own padding and alignment.
fn arrange_stack(node: &mut ResolvedNode, area: Rect) {
    for child in node.children.iter_mut().filter(|c| !is_collapsed(c)) {
        let cell = place_in(area, child);
        arrange(child, cell, Affine::IDENTITY);
    }
}

/// Where `child` goes in `area`: inside its padding, aligned by its slot.
pub(crate) fn place_in(area: Rect, child: &ResolvedNode) -> Rect {
    let inner = area.inset(pad(child));
    let v = child.slot_view();
    place(inner, child.desired, &v.choice("h_align"), &v.choice("v_align"))
}

/// `desired` placed in `inner` by alignment (`fill` takes all the space).
pub(crate) fn place(inner: Rect, desired: Vec2, h: &str, v: &str) -> Rect {
    let (x, w) = align_axis(inner.x, inner.w, desired.x, h, "left", "right");
    let (y, hh) = align_axis(inner.y, inner.h, desired.y, v, "top", "bottom");
    Rect::new(x, y, w, hh)
}

fn align_axis(start: f32, space: f32, desired: f32, align: &str, low: &str, high: &str) -> (f32, f32) {
    if align == low {
        (start, desired)
    } else if align == high {
        (start + space - desired, desired)
    } else if align == "center" {
        (start + (space - desired) * 0.5, desired)
    } else {
        (start, space)
    }
}

fn arrange_scale_box(node: &mut ResolvedNode) {
    let rect = node.rect;
    let (stretch, user) = {
        let v = node.view();
        (v.choice("stretch"), v.f32("user_scale"))
    };
    for child in node.children.iter_mut().filter(|c| !is_collapsed(c)) {
        let p = pad(child);
        let natural = Vec2::new(child.desired.x + p.horizontal(), child.desired.y + p.vertical());
        let k = Vec2::new(
            if natural.x > 0.0 { rect.w / natural.x } else { 1.0 },
            if natural.y > 0.0 { rect.h / natural.y } else { 1.0 },
        );
        let contain = k.x.min(k.y);
        let scale = match stretch.as_str() {
            "cover" => Vec2::new(k.x.max(k.y), k.x.max(k.y)),
            "fill" => k,
            "scale_down" => Vec2::new(contain.min(1.0), contain.min(1.0)),
            "user" => Vec2::new(user, user),
            _ => Vec2::new(contain, contain),
        };
        // Lay the child out at its natural size, then scale it about its own
        // corner so the result is centred in the box.
        let scaled = Vec2::new(natural.x * scale.x, natural.y * scale.y);
        let target = Vec2::new(rect.x + (rect.w - scaled.x) * 0.5, rect.y + (rect.h - scaled.y) * 0.5);
        let cell = Rect::new(target.x + p.left, target.y + p.top, child.desired.x, child.desired.y);
        arrange(child, cell, Affine::about(target, scale, Vec2::ZERO));
    }
}

#[cfg(test)]
mod tests;
