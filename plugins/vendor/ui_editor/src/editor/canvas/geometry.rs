//! The arithmetic behind direct manipulation on the canvas: resizing by a
//! handle, snapping to a grid and to neighbours, aligning and distributing,
//! and anchor presets. All in layout space, none of it touching GPUI.

use crate::model::{NodeId, Rect, Vec2};

// ── Handles ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Handle {
    pub const ALL: [Handle; 8] = [
        Handle::TopLeft,
        Handle::Top,
        Handle::TopRight,
        Handle::Right,
        Handle::BottomRight,
        Handle::Bottom,
        Handle::BottomLeft,
        Handle::Left,
    ];

    /// Which edges the handle moves: `(left, right, top, bottom)`.
    pub fn edges(self) -> (bool, bool, bool, bool) {
        match self {
            Handle::TopLeft => (true, false, true, false),
            Handle::Top => (false, false, true, false),
            Handle::TopRight => (false, true, true, false),
            Handle::Right => (false, true, false, false),
            Handle::BottomRight => (false, true, false, true),
            Handle::Bottom => (false, false, false, true),
            Handle::BottomLeft => (true, false, false, true),
            Handle::Left => (true, false, false, false),
        }
    }

    pub fn position(self, r: Rect) -> Vec2 {
        let (l, rt, t, b) = self.edges();
        let x = if l {
            r.x
        } else if rt {
            r.right()
        } else {
            r.x + r.w * 0.5
        };
        let y = if t {
            r.y
        } else if b {
            r.bottom()
        } else {
            r.y + r.h * 0.5
        };
        Vec2::new(x, y)
    }

    pub fn is_corner(self) -> bool {
        matches!(self, Handle::TopLeft | Handle::TopRight | Handle::BottomRight | Handle::BottomLeft)
    }
}

/// `start` resized by dragging `handle` by `delta`.
///
/// `keep_aspect` holds the width:height ratio (corner handles), and
/// `from_center` resizes symmetrically about the centre. Neither dimension
/// goes below `min`.
pub fn resize_rect(start: Rect, handle: Handle, delta: Vec2, keep_aspect: bool, from_center: bool, min: f32) -> Rect {
    let (l, r, t, b) = handle.edges();
    let mul = if from_center { 2.0 } else { 1.0 };
    let mut w = start.w + if r { delta.x * mul } else if l { -delta.x * mul } else { 0.0 };
    let mut h = start.h + if b { delta.y * mul } else if t { -delta.y * mul } else { 0.0 };
    w = w.max(min);
    h = h.max(min);

    if keep_aspect && start.w > 0.0 && start.h > 0.0 {
        let ratio = start.w / start.h;
        if handle.is_corner() {
            // Follow whichever dimension the mouse changed more.
            let scale_w = w / start.w;
            let scale_h = h / start.h;
            if (scale_w - 1.0).abs() >= (scale_h - 1.0).abs() {
                h = (w / ratio).max(min);
                w = h * ratio;
            } else {
                w = (h * ratio).max(min);
                h = w / ratio;
            }
        } else if l || r {
            h = (w / ratio).max(min);
        } else {
            w = (h * ratio).max(min);
        }
    }

    let center = start.center();
    // With the aspect held, an edge handle changes the other axis too, about the centre.
    let grows_x = keep_aspect && matches!(handle, Handle::Top | Handle::Bottom);
    let grows_y = keep_aspect && matches!(handle, Handle::Left | Handle::Right);
    let x = if from_center || grows_x {
        center.x - w * 0.5
    } else if l {
        start.right() - w
    } else {
        start.x
    };
    let y = if from_center || grows_y {
        center.y - h * 0.5
    } else if t {
        start.bottom() - h
    } else {
        start.y
    };
    Rect::new(x, y, w, h)
}

// ── Snapping ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GuideAxis {
    /// A vertical line at `at` (an x).
    X,
    /// A horizontal line at `at` (a y).
    Y,
}

/// A line shown while snapped, running from `from` to `to` along the other axis.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Guide {
    pub axis: GuideAxis,
    pub at: f32,
    pub from: f32,
    pub to: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Snap {
    /// Snap to multiples of this when nothing else is close.
    pub grid: Option<f32>,
    /// How close, in layout units, counts as touching.
    pub tolerance: f32,
}

fn xs(r: &Rect) -> [f32; 3] {
    [r.x, r.x + r.w * 0.5, r.right()]
}

fn ys(r: &Rect) -> [f32; 3] {
    [r.y, r.y + r.h * 0.5, r.bottom()]
}

