//! Turning a laid-out widget into a list of drawing commands.
//!
//! The list is plain data in widget space with every render transform,
//! opacity and enabled state already applied. A painter only has to turn each
//! command into pixels and map widget space onto its surface (the editor's
//! canvas paints with GPUI; a game would paint with its own renderer). The
//! same list answers "what did this draw" in tests.

use crate::layout::{self, draw_order, is_drawn, panels};
use crate::model::geom::{Color, Rect, Vec2};
use crate::model::schema::Kind;
use crate::resolve::{InstanceStatus, ResolvedNode};

/// How a shape is filled.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fill {
    Solid(Color),
    /// A linear gradient; `angle` in degrees, 180 runs top to bottom.
    Gradient { angle: f32, from: Color, to: Color },
}

impl Fill {
    pub fn is_visible(&self) -> bool {
        match self {
            Fill::Solid(c) => c.a > 0.0,
            Fill::Gradient { from, to, .. } => from.a > 0.0 || to.a > 0.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Justify {
    Left,
    Center,
    Right,
}

/// What a placeholder stands for, so the painter can tone it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceholderKind {
    /// A named slot nothing fills.
    EmptySlot,
    /// A widget that can't be shown.
    Problem,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    /// Restrict drawing to `Rect` until the matching [`Cmd::PopClip`].
    PushClip(Rect),
    PopClip,
    Shadow { rect: Rect, radii: [f32; 4], blur: f32, color: Color },
    Quad { rect: Rect, radii: [f32; 4], fill: Fill, border: Option<(f32, Color)> },
    Text {
        rect: Rect,
        text: String,
        size: f32,
        weight: String,
        italic: bool,
        color: Color,
        justify: Justify,
        wrap: bool,
        line_height: f32,
    },
    Image { rect: Rect, path: String, tint: Color, stretch: String, radii: [f32; 4], natural: Vec2 },
    /// A tick mark filling `rect`.
    Check { rect: Rect, color: Color },
    /// A downward chevron.
    Chevron { center: Vec2, size: f32, color: Color },
    /// A dashed outline with a label, drawn where something is missing.
    Placeholder { rect: Rect, label: String, kind: PlaceholderKind },
}

/// The look interactive widgets are previewed in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PreviewState {
    #[default]
    Normal,
    Hovered,
    Pressed,
    Disabled,
}

impl PreviewState {
    pub const ALL: [PreviewState; 4] =
        [PreviewState::Normal, PreviewState::Hovered, PreviewState::Pressed, PreviewState::Disabled];

