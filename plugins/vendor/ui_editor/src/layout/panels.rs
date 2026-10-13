//! Layout of the panels that hold many children.

use crate::model::geom::{Affine, Rect, Vec2};
use crate::resolve::ResolvedNode;

use super::{arrange, is_collapsed, measure, pad, place, TextMetrics, INF};

// ── Canvas ─────────────────────────────────────────────────────────────────

/// A canvas child's rectangle in a parent of size `parent`, from its
/// anchors and offsets. `desired` is used when it sizes to its content.
///
/// On each axis, when the two anchors are the same point the offsets are a
/// position and a size; when they differ the offsets are margins from the
/// anchor edges, so the child stretches with the canvas.
pub fn canvas_slot_rect(child: &ResolvedNode, parent: Rect) -> Rect {
    let v = child.slot_view();
    let (amin, amax) = (v.vec2("anchor_min"), v.vec2("anchor_max"));
    let o = v.quad("offsets");
    let align = v.vec2("alignment");
    let auto = v.bool("auto_size");

    let axis = |lo: f32, hi: f32, start: f32, size_of_parent: f32, off_a: f32, off_b: f32, align: f32, desired: f32| -> (f32, f32) {
        let a = start + lo * size_of_parent;
        let b = start + hi * size_of_parent;
        if (lo - hi).abs() < 1e-6 {
            let size = if auto { desired } else { off_b };
            (a + off_a - align * size, size)
        } else {
            (a + off_a, (b - off_b) - (a + off_a))
        }
    };
    let (x, w) = axis(amin.x, amax.x, parent.x, parent.w, o[0], o[2], align.x, child.desired.x);
    let (y, h) = axis(amin.y, amax.y, parent.y, parent.h, o[1], o[3], align.y, child.desired.y);
    Rect::new(x, y, w.max(0.0), h.max(0.0))
}

pub fn measure_canvas(node: &mut ResolvedNode, avail: Vec2, m: &dyn TextMetrics) -> Vec2 {
    let mut size = Vec2::ZERO;
    for child in node.children.iter_mut().filter(|c| !is_collapsed(c)) {
        // Offer the child the space its slot gives it, so wrapping text
        // inside a fixed-size panel measures at that panel's width.
        let offered = {
            let v = child.slot_view();
            let (amin, amax) = (v.vec2("anchor_min"), v.vec2("anchor_max"));
            let o = v.quad("offsets");
            let auto = v.bool("auto_size");
            let axis = |lo: f32, hi: f32, space: f32, off_a: f32, off_b: f32| {
                if auto {
                    space
                } else if (lo - hi).abs() < 1e-6 {
                    off_b
                } else {
                    ((hi - lo) * space - off_a - off_b).max(0.0)
                }
            };
            Vec2::new(axis(amin.x, amax.x, avail.x, o[0], o[2]), axis(amin.y, amax.y, avail.y, o[1], o[3]))
        };
        measure(child, offered, m);
        let v = child.slot_view();
        let (amin, amax) = (v.vec2("anchor_min"), v.vec2("anchor_max"));
        let o = v.quad("offsets");
        let auto = v.bool("auto_size");
        let align = v.vec2("alignment");
        let extent = |stretched: bool, off_a: f32, off_b: f32, desired: f32, align: f32| {
            if stretched {
                off_a.max(0.0) + off_b.max(0.0)
            } else {
                let s = if auto { desired } else { off_b };
                (off_a - align * s).max(0.0) + s
            }
        };
        size.x = size.x.max(extent(amin.x != amax.x, o[0], o[2], child.desired.x, align.x));
        size.y = size.y.max(extent(amin.y != amax.y, o[1], o[3], child.desired.y, align.y));
    }
    size
}

pub fn arrange_canvas(node: &mut ResolvedNode) {
    let rect = node.rect;
    for child in node.children.iter_mut().filter(|c| !is_collapsed(c)) {
        let cell = canvas_slot_rect(child, rect);
        arrange(child, cell, Affine::IDENTITY);
    }
}

// ── Boxes ──────────────────────────────────────────────────────────────────

fn along(v: Vec2, horizontal: bool) -> (f32, f32) {
    if horizontal {
        (v.x, v.y)
    } else {
        (v.y, v.x)
    }
}

