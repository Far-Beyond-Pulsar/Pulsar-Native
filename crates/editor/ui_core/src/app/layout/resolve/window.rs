//! Whole-window resolution: every root of a saved window, validated as one
//! layout.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{App, Entity, EntityId, Pixels, Window};
use ui::dock::{DockArea, DockPlacement, PanelState, PanelView, TabPanel, ZoomedState};

use super::super::dormant::{DormantLeaf, DormantRoot};
use super::super::format::{SavedPopout, SavedWindowLayout};
use super::super::window_geometry;

use super::*;

/// A resolved side dock.
pub(in crate::app::layout) struct DockPlan {
    pub placement: DockPlacement,
    pub plan: Plan<Arc<dyn PanelView>>,
    pub size: Pixels,
    pub open: bool,
}

/// A resolved pop-out window, ready to open.
pub(in crate::app::layout) struct PopoutPlan {
    /// The saved pop-out's id; kept while the window lives.
    pub id: u32,
    /// The zoom to apply once the window is open.
    pub zoomed: Option<ZoomedState>,
    /// Validated against the connected displays.
    pub bounds: gpui::WindowBounds,
    /// The windowed size a maximized pop-out returns to.
    pub restore_bounds: Option<gpui::Bounds<Pixels>>,
    pub plan: Plan<Arc<dyn PanelView>>,
}

/// A validated, fully resolved window layout, ready to commit.
pub(in crate::app::layout) struct ResolvedLayout {
    /// `None` when the saved centre was unusable: the live centre stays.
    pub center: Option<Plan<Arc<dyn PanelView>>>,
    pub docks: Vec<DockPlan>,
    /// Live side-dock panels no plan mentions; they go back where they were.
    pub unmentioned: Vec<LivePanel>,
    /// Identified tabs nothing is registered to build; saved back verbatim.
    pub dormant: Vec<DormantLeaf>,
    /// The zoomed group, applied by identity after the trees are installed.
    pub zoomed: Option<ZoomedState>,
    /// Pop-out windows to open once the main layout is installed.
    pub popouts: Vec<PopoutPlan>,
    /// Owned panels the plans claimed (panels of an editor's own dock) and the
    /// tab group each sits in now. Commit takes them out of it.
    pub owned_homes: HashMap<EntityId, Entity<TabPanel>>,
    /// Saved pop-outs that open nothing because everything they held is
    /// dormant. They stay in the file (empty, with their id and bounds) so the
    /// dormant tabs have a pop-out to come back to.
    pub ghost_popouts: Vec<SavedPopout>,
}

/// Validation bookkeeping shared by the roots of one window.
struct ResolveState {
    claims: Claims,
    /// Panels already present in an admitted plan.
    seen: HashSet<EntityId>,
    /// Tabs shelved by the resolver, across all roots.
    dormant: Vec<DormantLeaf>,
    /// The tab group each claimed owned panel sits in now.
    homes: HashMap<EntityId, Entity<TabPanel>>,
}

impl ResolveState {
    /// Accept `plan` if none of its panels is already in an admitted plan
    /// (each live panel is claimed at most once). On success its panels count
    /// as taken.
    fn admit(&mut self, plan: &Plan<Arc<dyn PanelView>>) -> bool {
        let ids: Vec<EntityId> = plan.leaves().iter().map(|p| p.view().entity_id()).collect();
        let unique: HashSet<EntityId> = ids.iter().copied().collect();
        if unique.len() != ids.len() || !unique.is_disjoint(&self.seen) {
            return false;
        }
        self.seen.extend(unique);
        true
    }

    /// Hand back the live panels `plan` had claimed.
    fn release(&mut self, plan: &Plan<Arc<dyn PanelView>>) {
        for panel in plan.leaves() {
            let id = panel.view().entity_id();
            self.claims.0.remove(&id);
            self.homes.remove(&id);
        }
    }
}

