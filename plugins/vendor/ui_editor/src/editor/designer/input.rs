//! Mouse, keyboard and drag-and-drop on the canvas.

use std::collections::HashSet;

use gpui::*;
use plugin_editor_api::{AssetPayload, OpenAsset};

use crate::model::schema::Children;
use crate::model::{Kind, NodeId, Rect, SlotKind, Vec2};

use super::super::canvas::geometry::{self, Snap};
use super::super::commands::DropSpot;
use super::super::icons::{LibraryDrag, NodeDrag, PaletteDrag};
use super::overlay::{handle_bounds, to_canvas_rect, DropPreview, ViewShot};
use super::{DesignerPanel, Gesture, MoveGesture, MoveItem, ResizeGesture};

/// Pixels the pointer must travel before a press becomes a drag.
const DRAG_THRESHOLD: f32 = 3.0;
/// How close, in canvas pixels, an edge must be to snap.
const SNAP_PIXELS: f32 = 6.0;

fn contains(r: &Rect, p: Vec2) -> bool {
    p.x >= r.x && p.x <= r.right() && p.y >= r.y && p.y <= r.bottom()
}

fn distance(a: Vec2, b: Vec2) -> f32 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2)).sqrt()
}

impl DesignerPanel {
    fn shot(&self, cx: &App) -> ViewShot {
        let v = self.ctx.view.read(cx);
        ViewShot { scale: v.scale(), pan: v.pan, grid: v.show_grid, grid_size: v.grid_size, outlines: v.outlines, safe_zone: v.safe_zone }
    }

    fn locked_ids(&self, cx: &App) -> HashSet<NodeId> {
        let mut locked = HashSet::new();
        self.ctx.widget(cx).root.walk(&mut |n| {
            if n.editor.locked {
                locked.insert(n.id);
            }
        });
        locked
    }

    /// Where on the canvas the pointer is, in layout space.
    fn layout_point(&self, canvas_px: Vec2, cx: &App) -> Vec2 {
        self.ctx.view.read(cx).to_widget(canvas_px)
    }

    // ── Mouse ──────────────────────────────────────────────────────────────