/// The correction (to add to the moving values) that brings one of `mine` onto
/// one of `lines`, and the line it landed on. `None` if nothing is in range.
fn nearest(mine: &[f32], lines: &[f32], tolerance: f32) -> Option<(f32, f32)> {
    let mut best: Option<(f32, f32)> = None;
    for &m in mine {
        for &line in lines {
            let d = line - m;
            if d.abs() <= tolerance && best.is_none_or(|(b, _)| d.abs() < b.abs()) {
                best = Some((d, line));
            }
        }
    }
    best
}

/// Where a rect dragged to `moving` lands after snapping to `targets` (other
/// widgets) and `bounds` (the container), else to the grid. Returns the
/// correction to apply and the guides to show.
pub fn snap_move(moving: Rect, targets: &[Rect], bounds: Rect, snap: Snap) -> (Vec2, Vec<Guide>) {
    let mut lines_x: Vec<f32> = xs(&bounds).to_vec();
    let mut lines_y: Vec<f32> = ys(&bounds).to_vec();
    for t in targets {
        lines_x.extend(xs(t));
        lines_y.extend(ys(t));
    }
    let mut correction = Vec2::ZERO;
    let mut guides = Vec::new();

    match nearest(&xs(&moving), &lines_x, snap.tolerance) {
        Some((d, at)) => {
            correction.x = d;
            guides.push(guide_x(at, moving.translate(Vec2::new(d, 0.0)), targets, bounds));
        }
        None => {
            if let Some(g) = snap.grid.filter(|g| *g > 0.0) {
                let snapped = (moving.x / g).round() * g;
                if (snapped - moving.x).abs() <= snap.tolerance {
                    correction.x = snapped - moving.x;
                }
            }
        }
    }
    match nearest(&ys(&moving), &lines_y, snap.tolerance) {
        Some((d, at)) => {
            correction.y = d;
            guides.push(guide_y(at, moving.translate(Vec2::new(0.0, d)), targets, bounds));
        }
        None => {
            if let Some(g) = snap.grid.filter(|g| *g > 0.0) {
                let snapped = (moving.y / g).round() * g;
                if (snapped - moving.y).abs() <= snap.tolerance {
                    correction.y = snapped - moving.y;
                }
            }
        }
    }
    (correction, guides)
}

fn guide_x(at: f32, moved: Rect, targets: &[Rect], bounds: Rect) -> Guide {
    let mut from = moved.y.min(bounds.y);
    let mut to = moved.bottom().max(bounds.bottom());
    for t in targets {
        if xs(t).iter().any(|x| (x - at).abs() < 0.01) {
            from = from.min(t.y);
            to = to.max(t.bottom());
        }
    }
    Guide { axis: GuideAxis::X, at, from: from.min(moved.y), to: to.max(moved.bottom()) }
}

fn guide_y(at: f32, moved: Rect, targets: &[Rect], bounds: Rect) -> Guide {
    let mut from = moved.x.min(bounds.x);
    let mut to = moved.right().max(bounds.right());
    for t in targets {
        if ys(t).iter().any(|y| (y - at).abs() < 0.01) {
            from = from.min(t.x);
            to = to.max(t.right());
        }
    }
    Guide { axis: GuideAxis::Y, at, from: from.min(moved.x), to: to.max(moved.right()) }
}

/// `rect` (being resized by `handle`) with its moving edges snapped to
/// `targets`, `bounds` or the grid.
pub fn snap_resize(rect: Rect, handle: Handle, targets: &[Rect], bounds: Rect, snap: Snap) -> (Rect, Vec<Guide>) {
    let (l, r, t, b) = handle.edges();
    let mut lines_x: Vec<f32> = xs(&bounds).to_vec();
    let mut lines_y: Vec<f32> = ys(&bounds).to_vec();
    for target in targets {
        lines_x.extend(xs(target));
        lines_y.extend(ys(target));
    }
    if let Some(g) = snap.grid.filter(|g| *g > 0.0) {
        let near = |v: f32| (v / g).round() * g;
        for v in [rect.x, rect.right()] {
            lines_x.push(near(v));
        }
        for v in [rect.y, rect.bottom()] {
            lines_y.push(near(v));
        }
    }

    let (mut left, mut right, mut top, mut bottom) = (rect.x, rect.right(), rect.y, rect.bottom());
    let mut guides = Vec::new();
    if l {
        if let Some((d, at)) = nearest(&[left], &lines_x, snap.tolerance) {
            left += d;
            guides.push(guide_x(at, Rect::new(left, top, right - left, bottom - top), targets, bounds));
        }
    }
    if r {
        if let Some((d, at)) = nearest(&[right], &lines_x, snap.tolerance) {
            right += d;
            guides.push(guide_x(at, Rect::new(left, top, right - left, bottom - top), targets, bounds));
        }
    }
    if t {
        if let Some((d, at)) = nearest(&[top], &lines_y, snap.tolerance) {
            top += d;
            guides.push(guide_y(at, Rect::new(left, top, right - left, bottom - top), targets, bounds));
        }
    }
    if b {
        if let Some((d, at)) = nearest(&[bottom], &lines_y, snap.tolerance) {
            bottom += d;
            guides.push(guide_y(at, Rect::new(left, top, right - left, bottom - top), targets, bounds));
        }
    }
    (Rect::new(left, top, (right - left).max(0.0), (bottom - top).max(0.0)), guides)
}

