//! Phase 2 of a restore: attach a validated [`ResolvedLayout`] to the live UI.
//!
//! Everything fallible happened in [`resolve`](super::resolve); this only
//! builds dock items from the plan and installs them. It is the single place a
//! restore mutates the dock.
//!
//! Order: centre (filling the anchor tab group in place), side docks, then the
//! live side-dock panels no plan mentioned, the zoom, and finally the pop-out
//! windows (see [`open_popouts`]).
//!
//! **Owned panels.** A plan can hold panels the resolver *claimed* from another
//! editor's dock (the Blueprint graph canvas...) rather than created. Before a
//! plan is built they are taken out of the tab group they sit in
//! ([`take_owned`], `TabPanel::take_panel`, as dragging a tab out does): a panel
//! shown in two tab groups at once fails to render. A pop-out made only of panels
//! from one tab group returns them there when it is closed, falling back to the
//! centre anchor group when that tab group is gone.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, Entity, EntityId, WeakEntity};
use ui::dock::{
    live_popouts, open_popout_with, DockArea, DockChannel, DockItem, DockPlacement, PanelSource,
    PanelView, PopoutSpec, TabPanel, ZoomedState,
};

use super::resolve::{Plan, PopoutPlan, ResolvedLayout};

type PanelPlan = Plan<Arc<dyn PanelView>>;

/// The tab groups claimed owned panels sit in (see [`ResolvedLayout::owned_homes`]).
type Homes = HashMap<EntityId, Entity<TabPanel>>;

/// Take the claimed owned panels of `plan` out of the tab groups they sit in.
/// A panel that is no longer in its group (moved or closed since the plan was
/// resolved) is dropped from the plan rather than shown twice; what that empties
/// is pruned. Panels that were not claimed from a dock are left alone.
fn take_owned(
    plan: PanelPlan,
    homes: &Homes,
    window: &mut gpui::Window,
    cx: &mut App,
) -> Option<PanelPlan> {
    if homes.is_empty() {
        return Some(plan);
    }
    plan.retain_leaves(&mut |panel| {
        let id = panel.view().entity_id();
        let Some(home) = homes.get(&id) else {
            return true;
        };
        let taken = home.update(cx, |tabs, cx| tabs.take_panel(panel.clone(), window, cx));
        if !taken {
            tracing::warn!("layout: an owned panel moved before it could be restored; skipped");
        }
        taken
    })
}

/// Where closing a pop-out sends its panels: the tab group they all came from,
/// when every panel of the plan was claimed from one. `None` (the centre anchor
/// group) otherwise; `PanelSource` names one tab group per pop-out.
fn common_home(plan: &PanelPlan, homes: &Homes) -> Option<Entity<TabPanel>> {
    let mut common: Option<&Entity<TabPanel>> = None;
    for panel in plan.leaves() {
        let home = homes.get(&panel.view().entity_id())?;
        match common {
            Some(seen) if seen.entity_id() != home.entity_id() => return None,
            _ => common = Some(home),
        }
    }
    common.cloned()
}

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

/// A pop-out window opened by [`commit`]: its handle, its saved id and the
/// windowed bounds it returns to when maximized.
pub(super) type OpenedPopout = (
    gpui::AnyWindowHandle,
    u32,
    Option<gpui::Bounds<gpui::Pixels>>,
);

/// Zoom the group of a freshly opened pop-out that `zoomed` names (by the
/// identity of its active panel, like the main window); dropped if none matches.
fn apply_popout_zoom(handle: gpui::AnyWindowHandle, zoomed: &ZoomedState, cx: &mut App) {
    let Some(dock) = live_popouts(cx)
        .into_iter()
        .find(|p| p.window == handle)
        .and_then(|p| p.dock_area.upgrade())
    else {
        return;
    };
    let applied = handle
        .update(cx, |_, window, cx| {
            dock.update(cx, |area, cx| area.apply_zoomed(zoomed, window, cx))
        })
        .unwrap_or(false);
    if !applied {
        tracing::debug!("layout: saved pop-out zoom matches no tab group; dropped");
    }
}

/// Open the pop-out windows of a restore. Their tabs return to the centre
/// `anchor` group of `parent` when the user closes them, so closing a restored
/// pop-out never loses its tabs.
fn open_popouts(
    popouts: Vec<PopoutPlan>,
    homes: &Homes,
    anchor: &Entity<TabPanel>,
    parent: gpui::AnyWindowHandle,
    window: &mut gpui::Window,
    cx: &mut App,
) -> Vec<OpenedPopout> {
    let mut opened = Vec::new();
    for PopoutPlan {
        id,
        zoomed,
        bounds,
        restore_bounds,
        plan,
    } in popouts
    {
        // Owned panels leave their editor's dock in this (the parent) window,
        // before the pop-out window exists.
        let home = common_home(&plan, homes);
        let Some(plan) = take_owned(plan, homes, window, cx) else {
            tracing::info!("layout: a pop-out lost all its owned panels; not opened");
            continue;
        };
        let build = move |window: &mut gpui::Window, dock: &WeakEntity<DockArea>, cx: &mut App| {
            // A pop-out never holds the anchor group.
            let no_anchor = std::collections::HashSet::new();
            let mut builder = Builder {
                dock_area: dock.clone(),
                anchor_panels: &no_anchor,
                anchor: None,
            };
            builder.build(plan, window, cx)
        };
        let source = PanelSource::tab_panel(home.as_ref().unwrap_or(anchor), parent, None);
        let spec = PopoutSpec::tree(build, bounds, DockChannel::default(), Some(source));
        match open_popout_with(spec, cx) {
            Some(handle) => {
                if let Some(zoomed) = zoomed {
                    apply_popout_zoom(handle, &zoomed, cx);
                }
                opened.push((handle, id, restore_bounds));
            }
            None => tracing::warn!("layout: could not reopen a pop-out window"),
        }
    }
    opened
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
) -> Vec<OpenedPopout> {
    let ResolvedLayout {
        center,
        docks,
        unmentioned,
        dormant: _,
        ghost_popouts: _,
        zoomed,
        popouts,
        owned_homes,
    } = resolved;

    let mut builder = Builder {
        dock_area: dock_area.downgrade(),
        anchor_panels,
        anchor: Some(anchor.clone()),
    };

    let center = center.and_then(|plan| take_owned(plan, &owned_homes, window, cx));
    if let Some(center) = center {
        let item = builder.build(center, window, cx);
        dock_area.update(cx, |area, cx| area.set_center(item, window, cx));
    }

    // Only the centre may take the anchor group; a side dock never owns it.
    builder.anchor = None;
    for dock in docks {
        let Some(plan) = take_owned(dock.plan, &owned_homes, window, cx) else {
            continue;
        };
        let item = builder.build(plan, window, cx);
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

    // Pop-outs last: the main layout is in place, so the anchor group they
    // return their tabs to is final.
    let parent = window.window_handle();
    open_popouts(popouts, &owned_homes, anchor, parent, window, cx)
}