    pub fn label(self) -> &'static str {
        match self {
            PreviewState::Normal => "Normal",
            PreviewState::Hovered => "Hovered",
            PreviewState::Pressed => "Pressed",
            PreviewState::Disabled => "Disabled",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DisplayOptions {
    pub state: PreviewState,
    /// Draw stand-ins for empty slots and unresolved widgets (design time).
    pub placeholders: bool,
    /// Seconds, for animated looks such as an indeterminate progress bar.
    pub time: f32,
}

impl Default for DisplayOptions {
    fn default() -> Self {
        Self { state: PreviewState::Normal, placeholders: true, time: 0.0 }
    }
}

/// The drawing commands for a laid-out tree.
pub fn build(root: &ResolvedNode, options: &DisplayOptions) -> Vec<Cmd> {
    let mut out = Vec::new();
    paint(root, options, 1.0, &mut out);
    out
}

fn radii(v: [f32; 4], k: f32) -> [f32; 4] {
    [v[0] * k, v[1] * k, v[2] * k, v[3] * k]
}

/// `rect` of `node` as drawn: layout space through the node's world transform.
pub fn screen_rect(node: &ResolvedNode) -> Rect {
    node.world.apply_rect(node.rect)
}

fn paint(node: &ResolvedNode, o: &DisplayOptions, parent_alpha: f32, out: &mut Vec<Cmd>) {
    if !is_drawn(node) {
        return;
    }
    let v = node.view();
    let mut alpha = parent_alpha * v.f32("opacity");
    let enabled = v.bool("enabled") && o.state != PreviewState::Disabled || !node.own;
    if !v.bool("enabled") {
        alpha *= 0.45;
    }
    let _ = enabled;
    if alpha <= 0.0 {
        return;
    }
    let rect = screen_rect(node);
    let k = node.world.uniform();
    let clip = v.bool("clip") || node.kind == Kind::ScrollBox;
    if clip {
        out.push(Cmd::PushClip(rect));
    }

    match node.kind {
        Kind::Border => paint_border(node, rect, k, alpha, out),
        Kind::Button => paint_button(node, rect, k, alpha, o, out),
        Kind::Text => paint_text(node, rect, k, alpha, out),
        Kind::Image => paint_image(node, rect, k, alpha, out),
        Kind::CheckBox => paint_check_box(node, rect, k, alpha, out),
        Kind::Slider => paint_slider(node, rect, k, alpha, out),
        Kind::ProgressBar => paint_progress(node, rect, k, alpha, o, out),
        Kind::TextInput => paint_text_input(node, rect, k, alpha, out),
        Kind::ComboBox => paint_combo(node, rect, k, alpha, out),
        Kind::Instance => {
            if let Some(status) = &node.status {
                if *status != InstanceStatus::Ok && o.placeholders {
                    out.push(Cmd::Placeholder { rect, label: problem_label(status), kind: PlaceholderKind::Problem });
                }
            }
        }
        Kind::NamedSlot if node.is_empty_slot && node.own && o.placeholders => {
            out.push(Cmd::Placeholder { rect, label: node.name.clone(), kind: PlaceholderKind::EmptySlot });
        }
        _ => {}
    }

    for child in draw_order(node) {
        paint(child, o, alpha, out);
    }

    if node.kind == Kind::ScrollBox {
        paint_scrollbar(node, rect, k, alpha, out);
    }
    if clip {
        out.push(Cmd::PopClip);
    }
}

pub fn problem_label(status: &InstanceStatus) -> String {
    let named = |hint: &str, what: &str| if hint.is_empty() { what.to_string() } else { format!("{what}: {hint}") };
    match status {
        InstanceStatus::Ok => String::new(),
        InstanceStatus::Missing { hint } => named(hint, "Missing widget"),
        InstanceStatus::Ambiguous { hint } => named(hint, "Ambiguous widget"),
        InstanceStatus::Cycle { hint } => named(hint, "Widget contains itself"),
        InstanceStatus::TooDeep => "Nested too deeply".to_string(),
    }
}

fn paint_border(node: &ResolvedNode, rect: Rect, k: f32, alpha: f32, out: &mut Vec<Cmd>) {
    let v = node.view();
    let r = radii(v.quad("corner_radius"), k);
    let shadow = v.color("shadow_color").faded(alpha);
    if shadow.a > 0.0 {
        let offset = v.vec2("shadow_offset");
        out.push(Cmd::Shadow { rect: rect.translate(offset * k), radii: r, blur: v.f32("shadow_blur") * k, color: shadow });
    }
    let bg = v.color("background").faded(alpha);
    let fill = if v.is_set("gradient_to") {
        Fill::Gradient { angle: v.f32("gradient_angle"), from: bg, to: v.color("gradient_to").faded(alpha) }
    } else {
        Fill::Solid(bg)
    };
    let width = v.f32("border_width") * k;
    let border_color = v.color("border_color").faded(alpha);
    let border = (width > 0.0 && border_color.a > 0.0).then_some((width, border_color));
    if fill.is_visible() || border.is_some() {
        out.push(Cmd::Quad { rect, radii: r, fill, border });
    }
}

fn paint_button(node: &ResolvedNode, rect: Rect, k: f32, alpha: f32, o: &DisplayOptions, out: &mut Vec<Cmd>) {
    let v = node.view();
    let state = if !v.bool("enabled") { PreviewState::Disabled } else if node.own { o.state } else { PreviewState::Normal };
    let color = match state {
        PreviewState::Normal => v.color("normal_color"),
        PreviewState::Hovered => v.color("hover_color"),
        PreviewState::Pressed => v.color("pressed_color"),
        PreviewState::Disabled => v.color("disabled_color"),
    }
    .faded(alpha);
    let width = v.f32("border_width") * k;
    let border_color = v.color("border_color").faded(alpha);
    out.push(Cmd::Quad {
        rect,
        radii: radii(v.quad("corner_radius"), k),
        fill: Fill::Solid(color),
        border: (width > 0.0 && border_color.a > 0.0).then_some((width, border_color)),
    });
}

fn paint_text(node: &ResolvedNode, rect: Rect, k: f32, alpha: f32, out: &mut Vec<Cmd>) {
    let v = node.view();
    let text = v.text("text");
    if text.is_empty() {
        return;
    }
    let justify = match v.choice("justify").as_str() {
        "center" => Justify::Center,
        "right" => Justify::Right,
        _ => Justify::Left,
    };
    let mut cmd = |rect: Rect, color: Color| Cmd::Text {
        rect,
        text: text.clone(),
        size: v.f32("font_size") * k,
        weight: v.choice("weight"),
        italic: v.bool("italic"),
        color,
        justify,
        wrap: v.bool("wrap"),
        line_height: v.f32("line_height"),
    };
    let shadow = v.color("shadow_color").faded(alpha);
    if shadow.a > 0.0 {
        out.push(cmd(rect.translate(v.vec2("shadow_offset") * k), shadow));
    }
    out.push(cmd(rect, v.color("color").faded(alpha)));
}

/// Where an image of `natural` size goes in `rect` for a stretch mode.
pub fn fit_image(rect: Rect, natural: Vec2, stretch: &str) -> Rect {
    if natural.x <= 0.0 || natural.y <= 0.0 {
        return rect;
    }
    let (kx, ky) = (rect.w / natural.x, rect.h / natural.y);
    let scale = match stretch {
        "uniform" => Vec2::new(kx.min(ky), kx.min(ky)),
        "uniform_fill" => Vec2::new(kx.max(ky), kx.max(ky)),
        "none" => Vec2::new(1.0, 1.0),
        _ => return rect,
    };
    let size = Vec2::new(natural.x * scale.x, natural.y * scale.y);
    Rect::new(rect.x + (rect.w - size.x) * 0.5, rect.y + (rect.h - size.y) * 0.5, size.x, size.y)
}

fn paint_image(node: &ResolvedNode, rect: Rect, k: f32, alpha: f32, out: &mut Vec<Cmd>) {
    let v = node.view();
    out.push(Cmd::Image {
        rect,
        path: v.image("source").0,
        tint: v.color("tint").faded(alpha),
        stretch: v.choice("stretch"),
        radii: radii(v.quad("corner_radius"), k),
        natural: v.vec2("image_size"),
    });
}

fn paint_check_box(node: &ResolvedNode, rect: Rect, k: f32, alpha: f32, out: &mut Vec<Cmd>) {
    let v = node.view();
    let size = v.f32("box_size") * k;
    let box_rect = Rect::new(rect.x, rect.y + (rect.h - size) * 0.5, size, size);
    let checked = v.bool("checked");
    let accent = v.color("check_color").faded(alpha);
    out.push(Cmd::Quad {
        rect: box_rect,
        radii: [4.0 * k; 4],
        fill: Fill::Solid(if checked { accent } else { v.color("box_color").faded(alpha) }),
        border: Some((1.0 * k, if checked { accent } else { Color::WHITE.with_alpha(0.22 * alpha) })),
    });
    if checked {
        out.push(Cmd::Check { rect: box_rect.inset(crate::model::Insets::new(size * 0.22, size * 0.22, size * 0.22, size * 0.22)), color: Color::WHITE.with_alpha(alpha) });
    }
    let label_x = rect.x + size + v.f32("spacing") * k;
    out.push(Cmd::Text {
        rect: Rect::new(label_x, rect.y, (rect.right() - label_x).max(0.0), rect.h),
        text: v.text("label"),
        size: v.f32("font_size") * k,
        weight: "regular".into(),
        italic: false,
        color: v.color("text_color").faded(alpha),
        justify: Justify::Left,
        wrap: false,
        line_height: 1.25,
    });
}

fn paint_slider(node: &ResolvedNode, rect: Rect, k: f32, alpha: f32, out: &mut Vec<Cmd>) {
    let v = node.view();
    let (min, max) = (v.f32("min"), v.f32("max"));
    let t = if max > min { ((v.f32("value") - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
    let thickness = v.f32("thickness") * k;
    let handle = v.f32("handle_size") * k;
    let vertical = v.choice("orientation") == "vertical";
    let center = rect.center();
    let (track, fill, handle_center) = if vertical {
        let track = Rect::new(center.x - thickness * 0.5, rect.y + handle * 0.5, thickness, (rect.h - handle).max(0.0));
        let y = track.bottom() - t * track.h;
        (track, Rect::new(track.x, y, thickness, track.bottom() - y), Vec2::new(center.x, y))
    } else {
        let track = Rect::new(rect.x + handle * 0.5, center.y - thickness * 0.5, (rect.w - handle).max(0.0), thickness);
        let x = track.x + t * track.w;
        (track, Rect::new(track.x, track.y, x - track.x, thickness), Vec2::new(x, center.y))
    };
    let r = [thickness * 0.5; 4];
    out.push(Cmd::Quad { rect: track, radii: r, fill: Fill::Solid(v.color("track_color").faded(alpha)), border: None });
    out.push(Cmd::Quad { rect: fill, radii: r, fill: Fill::Solid(v.color("fill_color").faded(alpha)), border: None });
    out.push(Cmd::Shadow {
        rect: Rect::new(handle_center.x - handle * 0.5, handle_center.y - handle * 0.5 + 1.0 * k, handle, handle),
        radii: [handle * 0.5; 4],
        blur: 4.0 * k,
        color: Color::BLACK.with_alpha(0.35 * alpha),
    });
    out.push(Cmd::Quad {
        rect: Rect::new(handle_center.x - handle * 0.5, handle_center.y - handle * 0.5, handle, handle),
        radii: [handle * 0.5; 4],
        fill: Fill::Solid(v.color("handle_color").faded(alpha)),
        border: None,
    });
}

fn paint_progress(node: &ResolvedNode, rect: Rect, k: f32, alpha: f32, o: &DisplayOptions, out: &mut Vec<Cmd>) {
    let v = node.view();
    let vertical = v.choice("orientation") == "vertical";
    let thickness = v.f32("thickness") * k;
    let bar = if vertical {
        Rect::new(rect.center().x - thickness * 0.5, rect.y, thickness, rect.h)
    } else {
        Rect::new(rect.x, rect.center().y - thickness * 0.5, rect.w, thickness)
    };
    let radius = (v.f32("corner_radius") * k).min(thickness * 0.5);
    out.push(Cmd::Quad { rect: bar, radii: [radius; 4], fill: Fill::Solid(v.color("back_color").faded(alpha)), border: None });

    let (start, len) = if v.bool("indeterminate") {
        let phase = (o.time * 0.8).fract();
        let seg = 0.35;
        let start = -seg + phase * (1.0 + seg);
        (start.max(0.0), (start + seg).min(1.0) - start.max(0.0))
    } else {
        (0.0, v.f32("percent").clamp(0.0, 1.0))
    };
    if len > 0.0 {
        let fill = if vertical {
            Rect::new(bar.x, bar.bottom() - (start + len) * bar.h, bar.w, len * bar.h)
        } else {
            Rect::new(bar.x + start * bar.w, bar.y, len * bar.w, bar.h)
        };
        out.push(Cmd::Quad { rect: fill, radii: [radius; 4], fill: Fill::Solid(v.color("fill_color").faded(alpha)), border: None });
    }
}

fn field_frame(v: &crate::model::node::View<'_>, rect: Rect, k: f32, alpha: f32, out: &mut Vec<Cmd>) {
    let width = v.f32("border_width") * k;
    out.push(Cmd::Quad {
        rect,
        radii: radii(v.quad("corner_radius"), k),
        fill: Fill::Solid(v.color("background").faded(alpha)),
        border: (width > 0.0).then(|| (width, v.color("border_color").faded(alpha))),
    });
}

fn paint_text_input(node: &ResolvedNode, rect: Rect, k: f32, alpha: f32, out: &mut Vec<Cmd>) {
    let v = node.view();
    field_frame(&v, rect, k, alpha, out);
    let p = v.insets("padding");
    let inner = Rect::new(rect.x + p.left * k, rect.y + p.top * k, (rect.w - p.horizontal() * k).max(0.0), (rect.h - p.vertical() * k).max(0.0));
    let text = v.text("text");
    let (shown, color) = if text.is_empty() {
        (v.text("hint"), v.color("hint_color"))
    } else if v.bool("password") {
        ("\u{2022}".repeat(text.chars().count()), v.color("color"))
    } else {
        (text, v.color("color"))
    };
    if !shown.is_empty() {
        out.push(Cmd::Text {
            rect: inner,
            text: shown,
            size: v.f32("font_size") * k,
            weight: "regular".into(),
            italic: false,
            color: color.faded(alpha),
            justify: Justify::Left,
            wrap: v.bool("multiline"),
            line_height: 1.25,
        });
    }
}

fn paint_combo(node: &ResolvedNode, rect: Rect, k: f32, alpha: f32, out: &mut Vec<Cmd>) {
    let v = node.view();
    field_frame(&v, rect, k, alpha, out);
    let p = v.insets("padding");
    let options = v.text("options");
    let selected = usize::try_from(v.i32("selected")).ok().and_then(|i| options.lines().nth(i)).unwrap_or("").to_string();
    let arrow = 28.0 * k;
    if !selected.is_empty() {
        out.push(Cmd::Text {
            rect: Rect::new(rect.x + p.left * k, rect.y + p.top * k, (rect.w - arrow - p.left * k).max(0.0), (rect.h - p.vertical() * k).max(0.0)),
            text: selected,
            size: v.f32("font_size") * k,
            weight: "regular".into(),
            italic: false,
            color: v.color("color").faded(alpha),
            justify: Justify::Left,
            wrap: false,
            line_height: 1.25,
        });
    }
    out.push(Cmd::Chevron { center: Vec2::new(rect.right() - arrow * 0.5, rect.center().y), size: 5.0 * k, color: v.color("color").faded(alpha * 0.8) });
}

fn paint_scrollbar(node: &ResolvedNode, rect: Rect, k: f32, alpha: f32, out: &mut Vec<Cmd>) {
    let v = node.view();
    if !v.bool("scrollbar_visible") {
        return;
    }
    let horizontal = v.choice("orientation") == "horizontal";
    let content = panels::scroll_extent(node) * k;
    let view = if horizontal { rect.w } else { rect.h };
    if content <= view + 0.5 || view <= 0.0 {
        return;
    }
    let thickness = 6.0 * k;
    let t0 = (v.f32("scroll_offset").max(0.0) * k / content).clamp(0.0, 1.0);
    let len = (view / content).clamp(0.05, 1.0);
    let color = v.color("scrollbar_color").faded(alpha);
    let thumb = if horizontal {
        Rect::new(rect.x + t0 * rect.w, rect.bottom() - thickness - 2.0 * k, len * rect.w, thickness)
    } else {
        Rect::new(rect.right() - thickness - 2.0 * k, rect.y + t0 * rect.h, thickness, len * rect.h)
    };
    out.push(Cmd::Quad { rect: thumb, radii: [thickness * 0.5; 4], fill: Fill::Solid(color), border: None });
}

// ── Hit testing ────────────────────────────────────────────────────────────

/// The deepest node drawn under `p` (widget space) that `accept` allows.
///
/// Containers with nothing to see in them are transparent to clicks, so a
/// click in empty canvas finds nothing rather than the canvas itself.
pub fn hit_test<'a>(root: &'a ResolvedNode, p: Vec2, accept: &dyn Fn(&ResolvedNode) -> bool) -> Option<&'a ResolvedNode> {
    fn go<'a>(node: &'a ResolvedNode, p: Vec2, clip: Option<Rect>, accept: &dyn Fn(&ResolvedNode) -> bool) -> Option<&'a ResolvedNode> {
        if layout::is_collapsed(node) || !matches!(node.view().choice("visibility").as_str(), "visible" | "hit_test_invisible") {
            return None;
        }
        let rect = screen_rect(node);
        let clip = if node.view().bool("clip") || node.kind == Kind::ScrollBox {
            Some(clip.map_or(Some(rect), |c| c.intersection(&rect))?)
        } else {
            clip
        };
        if let Some(c) = clip {
            if !c.contains(p) {
                return None;
            }
        }
        for child in draw_order(node).into_iter().rev() {
            if let Some(hit) = go(child, p, clip, accept) {
                return Some(hit);
            }
        }
        let passes_clicks = node.view().choice("visibility") == "hit_test_invisible";
        if !passes_clicks && rect.contains(p) && is_solid(node) && accept(node) {
            return Some(node);
        }
        None
    }
    go(root, p, None, accept)
}

/// Whether a node takes clicks on its own area (as opposed to only its children).
fn is_solid(node: &ResolvedNode) -> bool {
    match node.kind {
        Kind::Canvas | Kind::VerticalBox | Kind::HorizontalBox | Kind::Overlay | Kind::Grid | Kind::WrapBox | Kind::SizeBox | Kind::ScaleBox | Kind::NamedSlot => {
            false
        }
        Kind::Border => {
            let v = node.view();
            v.color("background").a > 0.0 || v.is_set("gradient_to") || (v.f32("border_width") > 0.0 && v.color("border_color").a > 0.0)
        }
        Kind::Instance => node.status.as_ref().is_some_and(|s| *s != InstanceStatus::Ok),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::{layout, FixedMetrics};
    use crate::model::validate::NoWidgets;
    use crate::model::WidgetAsset;
    use crate::resolve::resolve;
    use serde_json::json;

    fn laid(w: &WidgetAsset) -> ResolvedNode {
        let mut tree = resolve(w, &NoWidgets);
        layout(&mut tree, Vec2::new(800.0, 600.0), &FixedMetrics);
        tree
    }

    fn add(w: &mut WidgetAsset, parent: crate::model::NodeId, kind: Kind, name: &str, offsets: [f32; 4]) -> crate::model::NodeId {
        let mut n = w.new_node(kind);
        n.name = name.into();
        let id = n.id;
        w.add(parent, None, n).unwrap();
        w.node_mut(id).unwrap().set_slot(crate::model::SlotKind::Canvas, "offsets", json!(offsets));
        id
    }

    #[test]
    fn text_borders_and_buttons_produce_the_expected_commands() {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let b = add(&mut w, root, Kind::Border, "B", [10.0, 10.0, 200.0, 100.0]);
        w.node_mut(b).unwrap().set_prop("background", json!("#336699"));
        w.node_mut(b).unwrap().set_prop("corner_radius", json!([8.0, 8.0, 8.0, 8.0]));
        let t = add(&mut w, root, Kind::Text, "T", [20.0, 20.0, 100.0, 30.0]);
        w.node_mut(t).unwrap().set_prop("text", json!("Hi"));
        add(&mut w, root, Kind::Button, "Btn", [0.0, 200.0, 80.0, 30.0]);

        let cmds = build(&laid(&w), &DisplayOptions::default());
        assert!(matches!(&cmds[0], Cmd::Quad { rect, radii, fill: Fill::Solid(c), .. }
            if *rect == Rect::new(10.0, 10.0, 200.0, 100.0) && *radii == [8.0; 4] && c.to_hex() == "#336699"));
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Text { text, .. } if text == "Hi")));
        assert!(cmds.iter().any(|c| matches!(c, Cmd::Quad { rect, .. } if rect.y == 200.0)), "the button draws a quad");
    }