    /// Runs before child elements see a press, so a right click selects what
    /// is under it before the context menu is built.
    pub(super) fn capture_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if event.button != MouseButton::Right {
            return;
        }
        window.focus(&self.focus_handle, cx);
        let Some(frame) = self.frame.clone() else { return };
        let p = self.layout_point(self.to_canvas_px(event.position), cx);
        let locked = self.locked_ids(cx);
        if let Some(id) = frame.pick(p, &|id| !locked.contains(&id)) {
            if !self.ctx.sel.read(cx).contains(id) {
                self.ctx.select(vec![id], cx);
            }
        }
    }

    pub(super) fn mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle, cx);
        let canvas_px = self.to_canvas_px(event.position);
        self.mouse = Some(canvas_px);
        self.ctx.clear_notice(cx);

        if event.button == MouseButton::Middle || (event.button == MouseButton::Left && self.space_down) {
            let pan = self.ctx.view.read(cx).pan;
            self.ctx.view.update(cx, |v, _| v.auto_fit = false);
            self.gesture = Some(Gesture::Pan { start_mouse: canvas_px, start_pan: pan });
            cx.notify();
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }
        let Some(frame) = self.frame.clone() else { return };
        let shot = self.shot(cx);
        let widget_point = self.layout_point(canvas_px, cx);

        // The handles of a single selected canvas widget come first.
        let (selected, primary) = {
            let sel = self.ctx.sel.read(cx);
            (sel.ids().to_vec(), sel.primary())
        };
        if let (1, Some(id)) = (selected.len(), primary) {
            let widget = self.ctx.widget(cx);
            let resizable = widget.slot_kind_of(id) == SlotKind::Canvas && widget.node(id).is_some_and(|n| !n.editor.locked);
            if resizable {
                if let Some(rect) = frame.screen.get(&id) {
                    let canvas_rect = to_canvas_rect(&shot, *rect);
                    if let Some((handle, _)) = handle_bounds(canvas_rect).into_iter().find(|(_, hr)| contains(hr, canvas_px)) {
                        self.begin_resize(id, handle, canvas_px, &frame, cx);
                        return;
                    }
                }
            }
        }

        let locked = self.locked_ids(cx);
        let hit = frame.pick(widget_point, &|id| !locked.contains(&id));
        let additive = event.modifiers.shift || event.modifiers.secondary();
        match hit {
            Some(id) => {
                if event.click_count >= 2 {
                    if let Some(path) = self.instance_path(id, cx) {
                        window.dispatch_action(Box::new(OpenAsset { path }), cx);
                        return;
                    }
                }
                if additive {
                    self.ctx.sel.update(cx, |s, cx| s.toggle(id, cx));
                    cx.notify();
                    return;
                }
                let already = self.ctx.sel.read(cx).contains(id);
                let narrow_to = (already && selected.len() > 1).then_some(id);
                if !already {
                    self.ctx.select(vec![id], cx);
                }
                self.begin_drag(canvas_px, narrow_to, &frame, cx);
            }
            None => {
                let base = if additive { selected } else { Vec::new() };
                if !additive {
                    self.ctx.select(Vec::new(), cx);
                }
                self.gesture = Some(Gesture::Marquee { start: widget_point, current: widget_point, additive, base });
            }
        }
        cx.notify();
    }

    fn instance_path(&self, id: NodeId, cx: &App) -> Option<std::path::PathBuf> {
        let instance = self.ctx.widget(cx).node(id)?.instance.as_ref()?;
        self.ctx.lib.read(cx).library().path_of(&instance.widget).map(|p| p.to_path_buf())
    }

    fn begin_resize(&mut self, id: NodeId, handle: geometry::Handle, start_mouse: Vec2, frame: &crate::editor::frame::Frame, cx: &mut Context<Self>) {
        let widget = self.ctx.widget(cx);
        let Some(parent) = widget.parent_of(id) else { return };
        let (Some(start), Some(parent_rect)) = (frame.layout.get(&id).copied(), frame.layout.get(&parent).copied()) else { return };
        let k = self.ctx.view.read(cx).scale() * frame.world.get(&parent).map_or(1.0, |w| w.uniform());
        let targets = widget.node(parent).map(|p| p.children.iter().filter(|c| c.id != id).filter_map(|c| frame.layout.get(&c.id).copied()).collect()).unwrap_or_default();
        let before = self.ctx.doc.read(cx).snapshot();
        self.gesture = Some(Gesture::Resize(ResizeGesture { before, id, handle, start_mouse, start, parent: parent_rect, targets, k }));
        cx.notify();
    }

    /// A press on a widget: drag it where it is (canvas) or to somewhere else in the tree.
    fn begin_drag(&mut self, start_mouse: Vec2, narrow_to: Option<NodeId>, frame: &crate::editor::frame::Frame, cx: &mut Context<Self>) {
        let selected = self.ctx.selected(cx);
        let top = self.ctx.top_level(&selected, cx);
        let widget = self.ctx.widget(cx);

        let items: Vec<MoveItem> = top
            .iter()
            .filter(|&&id| widget.slot_kind_of(id) == SlotKind::Canvas && widget.node(id).is_some_and(|n| !n.editor.locked))
            .filter_map(|&id| {
                let parent = widget.parent_of(id)?;
                Some(MoveItem { id, rect: *frame.layout.get(&id)?, parent: *frame.layout.get(&parent)? })
            })
            .collect();

        if items.is_empty() {
            // Not on a canvas: dragging reorders it in the tree.
            if !top.is_empty() && top.iter().all(|&id| id != widget.root.id) {
                self.gesture = Some(Gesture::Reorder { ids: top, start_mouse, active: false });
            }
            return;
        }

        let moving: HashSet<NodeId> = items.iter().map(|i| i.id).collect();
        let first_parent = widget.parent_of(items[0].id);
        let targets = first_parent
            .and_then(|p| widget.node(p))
            .map(|p| p.children.iter().filter(|c| !moving.contains(&c.id)).filter_map(|c| frame.layout.get(&c.id).copied()).collect())
            .unwrap_or_default();
        let bounds = items[0].parent;
        let k = self.ctx.view.read(cx).scale() * first_parent.and_then(|p| frame.world.get(&p)).map_or(1.0, |w| w.uniform());
        let before = self.ctx.doc.read(cx).snapshot();
        self.gesture = Some(Gesture::Move(MoveGesture { before, start_mouse, items, targets, bounds, k, moved: false, narrow_to }));
    }

    pub(super) fn mouse_move(&mut self, event: &MouseMoveEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let canvas_px = self.to_canvas_px(event.position);
        self.mouse = Some(canvas_px);
        let Some(frame) = self.frame.clone() else { return };

        // Take the gesture out so the handlers can borrow `self` freely.
        let Some(mut gesture) = self.gesture.take() else {
            let locked = self.locked_ids(cx);
            let hover = frame.pick(self.layout_point(canvas_px, cx), &|id| !locked.contains(&id));
            if hover != self.hover {
                self.hover = hover;
            }
            cx.notify();
            return;
        };

        match &mut gesture {
            Gesture::Pan { start_mouse, start_pan } => {
                let (sm, sp) = (*start_mouse, *start_pan);
                self.ctx.view.update(cx, |v, cx| {
                    v.pan = Vec2::new(sp.x + canvas_px.x - sm.x, sp.y + canvas_px.y - sm.y);
                    cx.notify();
                });
            }
            Gesture::Marquee { start, current, additive, base } => {
                *current = self.layout_point(canvas_px, cx);
                let area = Rect::from_corners(*start, *current);
                let locked = self.locked_ids(cx);
                let mut ids = frame.in_area(area, self.ctx.widget(cx), &|id| !locked.contains(&id));
                if *additive {
                    let mut all = base.clone();
                    all.retain(|id| !ids.contains(id));
                    all.append(&mut ids);
                    ids = all;
                }
                self.ctx.select(ids, cx);
            }
            Gesture::Move(g) => self.drag_move(g, canvas_px, event.modifiers, cx),
            Gesture::Resize(g) => self.drag_resize(g, canvas_px, event.modifiers, cx),
            Gesture::Reorder { ids, start_mouse, active } => {
                if !*active && distance(*start_mouse, canvas_px) > DRAG_THRESHOLD + 1.0 {
                    *active = true;
                }
                if *active {
                    let p = self.layout_point(canvas_px, cx);
                    self.drop_preview = self.drop_target(p, &frame, ids, "Move here", cx).map(|(_, preview)| preview);
                }
            }
        }
        self.gesture = Some(gesture);
        cx.notify();
    }

    fn drag_move(&mut self, g: &mut MoveGesture, canvas_px: Vec2, modifiers: Modifiers, cx: &mut Context<Self>) {
        let mut d = Vec2::new((canvas_px.x - g.start_mouse.x) / g.k, (canvas_px.y - g.start_mouse.y) / g.k);
        if !g.moved {
            if distance(canvas_px, g.start_mouse) < DRAG_THRESHOLD {
                return;
            }
            g.moved = true;
        }
        if modifiers.shift {
            // Hold to one axis.
            if d.x.abs() >= d.y.abs() {
                d.y = 0.0;
            } else {
                d.x = 0.0;
            }
        }
        let union = g.items.iter().skip(1).fold(g.items[0].rect, |u, i| u.union(&i.rect));
        let mut moved = union.translate(d);
        self.guides.clear();
        let view = self.ctx.view.read(cx);
        if view.snap && !modifiers.alt {
            let snap = Snap { grid: Some(view.grid_size), tolerance: SNAP_PIXELS / view.scale() };
            let (fix, guides) = geometry::snap_move(moved, &g.targets, g.bounds, snap);
            d = Vec2::new(d.x + fix.x, d.y + fix.y);
            moved = moved.translate(fix);
            self.guides = guides;
        }
        let _ = moved;
        let items: Vec<(NodeId, Rect, Rect)> = g.items.iter().map(|i| (i.id, i.rect.translate(d), i.parent)).collect();
        let ctx = self.ctx.clone();
        // Start each live change from the state before the drag.
        let before = g.before.clone();
        ctx.doc.update(cx, |doc, cx| {
            doc.live(cx, |w| {
                *w = before;
                for (id, rect, parent) in &items {
                    super::super::commands::set_canvas_rect(w, *id, *parent, *rect);
                }
            })
        });
    }

    fn drag_resize(&mut self, g: &mut ResizeGesture, canvas_px: Vec2, modifiers: Modifiers, cx: &mut Context<Self>) {
        let d = Vec2::new((canvas_px.x - g.start_mouse.x) / g.k, (canvas_px.y - g.start_mouse.y) / g.k);
        let mut rect = geometry::resize_rect(g.start, g.handle, d, modifiers.shift, modifiers.alt, 4.0);
        self.guides.clear();
        let view = self.ctx.view.read(cx);
        if view.snap && !modifiers.secondary() && !modifiers.shift && !modifiers.alt {
            let snap = Snap { grid: Some(view.grid_size), tolerance: SNAP_PIXELS / view.scale() };
            let (snapped, guides) = geometry::snap_resize(rect, g.handle, &g.targets, g.parent, snap);
            rect = snapped;
            self.guides = guides;
        }
        let (id, parent, before) = (g.id, g.parent, g.before.clone());
        self.ctx.doc.update(cx, |doc, cx| {
            doc.live(cx, |w| {
                *w = before;
                super::super::commands::set_canvas_rect(w, id, parent, rect);
            })
        });
    }

    pub(super) fn mouse_up(&mut self, event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(gesture) = self.gesture.take() else { return };
        let _ = event;
        self.guides.clear();
        match gesture {
            Gesture::Move(g) => {
                if g.moved {
                    self.ctx.doc.update(cx, |d, cx| d.commit_gesture(g.before, "Move", cx));
                } else if let Some(id) = g.narrow_to {
                    self.ctx.select(vec![id], cx);
                }
            }
            Gesture::Resize(g) => {
                self.ctx.doc.update(cx, |d, cx| d.commit_gesture(g.before, "Resize", cx));
            }
            Gesture::Reorder { ids, active, .. } => {
                if active {
                    if let (Some(frame), Some(mouse)) = (self.frame.clone(), self.mouse) {
                        let p = self.layout_point(mouse, cx);
                        if let Some((spot, _)) = self.drop_target(p, &frame, &ids, "", cx) {
                            self.ctx.move_nodes(&ids, spot, cx);
                        }
                    }
                }
            }
            Gesture::Pan { .. } | Gesture::Marquee { .. } => {}
        }
        self.drop_preview = None;
        cx.notify();
    }

    pub(super) fn scroll(&mut self, event: &ScrollWheelEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let delta = event.delta.pixel_delta(px(20.0));
        let (dx, dy) = (f32::from(delta.x), f32::from(delta.y));
        let at = self.to_canvas_px(event.position);
        if event.modifiers.alt || event.modifiers.shift {
            self.ctx.view.update(cx, |v, cx| {
                v.pan = Vec2::new(v.pan.x + if event.modifiers.shift { dy } else { dx }, v.pan.y + if event.modifiers.shift { 0.0 } else { dy });
                v.auto_fit = false;
                cx.notify();
            });
        } else {
            let factor = (dy * 0.0015).exp();
            self.ctx.view.update(cx, |v, cx| {
                let zoom = v.zoom * factor;
                v.set_zoom_about(zoom, at);
                cx.notify();
            });
        }
    }

    // ── Keyboard ───────────────────────────────────────────────────────────

    pub(super) fn key_down(&mut self, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let m = event.keystroke.modifiers;
        if key == "space" && !event.is_held {
            self.space_down = true;
            cx.stop_propagation();
            return;
        }
        if key == "escape" {
            if let Some(gesture) = self.gesture.take() {
                match gesture {
                    Gesture::Move(g) => self.ctx.doc.update(cx, |d, cx| d.cancel_gesture(g.before, cx)),
                    Gesture::Resize(g) => self.ctx.doc.update(cx, |d, cx| d.cancel_gesture(g.before, cx)),
                    _ => {}
                }
                self.guides.clear();
                self.drop_preview = None;
                cx.notify();
                cx.stop_propagation();
                return;
            }
        }
        match (key, m.secondary()) {
            ("f", false) => self.frame_selection(cx),
            ("0" | "home", false) => self.ctx.view.update(cx, |v, cx| {
                v.auto_fit = true;
                cx.notify();
            }),
            ("0", true) => {
                let canvas = self.canvas_size();
                self.ctx.view.update(cx, |v, cx| {
                    v.set_zoom_about(1.0, Vec2::new(canvas.x * 0.5, canvas.y * 0.5));
                    cx.notify();
                });
            }
            _ => {
                if super::super::keys::handle(&self.ctx, event, self.frame.as_deref(), cx) {
                    cx.stop_propagation();
                }
                return;
            }
        }
        cx.stop_propagation();
    }

    pub(super) fn key_up(&mut self, event: &KeyUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        if event.keystroke.key == "space" {
            self.space_down = false;
            cx.notify();
        }
    }

    /// Zoom to the selection, or the whole screen when nothing is selected.
    pub fn frame_selection(&mut self, cx: &mut Context<Self>) {
        let Some(frame) = self.frame.clone() else { return };
        let ids = self.ctx.selected(cx);
        let rects: Vec<Rect> = ids.iter().filter_map(|id| frame.screen.get(id).copied()).collect();
        let canvas = self.canvas_size();
        if rects.is_empty() {
            self.ctx.view.update(cx, |v, cx| {
                v.auto_fit = true;
                cx.notify();
            });
            return;
        }
        let union = rects.iter().skip(1).fold(rects[0], |u, r| u.union(r));
        self.ctx.view.update(cx, |v, cx| {
            let dpi = v.dpi.max(0.25);
            let margin = 80.0;
            let zoom = (((canvas.x - margin * 2.0) / (union.w * dpi).max(1.0)).min((canvas.y - margin * 2.0) / (union.h * dpi).max(1.0))).clamp(0.05, 4.0);
            v.zoom = zoom;
            let scale = zoom * dpi;
            v.pan = Vec2::new(canvas.x * 0.5 - union.center().x * scale, canvas.y * 0.5 - union.center().y * scale);
            v.auto_fit = false;
            cx.notify();
        });
    }

    // ── Dropping onto the canvas ───────────────────────────────────────────

    /// Where something dropped at `p` (layout space) would go, and a preview of it.
    pub(crate) fn drop_target(
        &self,
        p: Vec2,
        frame: &crate::editor::frame::Frame,
        dragged: &[NodeId],
        label: &str,
        cx: &App,
    ) -> Option<(DropSpot, DropPreview)> {
        let widget = self.ctx.widget(cx);
        let exclude = |id: NodeId| dragged.iter().any(|&d| widget.is_within(d, id));
        let container = frame.container_at(p, widget, &exclude);
        let node = widget.node(container)?;
        let screen = *frame.screen.get(&container)?;

        let mut spot = DropSpot { container, index: None, at: None };
        let mut line = None;
        match node.kind {
            Kind::Canvas => spot.at = Some(p),
            Kind::VerticalBox | Kind::HorizontalBox | Kind::ScrollBox | Kind::WrapBox => {
                let horizontal = node.kind == Kind::HorizontalBox || (node.kind == Kind::WrapBox && node.view().choice("orientation") != "vertical");
                let mut index = node.children.len();
                for (i, child) in node.children.iter().enumerate() {
                    let Some(r) = frame.screen.get(&child.id) else { continue };
                    let mid = if horizontal { r.center().x } else { r.center().y };
                    if (if horizontal { p.x } else { p.y }) < mid {
                        index = i;
                        break;
                    }
                }
                let edge = node.children.get(index).and_then(|c| frame.screen.get(&c.id)).map(|r| if horizontal { r.x } else { r.y }).or_else(|| {
                    node.children.last().and_then(|c| frame.screen.get(&c.id)).map(|r| if horizontal { r.right() } else { r.bottom() })
                });
                if let Some(edge) = edge {
                    line = Some(if horizontal { Rect::new(edge - 1.5, screen.y, 3.0, screen.h) } else { Rect::new(screen.x, edge - 1.5, screen.w, 3.0) });
                }
                spot.index = Some(index);
            }
            _ => {
                if node.info().children == Children::None {
                    return None;
                }
            }
        }
        let preview = DropPreview {
            container: screen,
            line,
            at: spot.at,
            label: if label.is_empty() { String::new() } else { format!("{label} \u{2192} {}", node.name) },
        };
        Some((spot, preview))
    }

    fn preview_for(&mut self, position: Point<Pixels>, in_bounds: bool, dragged: &[NodeId], label: &str, cx: &mut Context<Self>) {
        let Some(frame) = self.frame.clone() else { return };
        let preview = in_bounds
            .then(|| self.drop_target(self.layout_point(self.to_canvas_px(position), cx), &frame, dragged, label, cx))
            .flatten()
            .map(|(_, p)| p);
        if preview != self.drop_preview {
            self.drop_preview = preview;
            cx.notify();
        }
    }

    pub(super) fn drag_over_palette(&mut self, event: &DragMoveEvent<PaletteDrag>, _window: &mut Window, cx: &mut Context<Self>) {
        let label = format!("Add {}", event.drag(cx).kind.info().label);
        self.preview_for(event.event.position, event.bounds.contains(&event.event.position), &[], &label, cx);
    }

    pub(super) fn drag_over_library(&mut self, event: &DragMoveEvent<LibraryDrag>, _window: &mut Window, cx: &mut Context<Self>) {
        let label = format!("Add {}", event.drag(cx).name);
        self.preview_for(event.event.position, event.bounds.contains(&event.event.position), &[], &label, cx);
    }

    pub(super) fn drag_over_nodes(&mut self, event: &DragMoveEvent<NodeDrag>, _window: &mut Window, cx: &mut Context<Self>) {
        let ids = event.drag(cx).ids.clone();
        self.preview_for(event.event.position, event.bounds.contains(&event.event.position), &ids, "Move", cx);
    }

    pub(super) fn drag_over_asset(&mut self, event: &DragMoveEvent<AssetPayload>, _window: &mut Window, cx: &mut Context<Self>) {
        let wanted = matches!(event.drag(cx).extension.as_str(), "widget" | "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tga" | "gif");
        self.preview_for(event.event.position, wanted && event.bounds.contains(&event.event.position), &[], "Add", cx);
    }

    fn spot_at(&self, position: Point<Pixels>, dragged: &[NodeId], cx: &App) -> Option<DropSpot> {
        let frame = self.frame.clone()?;
        let p = self.layout_point(self.to_canvas_px(position), cx);
        self.drop_target(p, &frame, dragged, "", cx).map(|(spot, _)| spot)
    }

    pub(super) fn drop_palette(&mut self, drag: &PaletteDrag, window: &mut Window, cx: &mut Context<Self>) {
        let spot = self.spot_at(window.mouse_position(), &[], cx);
        self.drop_preview = None;
        self.ctx.add_node(drag.kind, spot, cx);
        cx.notify();
    }

    pub(super) fn drop_library(&mut self, drag: &LibraryDrag, window: &mut Window, cx: &mut Context<Self>) {
        let spot = self.spot_at(window.mouse_position(), &[], cx);
        self.drop_preview = None;
        self.ctx.add_instance(&drag.id, &drag.name, spot, cx);
        cx.notify();
    }

    pub(super) fn drop_nodes(&mut self, drag: &NodeDrag, window: &mut Window, cx: &mut Context<Self>) {
        let spot = self.spot_at(window.mouse_position(), &drag.ids, cx);
        self.drop_preview = None;
        if let Some(spot) = spot {
            self.ctx.move_nodes(&drag.ids, spot, cx);
        }
        cx.notify();
    }

    /// A file dragged in from the file manager: widgets become instances, images become image widgets.
    pub(super) fn drop_asset(&mut self, drag: &AssetPayload, window: &mut Window, cx: &mut Context<Self>) {
        let spot = self.spot_at(window.mouse_position(), &[], cx);
        self.drop_preview = None;
        let root = self.project_root.clone();
        let path = std::path::PathBuf::from(&drag.engine_path);
        let absolute = if path.is_absolute() { path.clone() } else { root.join(&path) };
        match drag.extension.as_str() {
            "widget" => {
                let library = self.ctx.lib.read(cx).library();
                let id = library
                    .id_at(&absolute)
                    .cloned()
                    .or_else(|| std::fs::read_to_string(&absolute).ok().and_then(|t| crate::model::WidgetAsset::peek_id(&t)));
                match id {
                    Some(id) => {
                        let name = absolute.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                        self.ctx.add_instance(&id, &name, spot, cx);
                    }
                    None => self.ctx.say("That widget has no id yet. Open it once in the widget editor, then drag it in", cx),
                }
            }
            "png" | "jpg" | "jpeg" | "webp" | "bmp" | "tga" | "gif" => {
                let relative = absolute.strip_prefix(&root).map(|p| p.to_string_lossy().replace('\\', "/")).unwrap_or_else(|_| drag.engine_path.clone());
                self.ctx.add_image(&relative, spot, cx);
            }
            _ => self.ctx.say("Only widgets and images can be dropped here", cx),
        }
        cx.notify();
    }
}
