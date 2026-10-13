//! A widget laid out and ready to draw: the resolved tree, its display list,
//! and where every node of the edited widget ended up.

use std::collections::HashMap;

use crate::display::{self, Cmd, DisplayOptions};
use crate::layout::{self, TextMetrics};
use crate::model::validate::WidgetSource;
use crate::model::{Affine, NodeId, Rect, Vec2, WidgetAsset};
use crate::resolve::{self, ResolvedNode};

/// What a frame was built from; an equal key means the frame is still good.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameKey {
    pub doc_revision: u64,
    pub library_revision: u64,
    pub size: (u32, u32),
    pub options: (u8, bool),
}

impl FrameKey {
    pub fn new(doc_revision: u64, library_revision: u64, size: Vec2, options: &DisplayOptions) -> Self {
        Self {
            doc_revision,
            library_revision,
            size: (size.x.to_bits(), size.y.to_bits()),
            options: (options.state as u8, options.placeholders),
        }
    }
}

pub struct Frame {
    pub key: FrameKey,
    /// The layout size (the screen size under the preview scale).
    pub size: Vec2,
    pub tree: ResolvedNode,
    pub cmds: Vec<Cmd>,
    /// Where each node of the edited widget is drawn (after render transforms).
    pub screen: HashMap<NodeId, Rect>,
    /// Each node's rectangle in layout space, before render transforms.
    pub layout: HashMap<NodeId, Rect>,
    /// Layout space to drawn space for each node.
    pub world: HashMap<NodeId, Affine>,
}

impl Frame {
    pub fn build(
        widget: &WidgetAsset,
        lib: &dyn WidgetSource,
        key: FrameKey,
        size: Vec2,
        options: &DisplayOptions,
        metrics: &dyn TextMetrics,
    ) -> Frame {
        let mut tree = resolve::resolve(widget, lib);
        layout::layout(&mut tree, size, metrics);
        let cmds = display::build(&tree, options);
        let (mut screen, mut layout_rects, mut world) = (HashMap::new(), HashMap::new(), HashMap::new());
        tree.walk(&mut |n| {
            // A node of the edited widget: it owns itself.
            if n.own && n.key.last() == Some(&n.owner) {
                screen.insert(n.owner, display::screen_rect(n));
                layout_rects.insert(n.owner, n.rect);
                world.insert(n.owner, n.world);
            }
        });
        Frame { key, size, tree, cmds, screen, layout: layout_rects, world }
    }

    /// The node to select for a click at `p` (drawn space): the topmost
    /// widget under it that `accept` allows, as a node of the edited widget.
    pub fn pick(&self, p: Vec2, accept: &dyn Fn(NodeId) -> bool) -> Option<NodeId> {
        display::hit_test(&self.tree, p, &|n| accept(n.owner)).map(|n| n.owner)
    }

    /// The node ids of the edited widget whose drawn rectangle meets `area`.
    pub fn in_area(&self, area: Rect, widget: &WidgetAsset, accept: &dyn Fn(NodeId) -> bool) -> Vec<NodeId> {
        let mut hits: Vec<NodeId> = self
            .screen
            .iter()
            .filter(|(&id, rect)| id != widget.root.id && accept(id) && rect.intersects(&area) && !rect.is_empty())
            .filter(|(&id, rect)| {
                // A container is picked only when the area swallows it whole;
                // otherwise the things in it are picked instead.
                widget.node(id).is_some_and(|n| n.children.is_empty() || area_contains(&area, rect))
            })
            .map(|(&id, _)| id)
            .collect();
        hits.sort_unstable();
        hits
    }

    /// The deepest container of the edited widget under `p` that can take another child.
    pub fn container_at(&self, p: Vec2, widget: &WidgetAsset, exclude: &dyn Fn(NodeId) -> bool) -> NodeId {
        let mut best = widget.root.id;
        let mut best_depth = 0;
        for (&id, rect) in &self.screen {
            if exclude(id) || !rect.contains(p) {
                continue;
            }
            if widget.can_accept(id).is_err() {
                continue;
            }
            let depth = widget.path_to(id).map_or(0, |path| path.len());
            if depth >= best_depth {
                best = id;
                best_depth = depth;
            }
        }
        best
    }
}