    #[test]
    fn invisible_and_transparent_things_draw_nothing() {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let t = add(&mut w, root, Kind::Text, "T", [0.0, 0.0, 100.0, 30.0]);
        w.node_mut(t).unwrap().set_prop("visibility", json!("hidden"));
        let u = add(&mut w, root, Kind::Text, "U", [0.0, 40.0, 100.0, 30.0]);
        w.node_mut(u).unwrap().set_prop("opacity", json!(0.0));
        assert!(build(&laid(&w), &DisplayOptions::default()).is_empty());
    }

    #[test]
    fn opacity_multiplies_down_the_tree() {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let b = add(&mut w, root, Kind::Border, "B", [0.0, 0.0, 100.0, 100.0]);
        w.node_mut(b).unwrap().set_prop("opacity", json!(0.5));
        w.node_mut(b).unwrap().set_prop("background", json!("#ffffff"));
        let cmds = build(&laid(&w), &DisplayOptions::default());
        assert!(matches!(&cmds[0], Cmd::Quad { fill: Fill::Solid(c), .. } if (c.a - 0.5).abs() < 1e-4));
    }

    #[test]
    fn clipping_brackets_the_clipped_content() {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let b = add(&mut w, root, Kind::Border, "B", [0.0, 0.0, 100.0, 100.0]);
        w.node_mut(b).unwrap().set_prop("clip", json!(true));
        w.node_mut(b).unwrap().set_prop("background", json!("#ffffff"));
        let cmds = build(&laid(&w), &DisplayOptions::default());
        assert!(matches!(cmds.first(), Some(Cmd::PushClip(_))));
        assert!(matches!(cmds.last(), Some(Cmd::PopClip)));
    }

