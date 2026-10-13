use serde_json::{json, Value};

use super::*;
use crate::model::schema::SlotKind;
use crate::model::validate::NoWidgets;
use crate::model::{NodeId, WidgetAsset};
use crate::resolve::resolve;

const VIEW: Vec2 = Vec2::new(1000.0, 500.0);

/// A widget laid out in `VIEW`.
struct Laid {
    widget: WidgetAsset,
    tree: ResolvedNode,
}

impl Laid {
    fn new(widget: WidgetAsset) -> Self {
        let mut tree = resolve(&widget, &NoWidgets);
        layout(&mut tree, VIEW, &FixedMetrics);
        Laid { widget, tree }
    }

    fn node(&self, name: &str) -> &ResolvedNode {
        let mut found = None;
        self.tree.walk(&mut |n| {
            if n.name == name && found.is_none() {
                found = Some(n);
            }
        });
        found.unwrap_or_else(|| panic!("no node named {name}"))
    }

    fn rect(&self, name: &str) -> Rect {
        self.node(name).rect
    }
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect::new(x, y, w, h)
}

/// Adds a node of `kind` named `name` under `parent`.
fn add(w: &mut WidgetAsset, parent: NodeId, kind: Kind, name: &str) -> NodeId {
    let mut node = w.new_node(kind);
    node.name = name.to_string();
    let id = node.id;
    w.add(parent, None, node).unwrap();
    id
}

fn set(w: &mut WidgetAsset, id: NodeId, key: &str, value: Value) {
    w.node_mut(id).unwrap().set_prop(key, value);
}

fn slot(w: &mut WidgetAsset, id: NodeId, key: &str, value: Value) {
    let kind = w.slot_kind_of(id);
    w.node_mut(id).unwrap().set_slot(kind, key, value);
}

fn approx(a: Rect, b: Rect) {
    let close = |x: f32, y: f32| (x - y).abs() < 0.01;
    assert!(close(a.x, b.x) && close(a.y, b.y) && close(a.w, b.w) && close(a.h, b.h), "{a:?} != {b:?}");
}

#[test]
fn canvas_children_sit_at_their_offsets() {
    let mut w = WidgetAsset::new();
    let root = w.root.id;
    let a = add(&mut w, root, Kind::Spacer, "A");
    slot(&mut w, a, "offsets", json!([10.0, 20.0, 100.0, 30.0]));
    let l = Laid::new(w);
    approx(l.rect("A"), rect(10.0, 20.0, 100.0, 30.0));
    approx(l.rect("Root"), rect(0.0, 0.0, 1000.0, 500.0));
}

#[test]
fn anchors_and_alignment_centre_a_child() {
    let mut w = WidgetAsset::new();
    let root = w.root.id;
    let a = add(&mut w, root, Kind::Spacer, "A");
    slot(&mut w, a, "anchor_min", json!([0.5, 0.5]));
    slot(&mut w, a, "anchor_max", json!([0.5, 0.5]));
    slot(&mut w, a, "offsets", json!([0.0, 0.0, 200.0, 100.0]));
    slot(&mut w, a, "alignment", json!([0.5, 0.5]));
    approx(Laid::new(w).rect("A"), rect(400.0, 200.0, 200.0, 100.0));
}

#[test]
fn stretched_anchors_keep_margins_as_the_canvas_resizes() {
    let mut w = WidgetAsset::new();
    let root = w.root.id;
    let a = add(&mut w, root, Kind::Spacer, "A");
    slot(&mut w, a, "anchor_max", json!([1.0, 1.0]));
    slot(&mut w, a, "offsets", json!([10.0, 20.0, 30.0, 40.0]));
    let l = Laid::new(w.clone());
    approx(l.rect("A"), rect(10.0, 20.0, 960.0, 440.0));

    // The same widget in a bigger viewport stretches with it.
    let mut tree = resolve(&w, &NoWidgets);
    layout(&mut tree, Vec2::new(1920.0, 1080.0), &FixedMetrics);
    let mut found = None;
    tree.walk(&mut |n| {
        if n.name == "A" {
            found = Some(n.rect)
        }
    });
    approx(found.unwrap(), rect(10.0, 20.0, 1880.0, 1020.0));
}

#[test]
fn size_to_content_uses_the_desired_size() {
    let mut w = WidgetAsset::new();
    let root = w.root.id;
    let t = add(&mut w, root, Kind::Text, "T");
    set(&mut w, t, "text", json!("Hello"));
    set(&mut w, t, "font_size", json!(20.0));
    slot(&mut w, t, "auto_size", json!(true));
    // FixedMetrics: 10px per glyph at size 20, line 25.
    approx(Laid::new(w).rect("T"), rect(0.0, 0.0, 50.0, 25.0));
}

