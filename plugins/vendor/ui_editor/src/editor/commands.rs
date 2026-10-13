//! The things a user does to a widget, as undoable commands.
//!
//! Panels, the canvas, menus and the keyboard all go through [`Ctx`], so a
//! command behaves the same wherever it is started from. Each one is a single
//! undo step, keeps the selection sensible, and says why when it can't do
//! what was asked.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use gpui::*;
use serde_json::{json, Value as Json};

use crate::library::Library;
use crate::model::node::Node;
use crate::model::ops::OpError;
use crate::model::schema::{Kind, SlotKind, ValueClass};
use crate::model::validate::would_cycle;
use crate::model::{NodeId, ParamId, Rect, Vec2, WidgetAsset, WidgetId};

use super::canvas::geometry::{self, AlignKind, AnchorPreset};
use super::frame::Frame;
use super::state::{Doc, Lib, Sel, ViewState};
use crate::layout::panels::{offsets_for, translate_offsets};

/// How far pasted and duplicated canvas widgets are offset so they can be told apart.
pub const PASTE_OFFSET: f32 = 16.0;

/// Marks the clipboard text as widget nodes.
const CLIPBOARD_TAG: &str = "pulsar_widget_nodes";

/// What a new node of a kind looks like on a canvas.
pub fn default_canvas_size(kind: Kind) -> Option<Vec2> {
    // `None` sizes the node to its content.
    Some(match kind {
        Kind::Text | Kind::CheckBox | Kind::Instance | Kind::NamedSlot | Kind::Unknown => return None,
        Kind::Button => Vec2::new(160.0, 44.0),
        Kind::Image => Vec2::new(128.0, 128.0),
        Kind::Border => Vec2::new(240.0, 160.0),
        Kind::Slider => Vec2::new(220.0, 24.0),
        Kind::ProgressBar => Vec2::new(220.0, 14.0),
        Kind::TextInput => Vec2::new(240.0, 36.0),
        Kind::ComboBox => Vec2::new(220.0, 36.0),
        Kind::Spacer => Vec2::new(32.0, 32.0),
        Kind::SizeBox | Kind::ScaleBox => Vec2::new(200.0, 100.0),
        Kind::Canvas => Vec2::new(300.0, 200.0),
        Kind::VerticalBox | Kind::HorizontalBox | Kind::Overlay | Kind::Grid | Kind::WrapBox | Kind::ScrollBox => Vec2::new(240.0, 200.0),
    })
}

/// The shared state a command acts on.
#[derive(Clone)]
pub struct Ctx {
    pub doc: Doc,
    pub sel: Sel,
    pub view: ViewState,
    pub lib: Lib,
    /// The designer's latest layout, for commands that need to know where things are.
    pub frame: FrameHandle,
}

/// Shared slot the designer keeps its latest frame in.
pub type FrameHandle = Rc<RefCell<Option<Arc<Frame>>>>;

/// Where in the tree something goes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DropSpot {
    pub container: NodeId,
    /// Position among the container's children; the end when `None`.
    pub index: Option<usize>,
    /// For canvases: where the widget's corner goes, in the canvas's layout space.
    pub at: Option<Vec2>,
}

impl Ctx {
    /// The most recent layout, if a designer has drawn one.
    pub fn frame(&self) -> Option<Arc<Frame>> {
        self.frame.borrow().clone()
    }