    #[test]
    fn button_states_pick_their_colour() {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        add(&mut w, root, Kind::Button, "Btn", [0.0, 0.0, 80.0, 30.0]);
        let tree = laid(&w);
        let colour_in = |state| match &build(&tree, &DisplayOptions { state, ..Default::default() })[0] {
            Cmd::Quad { fill: Fill::Solid(c), .. } => c.to_hex(),
            other => panic!("{other:?}"),
        };
        assert_eq!(colour_in(PreviewState::Normal), "#3a3f4b");
        assert_eq!(colour_in(PreviewState::Hovered), "#4a5060");
        assert_eq!(colour_in(PreviewState::Pressed), "#2c303a");
    }

    #[test]
    fn progress_and_slider_reflect_their_values() {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let p = add(&mut w, root, Kind::ProgressBar, "P", [0.0, 0.0, 200.0, 12.0]);
        w.node_mut(p).unwrap().set_prop("percent", json!(0.25));
        let cmds = build(&laid(&w), &DisplayOptions::default());
        let widths: Vec<f32> = cmds.iter().filter_map(|c| if let Cmd::Quad { rect, .. } = c { Some(rect.w) } else { None }).collect();
        assert_eq!(widths, vec![200.0, 50.0]);
    }

    #[test]
    fn images_fit_their_box_by_stretch_mode() {
        let rect = Rect::new(0.0, 0.0, 200.0, 100.0);
        let natural = Vec2::new(100.0, 100.0);
        assert_eq!(fit_image(rect, natural, "fill"), rect);
        assert_eq!(fit_image(rect, natural, "uniform"), Rect::new(50.0, 0.0, 100.0, 100.0));
        assert_eq!(fit_image(rect, natural, "uniform_fill"), Rect::new(0.0, -50.0, 200.0, 200.0));
        assert_eq!(fit_image(rect, natural, "none"), Rect::new(50.0, 0.0, 100.0, 100.0));
    }