/// Resolve every root of `saved` against the live `snapshot`, without touching
/// the live UI.
pub(in crate::app::layout) fn resolve_layout(
    saved: &SavedWindowLayout,
    snapshot: &LiveSnapshot,
    project_root: &std::path::Path,
    dock_area: &Entity<DockArea>,
    window: &mut Window,
    cx: &mut App,
) -> ResolvedLayout {
    let mut state = ResolveState {
        claims: Claims::default(),
        seen: HashSet::new(),
        dormant: Vec::new(),
        homes: HashMap::new(),
    };
    let mut owners = HashSet::new();
    collect_owners(&saved.dock.center, &mut owners);
    for dock in [
        saved.dock.left_dock.as_ref(),
        saved.dock.right_dock.as_ref(),
        saved.dock.bottom_dock.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        collect_owners(dock.panel(), &mut owners);
    }
    for popout in &saved.popouts {
        collect_owners(&popout.dock, &mut owners);
    }
    // `claim_live`: whether the root may claim the live panels by name. Pop-outs
    // never do: a live panel sits in the main window's dock.
    let resolve = |state: &mut ResolveState,
                   root: DormantRoot,
                   claim_live: bool,
                   tree: &PanelState,
                   window: &mut Window,
                   cx: &mut App| {
        let mut pending = Vec::new();
        let plan = {
            let mut resolver = Resolver {
                project_root: project_root.to_path_buf(),
                claim_live,
                snapshot,
                claims: &mut state.claims,
                homes: &mut state.homes,
                dormant: &mut pending,
                root,
                owners: &owners,
                window,
                cx,
            };
            resolve_tree(&mut resolver, tree)
        };
        state
            .dormant
            .extend(pending.into_iter().map(|p| p.into_leaf(root)));
        plan
    };

    // Centre. It must hold a panel of the anchor group (the level editor
    // cannot be closed, so a centre without it is corrupt): leave the live
    // centre as it is rather than orphan the entity everything holds.
    let center = resolve(
        &mut state,
        DormantRoot::Center,
        true,
        &saved.dock.center,
        window,
        cx,
    )
    .and_then(|plan| {
        let holds_anchor = plan
            .leaves()
            .iter()
            .any(|p| snapshot.anchor_panels.contains(&p.view().entity_id()));
        if anchor_requirement_met(snapshot.anchor_panels.len(), holds_anchor) && state.admit(&plan)
        {
            Some(plan)
        } else {
            tracing::warn!("layout: saved centre has no editor tab; keeping the default");
            state.release(&plan);
            None
        }
    });

    // Side docks, only those this app has.
    let mut docks = Vec::new();
    let saved_docks = [
        (
            DockPlacement::Left,
            DormantRoot::Left,
            saved.dock.left_dock.as_ref(),
        ),
        (
            DockPlacement::Right,
            DormantRoot::Right,
            saved.dock.right_dock.as_ref(),
        ),
        (
            DockPlacement::Bottom,
            DormantRoot::Bottom,
            saved.dock.bottom_dock.as_ref(),
        ),
    ];
    for (placement, root, saved_dock) in saved_docks {
        let Some(saved_dock) = saved_dock else {
            continue;
        };
        if !dock_area.read(cx).has_dock(placement) {
            continue;
        }
        let Some(plan) = resolve(&mut state, root, true, saved_dock.panel(), window, cx) else {
            continue;
        };
        if !state.admit(&plan) {
            state.release(&plan);
            continue;
        }
        docks.push(DockPlan {
            placement,
            plan,
            size: saved_dock.size(),
            open: saved_dock.is_open(),
        });
    }

    // Pop-outs: more roots, resolved after the main window's so the main
    // layout wins any contested panel. Their windows open at commit.
    let mut popouts = Vec::new();
    let mut ghost_popouts = Vec::new();
    for saved_popout in saved.popouts.iter().take(MAX_POPOUTS) {
        let root = DormantRoot::Popout(saved_popout.id);
        let plan = resolve(&mut state, root, false, &saved_popout.dock, window, cx);
        let admitted = match plan {
            Some(plan) if state.admit(&plan) => Some(plan),
            Some(plan) => {
                state.release(&plan);
                None
            }
            None => None,
        };
        let Some(plan) = admitted else {
            // Nothing opens, but tabs it held that no plugin can build yet stay
            // saved under the pop-out's place in the file.
            if state.dormant.iter().any(|leaf| leaf.root == root) {
                ghost_popouts.push(SavedPopout::empty(saved_popout.id, saved_popout.bounds));
            } else {
                tracing::info!("layout: a saved pop-out resolves to nothing; dropped");
            }
            continue;
        };
        popouts.push(PopoutPlan {
            id: saved_popout.id,
            bounds: window_geometry::fit_to_connected_displays(saved_popout.bounds, cx),
            restore_bounds: Some(saved_popout.bounds.bounds),
            plan,
            zoomed: saved_popout.zoomed.clone(),
        });
    }

    let unmentioned = snapshot
        .panels
        .iter()
        .filter(|live| live.placement != DockPlacement::Center && !state.claims.contains(live.id()))
        .cloned()
        .collect();

    ResolvedLayout {
        center,
        docks,
        unmentioned,
        dormant: state.dormant,
        zoomed: saved.dock.zoomed.clone(),
        popouts,
        owned_homes: state.homes,
        ghost_popouts,
    }
}

/// Every file a tab in `state`'s tree names as its owner.
fn collect_owners(state: &PanelState, out: &mut HashSet<String>) {
    if let Some((owner, _)) = state.owner() {
        out.insert(owner.to_string());
    }
    for child in &state.children {
        collect_owners(child, out);
    }
}

/// Whether a saved centre satisfies the anchor rule. The centre must hold a
/// panel of the anchor group (the level editor) only when this window has one:
/// a window built without a level editor has an empty anchor group and any
/// centre is acceptable.
pub(super) fn anchor_requirement_met(anchor_panel_count: usize, centre_holds_anchor: bool) -> bool {
    anchor_panel_count == 0 || centre_holds_anchor
}