    pub fn widget<'a>(&self, cx: &'a App) -> &'a WidgetAsset {
        self.doc.read(cx).asset()
    }

    pub fn selected(&self, cx: &App) -> Vec<NodeId> {
        self.sel.read(cx).ids().to_vec()
    }

    pub fn primary(&self, cx: &App) -> Option<NodeId> {
        self.sel.read(cx).primary()
    }

    pub fn select(&self, ids: Vec<NodeId>, cx: &mut App) {
        self.sel.update(cx, |s, cx| s.set(ids, cx));
    }

    /// Tell the user why something didn't happen.
    pub fn say(&self, message: impl Into<String>, cx: &mut App) {
        let message = message.into();
        self.view.update(cx, |v, cx| {
            v.notice = Some(message);
            cx.notify();
        });
    }

    pub fn clear_notice(&self, cx: &mut App) {
        if self.view.read(cx).notice.is_some() {
            self.view.update(cx, |v, cx| {
                v.notice = None;
                cx.notify();
            });
        }
    }

    /// The project's widgets with this editor's unsaved version of itself, for checks.
    pub fn library_now(&self, cx: &App) -> Library {
        let library = self.lib.read(cx).library();
        match self.doc.read(cx).path() {
            Some(path) => library.with_current(path, self.doc.read(cx).asset()),
            None => (*library).clone(),
        }
    }

    fn edit(&self, label: &'static str, cx: &mut App, change: impl FnOnce(&mut WidgetAsset)) {
        self.doc.update(cx, |d, cx| d.edit(label, None, cx, change));
    }

    fn report(&self, result: Result<(), OpError>, cx: &mut App) {
        if let Err(error) = result {
            self.say(error.to_string(), cx);
        }
    }

    /// The nearest container, starting at `from` and going up, that can take a child.
    pub fn container_for(&self, from: Option<NodeId>, cx: &App) -> NodeId {
        let widget = self.widget(cx);
        let mut at = from;
        while let Some(id) = at {
            if widget.can_accept(id).is_ok() {
                return id;
            }
            at = widget.parent_of(id);
        }
        widget.root.id
    }

    // ── Adding ─────────────────────────────────────────────────────────────

    /// Add a new widget of `kind`. With no `drop`, it goes into the selected
    /// container (or beside the selected widget).
    pub fn add_node(&self, kind: Kind, drop: Option<DropSpot>, cx: &mut App) -> Option<NodeId> {
        let drop = drop.unwrap_or_else(|| DropSpot { container: self.container_for(self.primary(cx), cx), index: None, at: None });
        let mut created = None;
        let mut failure = None;
        self.edit("Add widget", cx, |w| match build_node(w, kind) {
            node => {
                let id = node.id;
                match place(w, node, drop) {
                    Ok(()) => created = Some(id),
                    Err(e) => failure = Some(e),
                }
            }
        });
        self.report(failure.map_or(Ok(()), Err), cx);
        if let Some(id) = created {
            self.select(vec![id], cx);
        }
        created
    }

    /// Add an instance of the widget `target`, refusing one that would contain itself.
    pub fn add_instance(&self, target: &WidgetId, name: &str, drop: Option<DropSpot>, cx: &mut App) -> Option<NodeId> {
        let this = self.widget(cx).id.clone();
        let library = self.library_now(cx);
        if library.widget_exists(target) && would_cycle(&library, &this, target) {
            self.say(format!("{name} can't be added: it contains this widget, so each would contain the other"), cx);
            return None;
        }
        let drop = drop.unwrap_or_else(|| DropSpot { container: self.container_for(self.primary(cx), cx), index: None, at: None });
        let mut created = None;
        let mut failure = None;
        let name = name.to_string();
        let target = target.clone();
        self.edit("Add widget", cx, |w| {
            let mut node = w.new_instance(target, &name);
            apply_canvas_defaults(w, &mut node, Kind::Instance, drop);
            let id = node.id;
            match place(w, node, drop) {
                Ok(()) => created = Some(id),
                Err(e) => failure = Some(e),
            }
        });
        self.report(failure.map_or(Ok(()), Err), cx);
        if let Some(id) = created {
            self.select(vec![id], cx);
        }
        created
    }

    /// Add an image widget showing `path`.
    pub fn add_image(&self, path: &str, drop: Option<DropSpot>, cx: &mut App) -> Option<NodeId> {
        let id = self.add_node(Kind::Image, drop, cx)?;
        let path = path.to_string();
        self.edit("Set image", cx, |w| {
            if let Some(n) = w.node_mut(id) {
                n.set_prop("source", json!(path));
            }
        });
        Some(id)
    }

    // ── Removing and copying ───────────────────────────────────────────────

    /// The selection without nodes whose ancestor is also selected (they go with it).
    pub fn top_level(&self, ids: &[NodeId], cx: &App) -> Vec<NodeId> {
        let widget = self.widget(cx);
        ids.iter()
            .copied()
            .filter(|&id| id != widget.root.id)
            .filter(|&id| !ids.iter().any(|&other| other != id && widget.is_within(other, id)))
            .collect()
    }

    pub fn delete_selected(&self, cx: &mut App) {
        let ids = self.top_level(&self.selected(cx), cx);
        if ids.is_empty() {
            if self.selected(cx).contains(&self.widget(cx).root.id) {
                self.say("The root can't be deleted", cx);
            }
            return;
        }
        let parent = self.widget(cx).parent_of(ids[0]);
        self.edit("Delete", cx, |w| {
            for id in &ids {
                let _ = w.remove(*id);
            }
        });
        self.select(parent.into_iter().collect(), cx);
    }

    pub fn duplicate_selected(&self, cx: &mut App) {
        let ids = self.top_level(&self.selected(cx), cx);
        if ids.is_empty() {
            return;
        }
        let mut copies = Vec::new();
        let mut failure = None;
        self.edit("Duplicate", cx, |w| {
            for id in &ids {
                match w.duplicate(*id) {
                    Ok(copy) => {
                        nudge_canvas_child(w, copy, Vec2::new(PASTE_OFFSET, PASTE_OFFSET));
                        copies.push(copy);
                    }
                    Err(e) => failure = Some(e),
                }
            }
        });
        self.report(failure.map_or(Ok(()), Err), cx);
        if !copies.is_empty() {
            self.select(copies, cx);
        }
    }

    pub fn copy_selected(&self, cx: &mut App) -> bool {
        let ids = self.top_level(&self.selected(cx), cx);
        if ids.is_empty() {
            return false;
        }
        let widget = self.widget(cx);
        let nodes: Vec<Json> = ids
            .iter()
            .filter_map(|&id| {
                let node = widget.node(id)?;
                let mut json = node.to_json();
                json["slot_kind"] = json!(widget.slot_kind_of(id).key());
                Some(json)
            })
            .collect();
        let text = serde_json::to_string(&json!({ CLIPBOARD_TAG: 1, "nodes": nodes })).unwrap_or_default();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        true
    }

    pub fn cut_selected(&self, cx: &mut App) {
        if self.copy_selected(cx) {
            self.delete_selected(cx);
        }
    }

    pub fn can_paste(&self, cx: &App) -> bool {
        cx.read_from_clipboard().and_then(|c| c.text()).is_some_and(|t| t.contains(CLIPBOARD_TAG))
    }

    /// Paste what was copied into the selected container (or beside the selected widget).
    pub fn paste(&self, cx: &mut App) {
        let Some(text) = cx.read_from_clipboard().and_then(|c| c.text()) else { return };
        let Ok(value) = serde_json::from_str::<Json>(&text) else { return };
        if value.get(CLIPBOARD_TAG).is_none() {
            return;
        }
        let container = self.container_for(self.primary(cx), cx);
        let mut created = Vec::new();
        let mut failure = None;
        self.edit("Paste", cx, |w| {
            for raw in value.get("nodes").and_then(Json::as_array).into_iter().flatten() {
                let slot = raw.get("slot_kind").and_then(Json::as_str).and_then(SlotKind::from_key).unwrap_or(SlotKind::None);
                let Some(source) = Node::from_json(raw, slot, &mut Vec::new()) else { continue };
                let copy = w.fresh_copy(&source);
                let id = copy.id;
                match w.add(container, None, copy) {
                    Ok(_) => {
                        nudge_canvas_child(w, id, Vec2::new(PASTE_OFFSET, PASTE_OFFSET));
                        created.push(id);
                    }
                    Err(e) => failure = Some(e),
                }
            }
        });
        self.report(failure.map_or(Ok(()), Err), cx);
        if !created.is_empty() {
            self.select(created, cx);
        }
    }

    // ── Structure ──────────────────────────────────────────────────────────

    pub fn wrap_selected(&self, kind: Kind, cx: &mut App) {
        let ids = self.top_level(&self.selected(cx), cx);
        let ids = if ids.is_empty() { self.selected(cx) } else { ids };
        if ids.is_empty() {
            return;
        }
        let mut made = None;
        let mut failure = None;
        self.edit("Wrap", cx, |w| match w.wrap(&ids, kind) {
            Ok(id) => made = Some(id),
            Err(e) => failure = Some(e),
        });
        self.report(failure.map_or(Ok(()), Err), cx);
        if let Some(id) = made {
            self.select(vec![id], cx);
        }
    }

    pub fn unwrap_selected(&self, cx: &mut App) {
        let Some(id) = self.primary(cx) else { return };
        let children: Vec<NodeId> = self.widget(cx).node(id).map(|n| n.children.iter().map(|c| c.id).collect()).unwrap_or_default();
        let mut failure = None;
        self.edit("Unwrap", cx, |w| {
            if let Err(e) = w.unwrap(id) {
                failure = Some(e);
            }
        });
        let ok = failure.is_none();
        self.report(failure.map_or(Ok(()), Err), cx);
        if ok {
            self.select(children, cx);
        }
    }

    /// Move `ids` under `drop.container`. On a canvas, `drop.at` is where the
    /// first one's corner goes and the rest keep their relative places.
    pub fn move_nodes(&self, ids: &[NodeId], drop: DropSpot, cx: &mut App) {
        let frame = self.frame();
        let frame = frame.as_deref();
        let ids = self.top_level(ids, cx);
        if ids.is_empty() {
            return;
        }
        let mut failure = None;
        let frame_rects: Vec<(NodeId, Option<Rect>)> = ids.iter().map(|&id| (id, frame.and_then(|f| f.layout.get(&id).copied()))).collect();
        let container_rect = frame.and_then(|f| f.layout.get(&drop.container).copied());
        self.edit("Move", cx, |w| {
            let first_origin = frame_rects.iter().find_map(|(_, r)| r.map(|r| r.origin()));
            let mut index = drop.index;
            for (id, rect) in &frame_rects {
                let was_canvas_child = w.slot_kind_of(*id) == SlotKind::Canvas;
                if let Err(e) = w.move_node(*id, drop.container, index) {
                    failure = Some(e);
                    continue;
                }
                index = index.map(|i| i + 1);
                let now_canvas = w.slot_kind_of(*id) == SlotKind::Canvas;
                if now_canvas && !was_canvas_child {
                    // Arrived from a layout that did its own placing: keep the size it had.
                    let (Some(rect), Some(at), Some(cont)) = (rect, drop.at, container_rect) else { continue };
                    let delta = first_origin.map_or(Vec2::ZERO, |o| Vec2::new(rect.x - o.x, rect.y - o.y));
                    let wanted = Rect::new(at.x - cont.x + delta.x, at.y - cont.y + delta.y, rect.w, rect.h);
                    if let Some(n) = w.node_mut(*id) {
                        n.set_slot(SlotKind::Canvas, "offsets", json!([wanted.x, wanted.y, wanted.w, wanted.h]));
                        n.set_slot(SlotKind::Canvas, "auto_size", json!(false));
                    }
                } else if now_canvas && was_canvas_child {
                    if let (Some(rect), Some(at), Some(cont)) = (rect, drop.at, container_rect) {
                        let delta = first_origin.map_or(Vec2::ZERO, |o| Vec2::new(rect.x - o.x, rect.y - o.y));
                        let wanted = Rect::new(at.x + delta.x, at.y + delta.y, rect.w, rect.h);
                        set_canvas_rect(w, *id, cont, wanted);
                    }
                }
            }
        });
        self.report(failure.map_or(Ok(()), Err), cx);
        self.select(ids, cx);
    }

    /// Move the selection `by` places among its siblings (negative is earlier,
    /// i.e. further back); `to_end` sends it all the way.
    pub fn reorder_selected(&self, by: i32, to_end: bool, cx: &mut App) {
        let ids = self.top_level(&self.selected(cx), cx);
        let label = if by < 0 { "Send backward" } else { "Bring forward" };
        let mut failure = None;
        self.edit(label, cx, |w| {
            let mut ordered = ids.clone();
            // Move the ones nearest the destination first so they don't block each other.
            ordered.sort_by_key(|&id| w.index_in_parent(id).unwrap_or(0));
            if by > 0 {
                ordered.reverse();
            }
            for id in ordered {
                let (Some(parent), Some(at)) = (w.parent_of(id), w.index_in_parent(id)) else { continue };
                let len = w.node(parent).map_or(0, |p| p.children.len());
                let target = if to_end {
                    if by > 0 {
                        len
                    } else {
                        0
                    }
                } else {
                    (at as i32 + by).clamp(0, len as i32 - 1) as usize + usize::from(by > 0)
                };
                if let Err(e) = w.move_node(id, parent, Some(target)) {
                    failure = Some(e);
                }
            }
        });
        self.report(failure.map_or(Ok(()), Err), cx);
    }

    // ── Layout on a canvas ─────────────────────────────────────────────────

    /// Move canvas children by whole-pixel steps.
    pub fn nudge_selected(&self, delta: Vec2, cx: &mut App) {
        let ids = self.top_level(&self.selected(cx), cx);
        let mut moved = false;
        self.doc.update(cx, |d, cx| {
            d.edit("Nudge", Some("nudge"), cx, |w| {
                for id in &ids {
                    moved |= nudge_canvas_child(w, *id, delta);
                }
            })
        });
        if !moved && !ids.is_empty() {
            self.say("Only widgets on a canvas can be nudged; use the hierarchy to reorder the others", cx);
        }
    }

    /// Line the selection up with the primary selected widget.
    pub fn align_selected(&self, kind: AlignKind, cx: &mut App) {
        let Some(frame) = self.frame() else { return };
        let ids = self.canvas_selection(cx);
        let Some(primary) = self.primary(cx).filter(|p| ids.contains(p)) else {
            self.say("Select widgets on a canvas to align them", cx);
            return;
        };
        let items: Vec<(NodeId, Rect)> = ids.iter().filter_map(|&id| frame.layout.get(&id).map(|r| (id, *r))).collect();
        let moves = geometry::align(&items, primary, kind);
        self.apply_moves(&moves, "Align", cx);
    }

    pub fn distribute_selected(&self, horizontal: bool, cx: &mut App) {
        let Some(frame) = self.frame() else { return };
        let ids = self.canvas_selection(cx);
        let items: Vec<(NodeId, Rect)> = ids.iter().filter_map(|&id| frame.layout.get(&id).map(|r| (id, *r))).collect();
        if items.len() < 3 {
            self.say("Select at least three widgets on a canvas to distribute them", cx);
            return;
        }
        let moves = geometry::distribute(&items, horizontal);
        self.apply_moves(&moves, "Distribute", cx);
    }

    fn canvas_selection(&self, cx: &App) -> Vec<NodeId> {
        let widget = self.widget(cx);
        self.top_level(&self.selected(cx), cx).into_iter().filter(|&id| widget.slot_kind_of(id) == SlotKind::Canvas).collect()
    }

    fn apply_moves(&self, moves: &[(NodeId, Vec2)], label: &'static str, cx: &mut App) {
        self.edit(label, cx, |w| {
            for (id, d) in moves {
                nudge_canvas_child(w, *id, *d);
            }
        });
    }

    /// Pin the selected canvas widgets with `preset`, keeping them where they are.
    pub fn set_anchors(&self, preset: AnchorPreset, with_alignment: bool, cx: &mut App) {
        let Some(frame) = self.frame() else { return };
        let ids = self.canvas_selection(cx);
        if ids.is_empty() {
            self.say("Anchors apply to widgets on a canvas", cx);
            return;
        }
        let (amin, amax) = preset.anchors();
        let rects: Vec<(NodeId, Option<Rect>, Option<Rect>)> = ids
            .iter()
            .map(|&id| {
                let parent = self.widget(cx).parent_of(id).and_then(|p| frame.layout.get(&p).copied());
                (id, frame.layout.get(&id).copied(), parent)
            })
            .collect();
        self.edit("Set anchors", cx, |w| {
            for (id, rect, parent) in &rects {
                let (Some(rect), Some(parent)) = (rect, parent) else { continue };
                let Some(node) = w.node_mut(*id) else { continue };
                let align = if with_alignment { preset.alignment() } else { node.slot_view(SlotKind::Canvas).vec2("alignment") };
                let offsets = offsets_for(amin, amax, align, *parent, *rect);
                node.set_slot(SlotKind::Canvas, "anchor_min", json!([amin.x, amin.y]));
                node.set_slot(SlotKind::Canvas, "anchor_max", json!([amax.x, amax.y]));
                node.set_slot(SlotKind::Canvas, "alignment", json!([align.x, align.y]));
                node.set_slot(SlotKind::Canvas, "offsets", json!(offsets));
                node.set_slot(SlotKind::Canvas, "auto_size", json!(false));
            }
        });
    }

    // ── Flags and names ────────────────────────────────────────────────────

    pub fn toggle_hidden(&self, id: NodeId, cx: &mut App) {
        self.edit("Show or hide in designer", cx, |w| {
            if let Some(n) = w.node_mut(id) {
                n.editor.hidden = !n.editor.hidden;
            }
        });
    }

    pub fn toggle_locked(&self, id: NodeId, cx: &mut App) {
        self.edit("Lock or unlock", cx, |w| {
            if let Some(n) = w.node_mut(id) {
                n.editor.locked = !n.editor.locked;
            }
        });
    }

    pub fn set_expanded(&self, id: NodeId, expanded: bool, cx: &mut App) {
        // Expansion is a view preference, saved with the file but not worth an undo step.
        self.doc.update(cx, |d, cx| {
            d.live(cx, |w| {
                if let Some(n) = w.node_mut(id) {
                    n.editor.expanded = expanded;
                }
            })
        });
    }

    pub fn rename(&self, id: NodeId, name: &str, cx: &mut App) {
        let name = name.to_string();
        let mut failure = None;
        self.doc.update(cx, |d, cx| {
            d.edit("Rename", Some("rename"), cx, |w| {
                if let Err(e) = w.rename(id, &name) {
                    failure = Some(e);
                }
            })
        });
        self.report(failure.map_or(Ok(()), Err), cx);
    }

    // ── Parameters ─────────────────────────────────────────────────────────

    /// Make a parameter from the current value of `prop` on `node` and let it drive that property.
    pub fn parameterize(&self, node: NodeId, prop: &str, cx: &mut App) -> Option<ParamId> {
        let prop = prop.to_string();
        let mut made = None;
        self.edit("Create parameter", cx, |w| {
            let Some(n) = w.node(node) else { return };
            let Some(def) = n.info().def(&prop) else { return };
            let class = def.ty.class();
            let current = n.view().json(&prop).into_owned();
            let name = format!("{}_{}", n.name, def.label.replace(' ', ""));
            let id = w.add_parameter(&name, class);
            w.adopt_value(id, current);
            if w.link_parameter(id, node, &prop).is_ok() {
                made = Some(id);
            }
        });
        made
    }

    pub fn link_parameter(&self, param: ParamId, node: NodeId, prop: &str, cx: &mut App) {
        let prop = prop.to_string();
        let mut failure = None;
        self.edit("Link parameter", cx, |w| {
            if let Err(e) = w.link_parameter(param, node, &prop) {
                failure = Some(e);
            }
        });
        self.report(failure.map_or(Ok(()), Err), cx);
    }

    pub fn unlink_property(&self, node: NodeId, prop: &str, cx: &mut App) {
        let prop = prop.to_string();
        self.edit("Unlink parameter", cx, |w| w.unlink_property(node, &prop));
    }

    pub fn add_parameter(&self, name: &str, class: ValueClass, cx: &mut App) -> Option<ParamId> {
        let mut made = None;
        let name = name.to_string();
        self.edit("Add parameter", cx, |w| made = Some(w.add_parameter(&name, class)));
        made
    }

    pub fn remove_parameter(&self, id: ParamId, cx: &mut App) {
        self.edit("Remove parameter", cx, |w| w.remove_parameter(id));
    }

    pub fn select_all(&self, cx: &mut App) {
        let widget = self.widget(cx);
        let mut ids = Vec::new();
        widget.root.walk(&mut |n| {
            if n.id != widget.root.id && !n.editor.locked && !n.editor.hidden {
                ids.push(n.id);
            }
        });
        self.select(ids, cx);
    }

    /// Copy canvas offsets of every selected widget into the same place on a screen.
    pub fn set_canvas_rect(&self, id: NodeId, parent: Rect, wanted: Rect, cx: &mut App) {
        self.doc.update(cx, |d, cx| d.live(cx, |w| set_canvas_rect(w, id, parent, wanted)));
    }
}