    #[test]
    fn missing_widgets_and_empty_slots_draw_placeholders_at_design_time_only() {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let inst = w.new_instance(crate::model::WidgetId::new(), "Ghost");
        let id = inst.id;
        w.add(root, None, inst).unwrap();
        w.node_mut(id).unwrap().set_slot(crate::model::SlotKind::Canvas, "auto_size", json!(true));
        let tree = laid(&w);
        let with = build(&tree, &DisplayOptions::default());
        assert!(matches!(&with[0], Cmd::Placeholder { label, kind: PlaceholderKind::Problem, .. } if label.contains("Ghost")));
        assert!(build(&tree, &DisplayOptions { placeholders: false, ..Default::default() }).is_empty());
    }

    #[test]
    fn hit_testing_finds_the_topmost_visible_leaf_and_ignores_empty_containers() {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let a = add(&mut w, root, Kind::Spacer, "A", [0.0, 0.0, 100.0, 100.0]);
        w.node_mut(a).unwrap().set_prop("size", json!([100.0, 100.0]));
        let b = add(&mut w, root, Kind::Spacer, "B", [50.0, 50.0, 100.0, 100.0]);
        let tree = laid(&w);
        let any = |_: &ResolvedNode| true;
        assert_eq!(hit_test(&tree, Vec2::new(60.0, 60.0), &any).map(|n| n.name.as_str()), Some("B"), "later siblings draw on top");
        assert_eq!(hit_test(&tree, Vec2::new(10.0, 10.0), &any).map(|n| n.name.as_str()), Some("A"));
        assert!(hit_test(&tree, Vec2::new(500.0, 500.0), &any).is_none(), "empty canvas is not a hit");
        // Locked or filtered nodes let the click through to what is under them.
        let not_b = |n: &ResolvedNode| n.owner != b;
        assert_eq!(hit_test(&tree, Vec2::new(60.0, 60.0), &not_b).map(|n| n.name.as_str()), Some("A"));
    }