// ── Aligning ───────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlignKind {
    Left,
    CenterH,
    Right,
    Top,
    CenterV,
    Bottom,
}

impl AlignKind {
    pub const ALL: [AlignKind; 6] =
        [AlignKind::Left, AlignKind::CenterH, AlignKind::Right, AlignKind::Top, AlignKind::CenterV, AlignKind::Bottom];

    pub fn label(self) -> &'static str {
        match self {
            AlignKind::Left => "Align left",
            AlignKind::CenterH => "Align horizontal centres",
            AlignKind::Right => "Align right",
            AlignKind::Top => "Align top",
            AlignKind::CenterV => "Align vertical centres",
            AlignKind::Bottom => "Align bottom",
        }
    }
}

/// How far each item moves to line up with `primary`.
pub fn align(items: &[(NodeId, Rect)], primary: NodeId, kind: AlignKind) -> Vec<(NodeId, Vec2)> {
    let Some((_, anchor)) = items.iter().find(|(id, _)| *id == primary) else { return Vec::new() };
    items
        .iter()
        .filter(|(id, _)| *id != primary)
        .map(|(id, r)| {
            let d = match kind {
                AlignKind::Left => Vec2::new(anchor.x - r.x, 0.0),
                AlignKind::CenterH => Vec2::new(anchor.center().x - r.center().x, 0.0),
                AlignKind::Right => Vec2::new(anchor.right() - r.right(), 0.0),
                AlignKind::Top => Vec2::new(0.0, anchor.y - r.y),
                AlignKind::CenterV => Vec2::new(0.0, anchor.center().y - r.center().y),
                AlignKind::Bottom => Vec2::new(0.0, anchor.bottom() - r.bottom()),
            };
            (*id, d)
        })
        .collect()
}

/// How far each item moves to space the group evenly between its outermost
/// members. Needs three or more.
pub fn distribute(items: &[(NodeId, Rect)], horizontal: bool) -> Vec<(NodeId, Vec2)> {
    if items.len() < 3 {
        return Vec::new();
    }
    let mut sorted: Vec<&(NodeId, Rect)> = items.iter().collect();
    sorted.sort_by(|a, b| {
        let (ka, kb) = if horizontal { (a.1.x, b.1.x) } else { (a.1.y, b.1.y) };
        ka.total_cmp(&kb)
    });
    let (first, last) = (sorted[0].1, sorted[sorted.len() - 1].1);
    let size = |r: &Rect| if horizontal { r.w } else { r.h };
    let start = |r: &Rect| if horizontal { r.x } else { r.y };
    let end = |r: &Rect| if horizontal { r.right() } else { r.bottom() };
    let total: f32 = sorted.iter().map(|(_, r)| size(r)).sum();
    let gap = (end(&last) - start(&first) - total) / (sorted.len() - 1) as f32;
    let mut cursor = start(&first);
    sorted
        .iter()
        .map(|(id, r)| {
            let d = cursor - start(r);
            cursor += size(r) + gap;
            (*id, if horizontal { Vec2::new(d, 0.0) } else { Vec2::new(0.0, d) })
        })
        .collect()
}

// ── Anchor presets ─────────────────────────────────────────────────────────

/// Where along one axis a canvas child is pinned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pin {
    Start,
    Center,
    End,
    Stretch,
}

impl Pin {
    pub const ALL: [Pin; 4] = [Pin::Start, Pin::Center, Pin::End, Pin::Stretch];

    fn anchors(self) -> (f32, f32) {
        match self {
            Pin::Start => (0.0, 0.0),
            Pin::Center => (0.5, 0.5),
            Pin::End => (1.0, 1.0),
            Pin::Stretch => (0.0, 1.0),
        }
    }

    fn alignment(self) -> f32 {
        match self {
            Pin::Start | Pin::Stretch => 0.0,
            Pin::Center => 0.5,
            Pin::End => 1.0,
        }
    }
}