// ── Pieces shared with the canvas ──────────────────────────────────────────

/// A new node of `kind`, with the children a fresh one of that kind should have.
fn build_node(w: &mut WidgetAsset, kind: Kind) -> Node {
    let mut node = w.new_node(kind);
    if kind == Kind::NamedSlot {
        node.name = w.unique_name("Content");
    }
    if kind == Kind::Button {
        let mut label = w.new_node(Kind::Text);
        label.set_prop("text", json!("Button"));
        label.set_slot(SlotKind::Single, "h_align", json!("center"));
        label.set_slot(SlotKind::Single, "v_align", json!("center"));
        node.children.push(label);
    }
    node
}

/// Put `node` where `drop` says, giving it the size it should have on a canvas.
fn place(w: &mut WidgetAsset, mut node: Node, drop: DropSpot) -> Result<(), OpError> {
    let kind = node.kind;
    apply_canvas_defaults(w, &mut node, kind, drop);
    w.add(drop.container, drop.index, node).map(|_| ())
}

fn apply_canvas_defaults(w: &WidgetAsset, node: &mut Node, kind: Kind, drop: DropSpot) {
    let on_canvas = w.node(drop.container).is_some_and(|c| c.info().slot == SlotKind::Canvas);
    if !on_canvas {
        return;
    }
    let at = drop.at.unwrap_or(Vec2::new(32.0, 32.0));
    match default_canvas_size(kind) {
        Some(size) => {
            node.set_slot(SlotKind::Canvas, "offsets", json!([at.x.round(), at.y.round(), size.x, size.y]));
        }
        None => {
            node.set_slot(SlotKind::Canvas, "offsets", json!([at.x.round(), at.y.round(), 100.0, 30.0]));
            node.set_slot(SlotKind::Canvas, "auto_size", json!(true));
        }
    }
}