#[test]
fn a_vertical_box_stacks_auto_children_and_shares_the_rest_by_weight() {
    let mut w = WidgetAsset::new();
    let root = w.root.id;
    let v = add(&mut w, root, Kind::VerticalBox, "V");
    slot(&mut w, v, "anchor_max", json!([1.0, 1.0]));
    slot(&mut w, v, "offsets", json!([0.0, 0.0, 0.0, 0.0]));
    let a = add(&mut w, v, Kind::Spacer, "A");
    set(&mut w, a, "size", json!([50.0, 100.0]));
    let b = add(&mut w, v, Kind::Spacer, "B");
    slot(&mut w, b, "size_rule", json!("fill"));
    slot(&mut w, b, "fill_weight", json!(1.0));
    let c = add(&mut w, v, Kind::Spacer, "C");
    slot(&mut w, c, "size_rule", json!("fill"));
    slot(&mut w, c, "fill_weight", json!(3.0));
    let l = Laid::new(w);
    // 500 - 100 auto = 400 shared 1:3.
    approx(l.rect("A"), rect(0.0, 0.0, 1000.0, 100.0));
    approx(l.rect("B"), rect(0.0, 100.0, 1000.0, 100.0));
    approx(l.rect("C"), rect(0.0, 200.0, 1000.0, 300.0));
}

#[test]
fn a_horizontal_box_aligns_children_across_its_height() {
    let mut w = WidgetAsset::new();
    let root = w.root.id;
    let h = add(&mut w, root, Kind::HorizontalBox, "H");
    slot(&mut w, h, "anchor_max", json!([1.0, 1.0]));
    slot(&mut w, h, "offsets", json!([0.0, 0.0, 0.0, 0.0]));
    let a = add(&mut w, h, Kind::Spacer, "A");
    set(&mut w, a, "size", json!([40.0, 20.0]));
    slot(&mut w, a, "v_align", json!("center"));
    slot(&mut w, a, "padding", json!([5.0, 0.0, 5.0, 0.0]));
    let b = add(&mut w, h, Kind::Spacer, "B");
    set(&mut w, b, "size", json!([60.0, 20.0]));
    slot(&mut w, b, "v_align", json!("bottom"));
    let l = Laid::new(w);
    approx(l.rect("A"), rect(5.0, 240.0, 40.0, 20.0));
    approx(l.rect("B"), rect(50.0, 480.0, 60.0, 20.0));
}

#[test]
fn collapsed_children_take_no_space_but_hidden_ones_do() {
    let build = |visibility: &str| {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let v = add(&mut w, root, Kind::VerticalBox, "V");
        let a = add(&mut w, v, Kind::Spacer, "A");
        set(&mut w, a, "size", json!([10.0, 100.0]));
        set(&mut w, a, "visibility", json!(visibility));
        let b = add(&mut w, v, Kind::Spacer, "B");
        set(&mut w, b, "size", json!([10.0, 10.0]));
        Laid::new(w)
    };
    assert_eq!(build("collapsed").rect("B").y, 0.0);
    assert_eq!(build("hidden").rect("B").y, 100.0);
    assert!(!is_drawn(build("hidden").node("A")));
    assert!(is_drawn(build("visible").node("A")));
}

#[test]
fn an_overlay_aligns_children_in_its_space() {
    let mut w = WidgetAsset::new();
    let root = w.root.id;
    let o = add(&mut w, root, Kind::Overlay, "O");
    slot(&mut w, o, "anchor_max", json!([1.0, 1.0]));
    slot(&mut w, o, "offsets", json!([0.0, 0.0, 0.0, 0.0]));
    let a = add(&mut w, o, Kind::Spacer, "A");
    set(&mut w, a, "size", json!([100.0, 50.0]));
    slot(&mut w, a, "h_align", json!("center"));
    slot(&mut w, a, "v_align", json!("bottom"));
    let b = add(&mut w, o, Kind::Spacer, "B");
    slot(&mut w, b, "padding", json!([10.0, 10.0, 10.0, 10.0]));
    let l = Laid::new(w);
    approx(l.rect("A"), rect(450.0, 450.0, 100.0, 50.0));
    approx(l.rect("B"), rect(10.0, 10.0, 980.0, 480.0));
}