fn area_contains(area: &Rect, rect: &Rect) -> bool {
    rect.x >= area.x && rect.y >= area.y && rect.right() <= area.right() && rect.bottom() <= area.bottom()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::FixedMetrics;
    use crate::model::validate::NoWidgets;
    use crate::model::Kind;
    use serde_json::json;

    fn widget() -> (WidgetAsset, NodeId, NodeId, NodeId) {
        let mut w = WidgetAsset::new();
        let root = w.root.id;
        let mut panel = w.new_node(Kind::Border);
        panel.set_prop("background", json!("#223344"));
        let panel_id = panel.id;
        w.add(root, None, panel).unwrap();
        w.node_mut(panel_id).unwrap().set_slot(crate::model::SlotKind::Canvas, "offsets", json!([100.0, 100.0, 400.0, 300.0]));
        let mut leaf = w.new_node(Kind::Spacer);
        leaf.set_prop("size", json!([50.0, 50.0]));
        let leaf_id = leaf.id;
        w.add(panel_id, None, leaf).unwrap();
        w.node_mut(leaf_id).unwrap().set_slot(crate::model::SlotKind::Single, "h_align", json!("left"));
        w.node_mut(leaf_id).unwrap().set_slot(crate::model::SlotKind::Single, "v_align", json!("top"));
        (w, root, panel_id, leaf_id)
    }

    fn frame(w: &WidgetAsset) -> Frame {
        let size = Vec2::new(1000.0, 700.0);
        let options = DisplayOptions::default();
        Frame::build(w, &NoWidgets, FrameKey::new(1, 0, size, &options), size, &options, &FixedMetrics)
    }

    #[test]
    fn every_node_of_the_widget_has_a_rectangle() {
        let (w, root, panel, leaf) = widget();
        let f = frame(&w);
        assert_eq!(f.screen[&root], Rect::new(0.0, 0.0, 1000.0, 700.0));
        assert_eq!(f.screen[&panel], Rect::new(100.0, 100.0, 400.0, 300.0));
        assert_eq!(f.screen[&leaf], Rect::new(100.0, 100.0, 50.0, 50.0));
        assert_eq!(f.layout.len(), 3);
    }

    #[test]
    fn picking_finds_the_leaf_then_the_panel_then_nothing() {
        let (w, _, panel, leaf) = widget();
        let f = frame(&w);
        let any = |_: NodeId| true;
        assert_eq!(f.pick(Vec2::new(110.0, 110.0), &any), Some(leaf));
        assert_eq!(f.pick(Vec2::new(300.0, 300.0), &any), Some(panel), "a painted panel takes clicks");
        assert_eq!(f.pick(Vec2::new(900.0, 600.0), &any), None);
        let not_leaf = |id: NodeId| id != leaf;
        assert_eq!(f.pick(Vec2::new(110.0, 110.0), &not_leaf), Some(panel), "a locked widget lets clicks through");
    }

    #[test]
    fn marquee_picks_leaves_it_touches_and_containers_it_swallows() {
        let (w, _, panel, leaf) = widget();
        let f = frame(&w);
        let any = |_: NodeId| true;
        assert_eq!(f.in_area(Rect::new(90.0, 90.0, 30.0, 30.0), &w, &any), vec![leaf]);
        assert_eq!(f.in_area(Rect::new(0.0, 0.0, 600.0, 500.0), &w, &any), vec![panel, leaf]);
        assert!(f.in_area(Rect::new(800.0, 600.0, 10.0, 10.0), &w, &any).is_empty());
    }

    #[test]
    fn drops_land_in_the_deepest_container_that_has_room() {
        let (w, root, panel, leaf) = widget();
        let f = frame(&w);
        let none = |_: NodeId| false;
        assert_eq!(f.container_at(Vec2::new(900.0, 600.0), &w, &none), root);
        // The border holds one child and already has it, and a spacer holds
        // none, so over either the drop goes to the canvas behind them.
        assert_eq!(f.container_at(Vec2::new(300.0, 300.0), &w, &none), root);
        assert_eq!(f.container_at(Vec2::new(110.0, 110.0), &w, &none), root);
        let _ = (panel, leaf);
    }
}