/// Move `id` by `delta` if it sits on a canvas. Returns whether it did.
pub fn nudge_canvas_child(w: &mut WidgetAsset, id: NodeId, delta: Vec2) -> bool {
    if w.slot_kind_of(id) != SlotKind::Canvas {
        return false;
    }
    let Some(node) = w.node_mut(id) else { return false };
    let v = node.slot_view(SlotKind::Canvas);
    let (amin, amax, o) = (v.vec2("anchor_min"), v.vec2("anchor_max"), v.quad("offsets"));
    let moved = translate_offsets(o, amin, amax, delta.x, delta.y);
    node.set_slot(SlotKind::Canvas, "offsets", json!(moved));
    true
}

/// Place canvas child `id` at `wanted` (layout space) in a parent of `parent` size.
pub fn set_canvas_rect(w: &mut WidgetAsset, id: NodeId, parent: Rect, wanted: Rect) {
    if w.slot_kind_of(id) != SlotKind::Canvas {
        return;
    }
    let Some(node) = w.node_mut(id) else { return };
    let v = node.slot_view(SlotKind::Canvas);
    let offsets = offsets_for(v.vec2("anchor_min"), v.vec2("anchor_max"), v.vec2("alignment"), parent, wanted);
    node.set_slot(SlotKind::Canvas, "offsets", json!(offsets));
    // Giving it a size by hand means it no longer follows its content.
    node.set_slot(SlotKind::Canvas, "auto_size", json!(false));
}