fn join(main: f32, cross: f32, horizontal: bool) -> Vec2 {
    if horizontal {
        Vec2::new(main, cross)
    } else {
        Vec2::new(cross, main)
    }
}

pub fn measure_box(node: &mut ResolvedNode, avail: Vec2, horizontal: bool, m: &dyn TextMetrics) -> Vec2 {
    let (mut main, mut cross) = (0.0f32, 0.0f32);
    for child in node.children.iter_mut().filter(|c| !is_collapsed(c)) {
        let p = pad(child);
        let child_avail = if horizontal {
            Vec2::new(INF, avail.y - p.vertical())
        } else {
            Vec2::new(avail.x - p.horizontal(), INF)
        };
        measure(child, child_avail, m);
        let (cm, cc) = along(Vec2::new(child.desired.x + p.horizontal(), child.desired.y + p.vertical()), horizontal);
        main += cm;
        cross = cross.max(cc);
    }
    join(main, cross, horizontal)
}

pub fn arrange_box(node: &mut ResolvedNode, horizontal: bool) {
    arrange_run(node, horizontal, 0.0);
}

/// Lay children in a row or column. `scroll` shifts the start back.
fn arrange_run(node: &mut ResolvedNode, horizontal: bool, scroll: f32) {
    let rect = node.rect;
    let (space_main, space_cross) = along(rect.size(), horizontal);

    let mut auto_total = 0.0;
    let mut weight_total = 0.0;
    for child in node.children.iter().filter(|c| !is_collapsed(c)) {
        let p = pad(child);
        let (cm, _) = along(Vec2::new(child.desired.x + p.horizontal(), child.desired.y + p.vertical()), horizontal);
        if child.slot_view().choice("size_rule") == "fill" && scroll == 0.0 {
            weight_total += child.slot_view().f32("fill_weight").max(0.0);
        } else {
            auto_total += cm;
        }
    }
    let free = (space_main - auto_total).max(0.0);

    let mut cursor = if horizontal { rect.x } else { rect.y } - scroll;
    for child in node.children.iter_mut().filter(|c| !is_collapsed(c)) {
        let p = pad(child);
        let (cm, _) = along(Vec2::new(child.desired.x + p.horizontal(), child.desired.y + p.vertical()), horizontal);
        let allotted = if child.slot_view().choice("size_rule") == "fill" && scroll == 0.0 {
            let w = child.slot_view().f32("fill_weight").max(0.0);
            if weight_total > 0.0 {
                free * w / weight_total
            } else {
                0.0
            }
        } else {
            cm
        };
        let cell = if horizontal {
            Rect::new(cursor, rect.y, allotted, space_cross)
        } else {
            Rect::new(rect.x, cursor, space_cross, allotted)
        };
        let (h, v) = {
            let s = child.slot_view();
            (s.choice("h_align"), s.choice("v_align"))
        };
        let placed = place(cell.inset(p), child.desired, &h, &v);
        arrange(child, placed, Affine::IDENTITY);
        cursor += allotted;
    }
}

pub fn measure_scroll(node: &mut ResolvedNode, avail: Vec2, m: &dyn TextMetrics) -> Vec2 {
    let horizontal = node.view().choice("orientation") == "horizontal";
    // The scrolling axis has all the room it needs.
    let unbounded = if horizontal { Vec2::new(INF, avail.y) } else { Vec2::new(avail.x, INF) };
    let content = measure_box(node, unbounded, horizontal, m);
    if horizontal {
        Vec2::new(if avail.x.is_finite() { content.x.min(avail.x) } else { content.x }, content.y)
    } else {
        Vec2::new(content.x, if avail.y.is_finite() { content.y.min(avail.y) } else { content.y })
    }
}

pub fn arrange_scroll(node: &mut ResolvedNode) {
    let horizontal = node.view().choice("orientation") == "horizontal";
    let offset = node.view().f32("scroll_offset");
    arrange_run(node, horizontal, offset.max(0.0));
}

/// How far a scroll box's content runs, along its scrolling axis.
pub fn scroll_extent(node: &ResolvedNode) -> f32 {
    let horizontal = node.view().choice("orientation") == "horizontal";
    let offset = node.view().f32("scroll_offset").max(0.0);
    let mut end = if horizontal { node.rect.x } else { node.rect.y };
    for child in node.children.iter().filter(|c| !is_collapsed(c)) {
        let p = pad(child);
        let far = if horizontal { child.rect.right() + p.right } else { child.rect.bottom() + p.bottom };
        end = end.max(far);
    }
    let start = if horizontal { node.rect.x } else { node.rect.y };
    (end - start + offset).max(0.0)
}