    #[test]
    fn hit_testing_respects_clipping_and_render_transforms() {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let clip = add(&mut w, root, Kind::Border, "Clip", [0.0, 0.0, 100.0, 100.0]);
        w.node_mut(clip).unwrap().set_prop("clip", json!(true));
        let inner = w.new_node(Kind::Spacer);
        let inner_id = inner.id;
        w.add(clip, None, inner).unwrap();
        w.node_mut(inner_id).unwrap().set_prop("size", json!([300.0, 300.0]));
        w.node_mut(inner_id).unwrap().set_slot(crate::model::SlotKind::Single, "h_align", json!("left"));
        w.node_mut(inner_id).unwrap().set_slot(crate::model::SlotKind::Single, "v_align", json!("top"));
        let tree = laid(&w);
        let any = |_: &ResolvedNode| true;
        assert!(hit_test(&tree, Vec2::new(50.0, 50.0), &any).is_some());
        assert!(hit_test(&tree, Vec2::new(200.0, 200.0), &any).is_none(), "the part outside the clip can't be clicked");

        // A moved widget is hit where it is drawn.
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let a = add(&mut w, root, Kind::Spacer, "A", [0.0, 0.0, 50.0, 50.0]);
        w.node_mut(a).unwrap().set_prop("translate", json!([300.0, 0.0]));
        let tree = laid(&w);
        assert!(hit_test(&tree, Vec2::new(25.0, 25.0), &any).is_none());
        assert!(hit_test(&tree, Vec2::new(325.0, 25.0), &any).is_some());
    }
}