// ── Properties ─────────────────────────────────────────────────────────────

impl Ctx {
    /// Set property `key` on every selected widget that has it (and isn't
    /// driven by a parameter). Quick successive edits of one property are one
    /// undo step.
    pub fn set_prop(&self, key: &'static str, value: Json, cx: &mut App) {
        let ids = self.selected(cx);
        self.doc.update(cx, |d, cx| {
            d.edit("Change property", Some(key), cx, |w| {
                for id in &ids {
                    if w.driver_of(*id, key).is_some() {
                        continue;
                    }
                    let Some(node) = w.node_mut(*id) else { continue };
                    let Some(valid) = node.info().def(key).and_then(|d| d.ty.coerce(&value).or_else(|| (d.optional && value.is_null()).then(|| value.clone()))) else { continue };
                    node.set_prop(key, valid);
                }
            })
        });
    }

    /// Put property `key` back to its default on every selected widget.
    pub fn reset_prop(&self, key: &'static str, cx: &mut App) {
        let ids = self.selected(cx);
        self.edit("Reset property", cx, |w| {
            for id in &ids {
                if w.driver_of(*id, key).is_some() {
                    continue;
                }
                if let Some(node) = w.node_mut(*id) {
                    node.reset_prop(key);
                }
            }
        });
    }