// ── Grid ───────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Track {
    Auto,
    Px(f32),
    Weight(f32),
}

/// `"1, auto, 120px, 2"` as tracks. Anything unreadable is a weight of 1.
pub fn parse_tracks(spec: &str) -> Vec<Track> {
    let tracks: Vec<Track> = spec
        .split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|t| {
            let lower = t.to_ascii_lowercase();
            if lower == "auto" {
                Track::Auto
            } else if let Some(px) = lower.strip_suffix("px") {
                px.trim().parse().map_or(Track::Weight(1.0), |v: f32| Track::Px(v.max(0.0)))
            } else {
                lower.trim_end_matches('*').parse().map_or(Track::Weight(1.0), |v: f32| Track::Weight(v.max(0.0)))
            }
        })
        .collect();
    if tracks.is_empty() {
        vec![Track::Auto]
    } else {
        tracks
    }
}

struct Cell {
    child: usize,
    col: usize,
    row: usize,
    col_span: usize,
    row_span: usize,
}

fn cells(node: &ResolvedNode) -> Vec<Cell> {
    node.children
        .iter()
        .enumerate()
        .filter(|(_, c)| !is_collapsed(c))
        .map(|(i, c)| {
            let v = c.slot_view();
            Cell {
                child: i,
                col: v.i32("column").max(0) as usize,
                row: v.i32("row").max(0) as usize,
                col_span: v.i32("column_span").max(1) as usize,
                row_span: v.i32("row_span").max(1) as usize,
            }
        })
        .collect()
}

fn track_at(tracks: &[Track], i: usize) -> Track {
    *tracks.get(i).unwrap_or_else(|| tracks.last().expect("parse_tracks never returns an empty list"))
}

/// Sizes of `count` tracks: content size for `auto` (and, while measuring,
/// weighted) tracks, pixels as given, and the rest of `space` shared by weight.
fn resolve_tracks(tracks: &[Track], count: usize, content: &[f32], space: Option<f32>, gap: f32) -> Vec<f32> {
    let mut sizes: Vec<f32> = (0..count)
        .map(|i| match track_at(tracks, i) {
            Track::Px(px) => px,
            Track::Auto => content[i],
            Track::Weight(_) => if space.is_some() { 0.0 } else { content[i] },
        })
        .collect();
    if let Some(space) = space {
        let used: f32 = sizes.iter().sum::<f32>() + gap * count.saturating_sub(1) as f32;
        let weights: f32 = (0..count).map(|i| if let Track::Weight(w) = track_at(tracks, i) { w } else { 0.0 }).sum();
        let free = (space - used).max(0.0);
        if weights > 0.0 {
            for (i, size) in sizes.iter_mut().enumerate() {
                if let Track::Weight(w) = track_at(tracks, i) {
                    *size = free * w / weights;
                }
            }
        }
    }
    sizes
}

fn grid_shape(node: &ResolvedNode, cells: &[Cell]) -> (usize, usize) {
    let cols = cells.iter().map(|c| c.col + c.col_span).max().unwrap_or(0).max(parse_tracks(&node.view().text("column_sizes")).len().min(64));
    let rows = cells.iter().map(|c| c.row + c.row_span).max().unwrap_or(0);
    (cols.min(256), rows.min(256))
}

fn content_sizes(node: &ResolvedNode, cells: &[Cell], cols: usize, rows: usize) -> (Vec<f32>, Vec<f32>) {
    let mut col_w = vec![0.0f32; cols];
    let mut row_h = vec![0.0f32; rows];
    for cell in cells {
        let child = &node.children[cell.child];
        let p = pad(child);
        // Spanning cells don't size their tracks; their span absorbs the slack.
        if cell.col_span == 1 && cell.col < cols {
            col_w[cell.col] = col_w[cell.col].max(child.desired.x + p.horizontal());
        }
        if cell.row_span == 1 && cell.row < rows {
            row_h[cell.row] = row_h[cell.row].max(child.desired.y + p.vertical());
        }
    }
    (col_w, row_h)
}