#[test]
fn a_grid_sizes_columns_by_pixels_content_and_weight() {
    let mut w = WidgetAsset::new();
    let root = w.root.id;
    let g = add(&mut w, root, Kind::Grid, "G");
    slot(&mut w, g, "anchor_max", json!([1.0, 1.0]));
    slot(&mut w, g, "offsets", json!([0.0, 0.0, 0.0, 0.0]));
    set(&mut w, g, "column_sizes", json!("100px, auto, 1"));
    set(&mut w, g, "row_sizes", json!("auto"));
    let cell = |w: &mut WidgetAsset, name: &str, col: i32, row: i32, size: [f32; 2]| {
        let id = add(w, g, Kind::Spacer, name);
        set(w, id, "size", json!(size));
        slot(w, id, "column", json!(col));
        slot(w, id, "row", json!(row));
        id
    };
    cell(&mut w, "A", 0, 0, [10.0, 30.0]);
    cell(&mut w, "B", 1, 0, [80.0, 20.0]);
    cell(&mut w, "C", 2, 0, [10.0, 10.0]);
    cell(&mut w, "D", 0, 1, [10.0, 50.0]);
    let l = Laid::new(w);
    approx(l.rect("A"), rect(0.0, 0.0, 100.0, 30.0));
    approx(l.rect("B"), rect(100.0, 0.0, 80.0, 30.0));
    approx(l.rect("C"), rect(180.0, 0.0, 820.0, 30.0));
    approx(l.rect("D"), rect(0.0, 30.0, 100.0, 50.0));
}

#[test]
fn a_wrap_box_starts_a_new_line_when_full() {
    let mut w = WidgetAsset::new();
    let root = w.root.id;
    let wrap = add(&mut w, root, Kind::WrapBox, "W");
    slot(&mut w, wrap, "offsets", json!([0.0, 0.0, 250.0, 200.0]));
    set(&mut w, wrap, "gap", json!([10.0, 5.0]));
    for name in ["A", "B", "C", "D"] {
        let id = add(&mut w, wrap, Kind::Spacer, name);
        set(&mut w, id, "size", json!([100.0, 40.0]));
        slot(&mut w, id, "h_align", json!("left"));
        slot(&mut w, id, "v_align", json!("top"));
    }
    let l = Laid::new(w);
    // 100 + 10 + 100 = 210 fits in 250; a third doesn't.
    approx(l.rect("A"), rect(0.0, 0.0, 100.0, 40.0));
    approx(l.rect("B"), rect(110.0, 0.0, 100.0, 40.0));
    approx(l.rect("C"), rect(0.0, 45.0, 100.0, 40.0));
    approx(l.rect("D"), rect(110.0, 45.0, 100.0, 40.0));
}

#[test]
fn a_size_box_forces_and_clamps_its_childs_size() {
    let build = |props: &[(&str, f32)]| {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let s = add(&mut w, root, Kind::SizeBox, "S");
        slot(&mut w, s, "auto_size", json!(true));
        for (k, v) in props {
            set(&mut w, s, k, json!(v));
        }
        let c = add(&mut w, s, Kind::Spacer, "C");
        set(&mut w, c, "size", json!([50.0, 50.0]));
        Laid::new(w)
    };
    assert_eq!(build(&[("width", 200.0)]).rect("S").w, 200.0);
    assert_eq!(build(&[("min_width", 120.0)]).rect("S").w, 120.0);
    assert_eq!(build(&[("max_width", 30.0)]).rect("S").w, 30.0);
    assert_eq!(build(&[]).rect("S").w, 50.0);
    // The child fills the box.
    assert_eq!(build(&[("width", 200.0)]).rect("C").w, 200.0);
}

#[test]
fn a_scale_box_scales_its_child_to_fit_and_centres_it() {
    let mut w = WidgetAsset::new();
    let root = w.root.id;
    let s = add(&mut w, root, Kind::ScaleBox, "S");
    slot(&mut w, s, "offsets", json!([0.0, 0.0, 400.0, 400.0]));
    let c = add(&mut w, s, Kind::Spacer, "C");
    set(&mut w, c, "size", json!([100.0, 50.0]));
    let l = Laid::new(w);
    let child = l.node("C");
    // Contain: scale 4 (400/100) fits width; height becomes 200, centred.
    assert!((child.world.sx - 4.0).abs() < 1e-4);
    approx(child.world.apply_rect(child.rect), rect(0.0, 100.0, 400.0, 200.0));
}