    /// Set a layout property of the selected widgets in their parents.
    pub fn set_slot_prop(&self, key: &'static str, value: Json, cx: &mut App) {
        let ids = self.selected(cx);
        self.doc.update(cx, |d, cx| {
            d.edit("Change layout", Some(key), cx, |w| {
                for id in &ids {
                    let kind = w.slot_kind_of(*id);
                    if let Some(node) = w.node_mut(*id) {
                        node.set_slot(kind, key, value.clone());
                    }
                }
            })
        });
    }

    pub fn reset_slot_prop(&self, key: &'static str, cx: &mut App) {
        let ids = self.selected(cx);
        self.edit("Reset layout", cx, |w| {
            for id in &ids {
                if let Some(node) = w.node_mut(*id) {
                    node.slot.remove(key);
                }
            }
        });
    }

    /// Change what an instance passes for one of the used widget's parameters
    /// (`None` goes back to the parameter's default).
    pub fn set_override(&self, node: NodeId, param: ParamId, arg: Option<crate::model::Arg>, cx: &mut App) {
        self.doc.update(cx, |d, cx| {
            d.edit("Change widget value", Some("override"), cx, |w| {
                let Some(instance) = w.node_mut(node).and_then(|n| n.instance.as_mut()) else { return };
                match arg {
                    Some(arg) => {
                        instance.overrides.insert(param, arg);
                    }
                    None => {
                        instance.overrides.remove(&param);
                    }
                }
            })
        });
    }

    /// Point an instance at a different widget, keeping the values that still fit.
    pub fn retarget_instance(&self, node: NodeId, target: &WidgetId, name: &str, cx: &mut App) {
        let this = self.widget(cx).id.clone();
        let library = self.library_now(cx);
        if would_cycle(&library, &this, target) {
            self.say(format!("{name} can't be used here: it contains this widget"), cx);
            return;
        }
        let (target, name) = (target.clone(), name.to_string());
        self.edit("Replace widget", cx, |w| {
            if let Some(instance) = w.node_mut(node).and_then(|n| n.instance.as_mut()) {
                instance.widget = target;
                instance.name_hint = name;
                instance.overrides.clear();
            }
        });
    }

    /// `path` relative to the project, with `/` separators, when it is inside it.
    pub fn project_relative(&self, path: &str, cx: &App) -> String {
        let root = self.lib.read(cx).root().to_path_buf();
        let p = std::path::Path::new(path);
        p.strip_prefix(&root).map_or_else(|_| path.replace('\\', "/"), |r| r.to_string_lossy().replace('\\', "/"))
    }
}