/// A named way of pinning a canvas child to its panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AnchorPreset {
    pub h: Pin,
    pub v: Pin,
}

impl AnchorPreset {
    pub fn anchors(self) -> (Vec2, Vec2) {
        let (x0, x1) = self.h.anchors();
        let (y0, y1) = self.v.anchors();
        (Vec2::new(x0, y0), Vec2::new(x1, y1))
    }

    /// The alignment that makes the position refer to the pinned corner or edge.
    pub fn alignment(self) -> Vec2 {
        Vec2::new(self.h.alignment(), self.v.alignment())
    }

    pub fn label(self) -> String {
        let h = match self.h {
            Pin::Start => "left",
            Pin::Center => "centre",
            Pin::End => "right",
            Pin::Stretch => "stretch",
        };
        let v = match self.v {
            Pin::Start => "top",
            Pin::Center => "middle",
            Pin::End => "bottom",
            Pin::Stretch => "stretch",
        };
        match (self.h, self.v) {
            (Pin::Stretch, Pin::Stretch) => "Fill the panel".to_string(),
            (Pin::Stretch, _) => format!("Stretch across, pinned {v}"),
            (_, Pin::Stretch) => format!("Stretch down, pinned {h}"),
            _ => format!("Pinned {v} {h}"),
        }
    }

    /// The preset in a row-major 4 x 4 picker: rows top, middle, bottom,
    /// stretch; columns left, centre, right, stretch.
    pub fn at(row: usize, col: usize) -> AnchorPreset {
        AnchorPreset { h: Pin::ALL[col.min(3)], v: [Pin::Start, Pin::Center, Pin::End, Pin::Stretch][row.min(3)] }
    }