pub fn measure_grid(node: &mut ResolvedNode, avail: Vec2, m: &dyn TextMetrics) -> Vec2 {
    for child in node.children.iter_mut().filter(|c| !is_collapsed(c)) {
        let p = pad(child);
        measure(child, Vec2::new(avail.x - p.horizontal(), avail.y - p.vertical()), m);
    }
    let cs = cells(node);
    let (cols, rows) = grid_shape(node, &cs);
    let gap = node.view().vec2("gap");
    let (col_content, row_content) = content_sizes(node, &cs, cols, rows);
    let col_tracks = parse_tracks(&node.view().text("column_sizes"));
    let row_tracks = parse_tracks(&node.view().text("row_sizes"));
    let widths = resolve_tracks(&col_tracks, cols, &col_content, None, gap.x);
    let heights = resolve_tracks(&row_tracks, rows, &row_content, None, gap.y);
    Vec2::new(
        widths.iter().sum::<f32>() + gap.x * cols.saturating_sub(1) as f32,
        heights.iter().sum::<f32>() + gap.y * rows.saturating_sub(1) as f32,
    )
}

pub fn arrange_grid(node: &mut ResolvedNode) {
    let rect = node.rect;
    let cs = cells(node);
    let (cols, rows) = grid_shape(node, &cs);
    let gap = node.view().vec2("gap");
    let (col_content, row_content) = content_sizes(node, &cs, cols, rows);
    let col_tracks = parse_tracks(&node.view().text("column_sizes"));
    let row_tracks = parse_tracks(&node.view().text("row_sizes"));
    let widths = resolve_tracks(&col_tracks, cols, &col_content, Some(rect.w), gap.x);
    let heights = resolve_tracks(&row_tracks, rows, &row_content, Some(rect.h), gap.y);

    let offsets = |sizes: &[f32], gap: f32, start: f32| {
        let mut at = start;
        sizes
            .iter()
            .map(|s| {
                let o = at;
                at += s + gap;
                o
            })
            .collect::<Vec<f32>>()
    };
    let xs = offsets(&widths, gap.x, rect.x);
    let ys = offsets(&heights, gap.y, rect.y);

    for cell in &cs {
        if cell.col >= cols || cell.row >= rows {
            continue;
        }
        let c1 = (cell.col + cell.col_span).min(cols);
        let r1 = (cell.row + cell.row_span).min(rows);
        let w: f32 = widths[cell.col..c1].iter().sum::<f32>() + gap.x * (c1 - cell.col - 1) as f32;
        let h: f32 = heights[cell.row..r1].iter().sum::<f32>() + gap.y * (r1 - cell.row - 1) as f32;
        let area = Rect::new(xs[cell.col], ys[cell.row], w, h);
        let child = &mut node.children[cell.child];
        let placed = super::place_in(area, child);
        arrange(child, placed, Affine::IDENTITY);
    }
}

// ── Wrap ───────────────────────────────────────────────────────────────────

struct Placement {
    child: usize,
    pos: Vec2,
    size: Vec2,
}

/// Flow children along lines no longer than `limit`. Returns where each
/// goes (relative to the box's corner) and the total size.
fn flow(node: &ResolvedNode, limit: f32, horizontal: bool, gap: Vec2) -> (Vec<Placement>, Vec2) {
    let mut placements = Vec::new();
    let (gap_main, gap_cross) = if horizontal { (gap.x, gap.y) } else { (gap.y, gap.x) };
    let (mut main, mut cross, mut line) = (0.0f32, 0.0f32, 0.0f32);
    let mut longest = 0.0f32;
    let mut first_in_line = true;
    for (i, child) in node.children.iter().enumerate().filter(|(_, c)| !is_collapsed(c)) {
        let p = pad(child);
        let size = Vec2::new(child.desired.x + p.horizontal(), child.desired.y + p.vertical());
        let (item_main, item_cross) = along(size, horizontal);
        if !first_in_line && main + gap_main + item_main > limit {
            cross += line + gap_cross;
            main = 0.0;
            line = 0.0;
            first_in_line = true;
        }
        if !first_in_line {
            main += gap_main;
        }
        let pos = join(main, cross, horizontal);
        placements.push(Placement { child: i, pos, size });
        main += item_main;
        line = line.max(item_cross);
        longest = longest.max(main);
        first_in_line = false;
    }
    (placements, join(longest, cross + line, horizontal))
}