#[test]
fn render_transforms_move_and_scale_without_changing_layout() {
    let mut w = WidgetAsset::new();
    let root = w.root.id;
    let a = add(&mut w, root, Kind::Spacer, "A");
    slot(&mut w, a, "offsets", json!([100.0, 100.0, 100.0, 100.0]));
    set(&mut w, a, "translate", json!([10.0, 0.0]));
    set(&mut w, a, "scale", json!([2.0, 2.0]));
    let l = Laid::new(w);
    let n = l.node("A");
    approx(n.rect, rect(100.0, 100.0, 100.0, 100.0));
    // Scaled about the centre (150, 150), then moved 10 right.
    approx(n.world.apply_rect(n.rect), rect(60.0, 50.0, 200.0, 200.0));
    let p = n.world.apply(Vec2::new(150.0, 150.0));
    assert!((p.x - 160.0).abs() < 1e-3 && (p.y - 150.0).abs() < 1e-3);
}

#[test]
fn buttons_wrap_their_content_in_padding() {
    let mut w = WidgetAsset::new();
    let root = w.root.id;
    let b = add(&mut w, root, Kind::Button, "B");
    slot(&mut w, b, "auto_size", json!(true));
    let t = add(&mut w, b, Kind::Text, "T");
    set(&mut w, t, "text", json!("OK"));
    set(&mut w, t, "font_size", json!(20.0));
    let l = Laid::new(w);
    // Text 20x25 plus the default 14/8 padding.
    approx(l.rect("B"), rect(0.0, 0.0, 20.0 + 28.0, 25.0 + 16.0));
    approx(l.rect("T"), rect(14.0, 8.0, 20.0, 25.0));
}

#[test]
fn wrapping_text_grows_taller_as_its_space_narrows() {
    let height_at = |width: f32| {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let v = add(&mut w, root, Kind::VerticalBox, "V");
        slot(&mut w, v, "offsets", json!([0.0, 0.0, width, 400.0]));
        let t = add(&mut w, v, Kind::Text, "T");
        set(&mut w, t, "text", json!("one two three four five six seven eight"));
        set(&mut w, t, "wrap", json!(true));
        set(&mut w, t, "font_size", json!(20.0));
        Laid::new(w).rect("T").h
    };
    assert!(height_at(100.0) > height_at(400.0));
    assert_eq!(height_at(10_000.0), 25.0, "fits on one line");
}

#[test]
fn a_scroll_box_shifts_its_content_by_the_offset() {
    let build = |offset: f32| {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let s = add(&mut w, root, Kind::ScrollBox, "S");
        slot(&mut w, s, "offsets", json!([0.0, 0.0, 300.0, 200.0]));
        set(&mut w, s, "scroll_offset", json!(offset));
        for i in 0..10 {
            let id = add(&mut w, s, Kind::Spacer, &format!("I{i}"));
            set(&mut w, id, "size", json!([50.0, 50.0]));
        }
        Laid::new(w)
    };
    let l = build(0.0);
    approx(l.rect("S"), rect(0.0, 0.0, 300.0, 200.0));
    assert_eq!(l.rect("I3").y, 150.0);
    assert_eq!(crate::layout::panels::scroll_extent(l.node("S")), 500.0);
    assert_eq!(build(120.0).rect("I3").y, 30.0);
}

#[test]
fn unresolved_instances_show_as_a_placeholder_and_empty_slots_as_a_hole() {
    let mut w = WidgetAsset::new();
    let root = w.root.id;
    let inst = w.new_instance(crate::model::WidgetId::new(), "Ghost");
    let inst_id = inst.id;
    w.add(root, None, inst).unwrap();
    slot(&mut w, inst_id, "auto_size", json!(true));
    let mut s = w.new_node(Kind::NamedSlot);
    s.name = "Body".into();
    let sid = s.id;
    w.add(root, None, s).unwrap();
    slot(&mut w, sid, "auto_size", json!(true));
    let l = Laid::new(w);
    assert_eq!(l.rect ("Ghost").size(), PLACEHOLDER_SIZE);
    assert_eq!(l.rect("Body").size(), EMPTY_SLOT_SIZE);
    let _ = SlotKind::Canvas;
}

#[test]
fn draw_order_follows_z_order_on_a_canvas() {
    let mut w = WidgetAsset::new();
    let root = w.root.id;
    for (name, z) in [("Top", 5), ("Back", -1), ("Mid", 0)] {
        let id = add(&mut w, root, Kind::Spacer, name);
        slot(&mut w, id, "z_order", json!(z));
    }
    let l = Laid::new(w);
    let order: Vec<_> = draw_order(&l.tree).iter().map(|n| n.name.clone()).collect();
    assert_eq!(order, ["Back", "Mid", "Top"]);
    let _ = &l.widget;
}
