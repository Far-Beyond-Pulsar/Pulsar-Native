//! Phase 2 of a restore: attach a validated [`ResolvedLayout`] to the live UI.
//!
//! Everything fallible happened in [`resolve`](super::resolve); this only
//! builds dock items from the plan and installs them. It is the single place a
//! restore mutates the dock.
//!
//! Order: centre (filling the anchor tab group in place), side docks, then the
//! live side-dock panels no plan mentioned. Later steps add tile / pop-out
//! roots and the zoom after the trees are installed.

use std::sync::Arc;

use gpui::{App, Entity, WeakEntity};
use ui::dock::{DockArea, DockItem, DockPlacement, PanelView, TabPanel};

use super::resolve::{Plan, ResolvedLayout};

type PanelPlan = Plan<Arc<dyn PanelView>>;

struct Builder<'a> {
    dock_area: WeakEntity<DockArea>,
    anchor_panels: &'a std::collections::HashSet<gpui::EntityId>,
    /// The centre tab group to keep. Taken by the first group that holds one of
    /// its panels.
    anchor: Option<Entity<TabPanel>>,
}

impl Builder<'_> {
    fn build(&mut self, plan: PanelPlan, window: &mut gpui::Window, cx: &mut App) -> DockItem {
        match plan {
            Plan::Split {
                axis,
                sizes,
                children,
            } => {
                let items = children
                    .into_iter()
                    .map(|child| self.build(child, window, cx))
                    .collect();
                DockItem::split_with_sizes(axis, items, sizes, &self.dock_area, window, cx)
            }
            Plan::Tabs { panels, active } => {
                let holds_anchor_panel = panels
                    .iter()
                    .any(|p| self.anchor_panels.contains(&p.view().entity_id()));
                if holds_anchor_panel {
                    if let Some(anchor) = self.anchor.take() {
                        return fill_anchor(anchor, &panels, active, window, cx);
                    }
                }
                DockItem::tabs(panels, Some(active), &self.dock_area, window, cx)
            }
            Plan::Tiles { metas, children } => {
                let items = children
                    .into_iter()
                    .map(|child| self.build(child, window, cx))
                    .collect();
                DockItem::tiles(items, metas, &self.dock_area, window, cx)
            }
        }
    }
}

/// Put `panels` into the existing `anchor` tab group in saved order. The panels
/// already in it are skipped by `insert_panel_at`, so inserting each at its
/// saved index leaves the group in exactly the saved order.
fn fill_anchor(
    anchor: Entity<TabPanel>,
    panels: &[Arc<dyn PanelView>],
    active: usize,
    window: &mut gpui::Window,
    cx: &mut App,
) -> DockItem {
    let items = anchor.update(cx, |tabs, cx| {
        for (ix, panel) in panels.iter().enumerate() {
            let at = ix.min(tabs.all_panels().len());
            tabs.insert_panel_at(panel.clone(), at, window, cx);
        }
        tabs.set_active_tab(active, window, cx);
        tabs.all_panels()
    });
    let active_ix = anchor
        .read(cx)
        .active_tab_index()
        .unwrap_or(active.min(items.len().saturating_sub(1)));
    DockItem::Tabs {
        items,
        active_ix,
        view: anchor,
    }
}

/// Install `resolved` into `dock_area`. `anchor` is the centre tab group to keep
/// and `anchor_panels` the ids of the panels it held when the plan was resolved.
pub(super) fn commit(
    resolved: ResolvedLayout,
    dock_area: &Entity<DockArea>,
    anchor: &Entity<TabPanel>,
    anchor_panels: &std::collections::HashSet<gpui::EntityId>,
    window: &mut gpui::Window,
    cx: &mut App,
) {
    let ResolvedLayout {
        center,
        docks,
        unmentioned,
        dormant: _,
        zoomed,
    } = resolved;

    let mut builder = Builder {
        dock_area: dock_area.downgrade(),
        anchor_panels,
        anchor: Some(anchor.clone()),
    };

    if let Some(center) = center {
        let item = builder.build(center, window, cx);
        dock_area.update(cx, |area, cx| area.set_center(item, window, cx));
    }

    // Only the centre may take the anchor group; a side dock never owns it.
    builder.anchor = None;
    for dock in docks {
        let item = builder.build(dock.plan, window, cx);
        let (size, open) = (Some(dock.size), dock.open);
        dock_area.update(cx, |area, cx| match dock.placement {
            DockPlacement::Left => area.set_left_dock(item, size, open, window, cx),
            DockPlacement::Right => area.set_right_dock(item, size, open, window, cx),
            DockPlacement::Bottom => area.set_bottom_dock(item, size, open, window, cx),
            DockPlacement::Center => {}
        });
    }

    // A side-dock panel the saved layout did not mention goes back where it was
    // instead of disappearing.
    for live in unmentioned {
        dock_area.update(cx, |area, cx| {
            area.add_panel(live.panel, live.placement, None, window, cx)
        });
    }

    // Zoom last, once every group it could name exists. It is found by the
    // identity of its active panel (then tree position), never by a runtime id;
    // a zoom that matches nothing is dropped.
    if let Some(zoomed) = zoomed {
        let applied = dock_area.update(cx, |area, cx| area.apply_zoomed(&zoomed, window, cx));
        if !applied {
            tracing::debug!("layout: saved zoom matches no tab group; dropped");
        }
    }
}