pub fn measure_wrap(node: &mut ResolvedNode, avail: Vec2, m: &dyn TextMetrics) -> Vec2 {
    let horizontal = node.view().choice("orientation") != "vertical";
    for child in node.children.iter_mut().filter(|c| !is_collapsed(c)) {
        measure(child, Vec2::new(INF, INF), m);
    }
    let limit = node.view().opt_f32("wrap_size").unwrap_or(if horizontal { avail.x } else { avail.y });
    let gap = node.view().vec2("gap");
    flow(node, if limit.is_finite() { limit } else { INF }, horizontal, gap).1
}

pub fn arrange_wrap(node: &mut ResolvedNode) {
    let horizontal = node.view().choice("orientation") != "vertical";
    let rect = node.rect;
    let limit = node.view().opt_f32("wrap_size").unwrap_or(if horizontal { rect.w } else { rect.h });
    let gap = node.view().vec2("gap");
    let (placements, _) = flow(node, limit, horizontal, gap);
    for placement in placements {
        let child = &mut node.children[placement.child];
        let area = Rect::new(rect.x + placement.pos.x, rect.y + placement.pos.y, placement.size.x, placement.size.y);
        let placed = super::place_in(area, child);
        arrange(child, placed, Affine::IDENTITY);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_specs_parse_leniently() {
        assert_eq!(parse_tracks("1, auto, 120px, 2*"), vec![Track::Weight(1.0), Track::Auto, Track::Px(120.0), Track::Weight(2.0)]);
        assert_eq!(parse_tracks(""), vec![Track::Auto]);
        assert_eq!(parse_tracks("wat"), vec![Track::Weight(1.0)]);
        assert_eq!(parse_tracks("-5px"), vec![Track::Px(0.0)]);
    }

    #[test]
    fn weighted_tracks_share_what_the_fixed_ones_leave() {
        let tracks = [Track::Px(100.0), Track::Weight(1.0), Track::Weight(3.0)];
        let sizes = resolve_tracks(&tracks, 3, &[0.0, 0.0, 0.0], Some(500.0), 0.0);
        assert_eq!(sizes, vec![100.0, 100.0, 300.0]);
        // More tracks than entries repeat the last.
        let sizes = resolve_tracks(&[Track::Px(10.0)], 3, &[0.0; 3], Some(500.0), 0.0);
        assert_eq!(sizes, vec![10.0, 10.0, 10.0]);
    }
}

/// The `offsets` that place a canvas child at `wanted` in a parent of
/// `parent` size: the inverse of [`canvas_slot_rect`], for dragging and
/// resizing. Keeps the child's anchors and alignment.
pub fn offsets_for_rect(child: &ResolvedNode, parent: Rect, wanted: Rect) -> [f32; 4] {
    let v = child.slot_view();
    offsets_for(v.vec2("anchor_min"), v.vec2("anchor_max"), v.vec2("alignment"), parent, wanted)
}

/// [`offsets_for_rect`] for anchors and alignment given directly.
pub fn offsets_for(amin: Vec2, amax: Vec2, align: Vec2, parent: Rect, wanted: Rect) -> [f32; 4] {
    let axis = |lo: f32, hi: f32, start: f32, space: f32, pos: f32, extent: f32, align: f32| -> (f32, f32) {
        let a = start + lo * space;
        let b = start + hi * space;
        if (lo - hi).abs() < 1e-6 {
            (pos + align * extent - a, extent)
        } else {
            (pos - a, b - (pos + extent))
        }
    };
    let (x0, x1) = axis(amin.x, amax.x, parent.x, parent.w, wanted.x, wanted.w, align.x);
    let (y0, y1) = axis(amin.y, amax.y, parent.y, parent.h, wanted.y, wanted.h, align.y);
    [x0, y0, x1, y1]
}

/// `offsets` moved by `(dx, dy)` without resizing the child.
pub fn translate_offsets(offsets: [f32; 4], amin: Vec2, amax: Vec2, dx: f32, dy: f32) -> [f32; 4] {
    let mut o = offsets;
    o[0] += dx;
    if (amin.x - amax.x).abs() >= 1e-6 {
        o[2] -= dx;
    }
    o[1] += dy;
    if (amin.y - amax.y).abs() >= 1e-6 {
        o[3] -= dy;
    }
    o
}

#[cfg(test)]
mod canvas_math_tests {
    use super::*;
    use crate::layout::{layout, FixedMetrics};
    use crate::model::schema::Kind;
    use crate::model::validate::NoWidgets;
    use crate::model::WidgetAsset;
    use crate::resolve::resolve;
    use serde_json::json;

    fn child_with(anchor_min: [f32; 2], anchor_max: [f32; 2], offsets: [f32; 4], alignment: [f32; 2]) -> (WidgetAsset, Vec2) {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let mut n = w.new_node(Kind::Spacer);
        n.name = "C".into();
        let id = n.id;
        w.add(root, None, n).unwrap();
        let node = w.node_mut(id).unwrap();
        node.set_slot(crate::model::SlotKind::Canvas, "anchor_min", json!(anchor_min));
        node.set_slot(crate::model::SlotKind::Canvas, "anchor_max", json!(anchor_max));
        node.set_slot(crate::model::SlotKind::Canvas, "offsets", json!(offsets));
        node.set_slot(crate::model::SlotKind::Canvas, "alignment", json!(alignment));
        (w, Vec2::new(800.0, 600.0))
    }

    fn rect_of(w: &WidgetAsset, size: Vec2) -> (Rect, ResolvedNode) {
        let mut tree = resolve(w, &NoWidgets);
        layout(&mut tree, size, &FixedMetrics);
        let child = tree.children[0].clone();
        (child.rect, child)
    }

    #[test]
    fn offsets_for_a_rect_reproduce_that_rect_for_any_anchoring() {
        let cases = [
            ([0.0, 0.0], [0.0, 0.0], [10.0, 20.0, 100.0, 50.0], [0.0, 0.0]),
            ([0.5, 0.5], [0.5, 0.5], [0.0, 0.0, 200.0, 100.0], [0.5, 0.5]),
            ([1.0, 0.0], [1.0, 0.0], [-120.0, 8.0, 100.0, 40.0], [0.0, 0.0]),
            ([0.0, 0.0], [1.0, 1.0], [10.0, 20.0, 30.0, 40.0], [0.0, 0.0]),
            ([0.0, 0.5], [1.0, 0.5], [16.0, 0.0, 16.0, 60.0], [0.0, 0.5]),
        ];
        for (amin, amax, offsets, alignment) in cases {
            let (mut w, size) = child_with(amin, amax, offsets, alignment);
            let (rect, node) = rect_of(&w, size);
            // Ask for a different rect, then lay out again with the new offsets.
            let wanted = Rect::new(rect.x + 37.0, rect.y - 11.0, rect.w + 21.0, rect.h + 9.0);
            let parent = Rect::new(0.0, 0.0, size.x, size.y);
            let new_offsets = offsets_for_rect(&node, parent, wanted);
            let id = w.root.children[0].id;
            w.node_mut(id).unwrap().set_slot(crate::model::SlotKind::Canvas, "offsets", json!(new_offsets));
            let (after, _) = rect_of(&w, size);
            let close = |a: f32, b: f32| (a - b).abs() < 0.01;
            assert!(close(after.x, wanted.x) && close(after.y, wanted.y) && close(after.w, wanted.w) && close(after.h, wanted.h), "{amin:?}-{amax:?}: {after:?} != {wanted:?}");
        }
    }

    #[test]
    fn translating_offsets_moves_without_resizing() {
        let (mut w, size) = child_with([0.0, 0.0], [1.0, 0.0], [10.0, 20.0, 30.0, 40.0], [0.0, 0.0]);
        let (before, _) = rect_of(&w, size);
        let moved = translate_offsets([10.0, 20.0, 30.0, 40.0], Vec2::new(0.0, 0.0), Vec2::new(1.0, 0.0), 5.0, 7.0);
        let id = w.root.children[0].id;
        w.node_mut(id).unwrap().set_slot(crate::model::SlotKind::Canvas, "offsets", json!(moved));
        let (after, _) = rect_of(&w, size);
        assert!((after.x - before.x - 5.0).abs() < 0.01 && (after.y - before.y - 7.0).abs() < 0.01);
        assert!((after.w - before.w).abs() < 0.01 && (after.h - before.h).abs() < 0.01);
    }
}