    /// The preset whose anchors are `amin`..`amax`, if there is one.
    pub fn matching(amin: Vec2, amax: Vec2) -> Option<AnchorPreset> {
        (0..4).flat_map(|r| (0..4).map(move |c| AnchorPreset::at(r, c))).find(|p| {
            let (a, b) = p.anchors();
            a == amin && b == amax
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u32) -> NodeId {
        NodeId(n)
    }

    #[test]
    fn dragging_a_corner_resizes_from_the_opposite_corner() {
        let r = Rect::new(100.0, 100.0, 200.0, 100.0);
        assert_eq!(resize_rect(r, Handle::BottomRight, Vec2::new(20.0, 10.0), false, false, 1.0), Rect::new(100.0, 100.0, 220.0, 110.0));
        assert_eq!(resize_rect(r, Handle::TopLeft, Vec2::new(20.0, 10.0), false, false, 1.0), Rect::new(120.0, 110.0, 180.0, 90.0));
        assert_eq!(resize_rect(r, Handle::Right, Vec2::new(-50.0, 99.0), false, false, 1.0), Rect::new(100.0, 100.0, 150.0, 100.0), "edge handles move one axis");
        assert_eq!(resize_rect(r, Handle::Bottom, Vec2::new(99.0, 30.0), false, false, 1.0), Rect::new(100.0, 100.0, 200.0, 130.0));
    }

    #[test]
    fn resizing_never_goes_below_the_minimum_and_keeps_the_far_edge_put() {
        let r = Rect::new(100.0, 100.0, 50.0, 50.0);
        let shrunk = resize_rect(r, Handle::TopLeft, Vec2::new(500.0, 500.0), false, false, 4.0);
        assert_eq!((shrunk.w, shrunk.h), (4.0, 4.0));
        assert_eq!((shrunk.right(), shrunk.bottom()), (150.0, 150.0));
    }

    #[test]
    fn holding_the_aspect_ratio_and_resizing_from_the_centre() {
        let r = Rect::new(0.0, 0.0, 200.0, 100.0);
        let k = resize_rect(r, Handle::BottomRight, Vec2::new(100.0, 5.0), true, false, 1.0);
        assert!((k.w / k.h - 2.0).abs() < 1e-4, "{k:?}");
        let c = resize_rect(r, Handle::Right, Vec2::new(20.0, 0.0), false, true, 1.0);
        assert_eq!(c, Rect::new(-20.0, 0.0, 240.0, 100.0));
        let edge = resize_rect(r, Handle::Right, Vec2::new(100.0, 0.0), true, false, 1.0);
        assert!((edge.w / edge.h - 2.0).abs() < 1e-4);
        assert!((edge.center().y - 50.0).abs() < 1e-3, "the other axis grows about the centre");
    }

    #[test]
    fn moving_snaps_to_neighbours_before_the_grid() {
        let bounds = Rect::new(0.0, 0.0, 1000.0, 500.0);
        let other = Rect::new(300.0, 100.0, 100.0, 50.0);
        let snap = Snap { grid: Some(8.0), tolerance: 5.0 };
        // Left edge 3px from the neighbour's right edge (400).
        let (d, guides) = snap_move(Rect::new(403.0, 300.0, 80.0, 40.0), &[other], bounds, snap);
        assert_eq!(d.x, -3.0);
        assert!(guides.iter().any(|g| g.axis == GuideAxis::X && g.at == 400.0));
        // Nothing near vertically except the grid.
        assert_eq!(d.y, 4.0 - 4.0 + (300.0f32 / 8.0).round() * 8.0 - 300.0);
    }

    #[test]
    fn centres_snap_to_the_containers_centre() {
        let bounds = Rect::new(0.0, 0.0, 1000.0, 500.0);
        let snap = Snap { grid: None, tolerance: 6.0 };
        let (d, guides) = snap_move(Rect::new(458.0, 200.0, 80.0, 40.0), &[], bounds, snap);
        assert_eq!(d.x, 2.0, "centre 498 -> 500");
        assert_eq!(guides[0].at, 500.0);
        let (d, guides) = snap_move(Rect::new(300.0, 200.0, 80.0, 40.0), &[], bounds, snap);
        assert_eq!(d, Vec2::ZERO);
        assert!(guides.is_empty());
    }

    #[test]
    fn resizing_snaps_only_the_edges_being_dragged() {
        let bounds = Rect::new(0.0, 0.0, 1000.0, 500.0);
        let snap = Snap { grid: None, tolerance: 6.0 };
        let (r, guides) = snap_resize(Rect::new(100.0, 100.0, 897.0, 100.0), Handle::Right, &[], bounds, snap);
        assert_eq!(r.right(), 1000.0, "right edge snaps to the container");
        assert_eq!(r.x, 100.0);
        assert_eq!(guides.len(), 1);
    }

    #[test]
    fn aligning_moves_everything_to_the_primary() {
        let items = [(id(1), Rect::new(10.0, 10.0, 50.0, 20.0)), (id(2), Rect::new(100.0, 40.0, 30.0, 60.0)), (id(3), Rect::new(200.0, 5.0, 10.0, 10.0))];
        let left = align(&items, id(1), AlignKind::Left);
        assert_eq!(left, vec![(id(2), Vec2::new(-90.0, 0.0)), (id(3), Vec2::new(-190.0, 0.0))]);
        let mid = align(&items, id(2), AlignKind::CenterV);
        assert_eq!(mid[0], (id(1), Vec2::new(0.0, 70.0 - 20.0)));
        assert!(align(&items, id(9), AlignKind::Top).is_empty());
    }

    #[test]
    fn distributing_equalises_the_gaps() {
        let items = [(id(1), Rect::new(0.0, 0.0, 10.0, 10.0)), (id(2), Rect::new(15.0, 0.0, 10.0, 10.0)), (id(3), Rect::new(100.0, 0.0, 10.0, 10.0))];
        let moves = distribute(&items, true);
        let new_x: Vec<f32> = moves.iter().map(|(i, d)| items.iter().find(|(j, _)| j == i).unwrap().1.x + d.x).collect();
        // 110 wide in total, 30 of boxes: two gaps of 40.
        assert_eq!(new_x, vec![0.0, 50.0, 100.0]);
        assert!(distribute(&items[..2], true).is_empty());
    }

    #[test]
    fn anchor_presets_describe_the_sixteen_standard_pins() {
        let tl = AnchorPreset::at(0, 0);
        assert_eq!(tl.anchors(), (Vec2::ZERO, Vec2::ZERO));
        assert_eq!(tl.alignment(), Vec2::ZERO);
        let br = AnchorPreset::at(2, 2);
        assert_eq!(br.anchors(), (Vec2::new(1.0, 1.0), Vec2::new(1.0, 1.0)));
        assert_eq!(br.alignment(), Vec2::new(1.0, 1.0));
        let fill = AnchorPreset::at(3, 3);
        assert_eq!(fill.anchors(), (Vec2::ZERO, Vec2::new(1.0, 1.0)));
        assert_eq!(fill.label(), "Fill the panel");
        assert_eq!(AnchorPreset::matching(Vec2::new(0.5, 0.5), Vec2::new(0.5, 0.5)), Some(AnchorPreset::at(1, 1)));
        assert_eq!(AnchorPreset::matching(Vec2::new(0.2, 0.0), Vec2::new(0.2, 0.0)), None);
    }
}
