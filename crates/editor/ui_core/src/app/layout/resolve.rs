//! Phase 1 of a restore: turn a saved tree into a [`Plan`] without touching the
//! live UI.
//!
//! A [`Plan`] is plain data: splits, tab groups and tiles whose leaves are
//! already-resolved panels. Two layers:
//!
//! - [`resolve_tree`] is generic over a [`LeafResolver`] and holds all of the
//!   tree logic (pruning leaves that cannot be resolved, dropping emptied
//!   groups, collapsing a split left with one child, discarding size
//!   information when a side was dropped). It knows nothing about GPUI, so it
//!   is unit tested with a fake resolver.
//! - [`Resolver`] is the GPUI leaf resolver. It looks panels up in a read-only
//!   [`LiveSnapshot`] of what the app created itself and *claims* them by id
//!   (nothing is removed from the live UI), or creates file editors through the
//!   plugin manager. A panel created here is not attached anywhere until
//!   [`commit`](super::commit) runs.
//!
//! [`resolve_layout`] resolves every root of a saved window and validates the
//! result: the centre must hold a panel of the anchor group (the level editor),
//! and no live panel may be claimed twice. A root that fails validation is
//! discarded whole and its claims are released, so the panels it took are
//! available to the other roots and to the "unmentioned live panel" pass.
//!
//! ## Extension points
//!
//! - [`Resolver::resolve_leaf`]: resolution order is live singleton by
//!   `panel_name`, then file. A restorer-by-kind lookup (`restore_key`) slots in
//!   between; a leaf that is identified but unresolvable becomes a dormant
//!   entry there.
//! - [`ResolvedLayout`]: gains tile / pop-out roots and a zoom target next to
//!   `center` and `docks`.
//! - [`Plan::leaves`] / [`admit`](ResolveState::admit): the validation hooks a
//!   zoom-target check would use.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Axis, Entity, EntityId, Pixels, Window};
use ui::dock::{DockArea, DockPlacement, PanelInfo, PanelState, PanelView, TabPanel, TileMeta};

use super::format::SavedWindowLayout;

// ── Plan ─────────────────────────────────────────────────────────────────────

/// A resolved layout tree. `L` is the resolved panel type.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Plan<L> {
    Split {
        axis: Axis,
        /// One entry per child; `None` lets the split size that child itself.
        sizes: Vec<Option<Pixels>>,
        children: Vec<Plan<L>>,
    },
    Tabs {
        panels: Vec<L>,
        active: usize,
    },
    /// Free-floating tab groups (each child is a [`Plan::Tabs`]) with their
    /// bounds; `metas` is parallel to `children`.
    Tiles {
        metas: Vec<TileMeta>,
        children: Vec<Plan<L>>,
    },
}

impl<L> Plan<L> {
    /// Every resolved panel in the tree, in tree order.
    pub fn leaves(&self) -> Vec<&L> {
        let mut out = Vec::new();
        self.collect_leaves(&mut out);
        out
    }

    fn collect_leaves<'a>(&'a self, out: &mut Vec<&'a L>) {
        match self {
            Plan::Split { children, .. } | Plan::Tiles { children, .. } => {
                for child in children {
                    child.collect_leaves(out);
                }
            }
            Plan::Tabs { panels, .. } => out.extend(panels.iter()),
        }
    }
}

/// Resolves one saved leaf (a tab) to a panel, or `None` if it cannot be.
/// Implementations must not attach anything to the live UI.
pub(super) trait LeafResolver {
    type Leaf;
    fn resolve_leaf(&mut self, leaf: &PanelState) -> Option<Self::Leaf>;
}

/// Build the plan for a saved tree. `None` when nothing in it resolves.
pub(super) fn resolve_tree<R: LeafResolver>(
    resolver: &mut R,
    state: &PanelState,
) -> Option<Plan<R::Leaf>> {
    match &state.info {
        PanelInfo::Stack { sizes, .. } => {
            let axis = state.info.axis()?;
            let mut children = Vec::new();
            let mut kept_sizes: Vec<Option<Pixels>> = Vec::new();
            let mut dropped = false;
            for (ix, child) in state.children.iter().enumerate() {
                match resolve_tree(resolver, child) {
                    Some(plan) => {
                        children.push(plan);
                        kept_sizes.push(sizes.get(ix).copied());
                    }
                    None => dropped = true,
                }
            }
            match children.len() {
                0 => None,
                1 => children.pop(),
                n => Some(Plan::Split {
                    axis,
                    // Saved sizes no longer add up once a side is gone.
                    sizes: if dropped { vec![None; n] } else { kept_sizes },
                    children,
                }),
            }
        }
        PanelInfo::Tabs { active_index } => {
            let mut panels = Vec::new();
            let mut active = None;
            for (ix, child) in state.children.iter().enumerate() {
                if let Some(panel) = resolver.resolve_leaf(child) {
                    if ix == *active_index {
                        active = Some(panels.len());
                    }
                    panels.push(panel);
                }
            }
            if panels.is_empty() {
                return None;
            }
            Some(Plan::Tabs {
                panels,
                active: active.unwrap_or(0),
            })
        }
        PanelInfo::Tiles { metas } => {
            // A group that cannot be rebuilt takes its bounds with it.
            let mut kept_metas = Vec::new();
            let mut children = Vec::new();
            for (ix, child) in state.children.iter().enumerate() {
                let Some(meta) = metas.get(ix).copied() else {
                    continue;
                };
                if let Some(plan @ Plan::Tabs { .. }) = resolve_tree(resolver, child) {
                    children.push(plan);
                    kept_metas.push(meta);
                }
            }
            if children.is_empty() {
                return None;
            }
            Some(Plan::Tiles {
                metas: kept_metas,
                children,
            })
        }
        // A bare panel outside any tab group is not part of this app's layouts.
        PanelInfo::Panel(_) => None,
    }
}

// ── Live snapshot and GPUI resolver ──────────────────────────────────────────

/// A live panel available for reuse, and where it lives now.
#[derive(Clone)]
pub(super) struct LivePanel {
    pub panel: Arc<dyn PanelView>,
    pub placement: DockPlacement,
}

impl LivePanel {
    fn id(&self) -> EntityId {
        self.panel.view().entity_id()
    }
}

/// Read-only picture of the live dock, taken before anything is resolved.
pub(super) struct LiveSnapshot {
    /// Everything the app created itself, with where it sits.
    pub panels: Vec<LivePanel>,
    /// Entity ids of the panels in the anchor tab group (the level editor's).
    pub anchor_panels: HashSet<EntityId>,
}

impl LiveSnapshot {
    pub fn capture(dock_area: &Entity<DockArea>, anchor: &Entity<TabPanel>, cx: &App) -> Self {
        let panels = dock_area
            .read(cx)
            .tab_panels(cx)
            .into_iter()
            .flat_map(|(placement, tabs)| {
                tabs.read(cx)
                    .all_panels()
                    .into_iter()
                    .map(move |panel| LivePanel { panel, placement })
            })
            .collect();
        let anchor_panels = anchor
            .read(cx)
            .all_panels()
            .iter()
            .map(|p| p.view().entity_id())
            .collect();
        Self {
            panels,
            anchor_panels,
        }
    }
}

/// Which live panels have been claimed by a plan so far.
#[derive(Default)]
pub(super) struct Claims(HashSet<EntityId>);

impl Claims {
    fn contains(&self, id: EntityId) -> bool {
        self.0.contains(&id)
    }
}

/// The GPUI [`LeafResolver`]: claims live panels, creates file editors.
pub(super) struct Resolver<'a> {
    project_root: PathBuf,
    snapshot: &'a LiveSnapshot,
    claims: &'a mut Claims,
    window: &'a mut Window,
    cx: &'a mut App,
}

impl LeafResolver for Resolver<'_> {
    type Leaf = Arc<dyn PanelView>;

    /// The panel a saved tab stands for. Order: a live singleton with the same
    /// `panel_name` that nobody has claimed, then a file editor from the plugin
    /// manager. (Restorer-by-kind goes between the two.)
    fn resolve_leaf(&mut self, leaf: &PanelState) -> Option<Self::Leaf> {
        if let Some(live) = self.snapshot.panels.iter().find(|live| {
            !self.claims.contains(live.id()) && live.panel.panel_name(self.cx) == leaf.panel_name
        }) {
            self.claims.0.insert(live.id());
            return Some(live.panel.clone());
        }

        let path = PathBuf::from(leaf.file()?);
        if !path.exists() {
            tracing::info!("layout: skipping {} (no longer exists)", path.display());
            return None;
        }
        let manager = plugin_manager::global()?;
        let mut manager = manager.write();
        manager.set_project_root(Some(self.project_root.clone()));
        crate::app::refresh_plugin_editor_settings(&mut manager);
        match manager.create_editor_for_file(&path, self.window, self.cx) {
            Ok(panel) => Some(panel),
            Err(error) => {
                tracing::warn!("layout: could not reopen {}: {error}", path.display());
                None
            }
        }
    }
}

// ── Whole-window resolution and validation ───────────────────────────────────

/// A resolved side dock.
pub(super) struct DockPlan {
    pub placement: DockPlacement,
    pub plan: Plan<Arc<dyn PanelView>>,
    pub size: Pixels,
    pub open: bool,
}

/// A validated, fully resolved window layout, ready to commit.
pub(super) struct ResolvedLayout {
    /// `None` when the saved centre was unusable: the live centre stays.
    pub center: Option<Plan<Arc<dyn PanelView>>>,
    pub docks: Vec<DockPlan>,
    /// Live side-dock panels no plan mentions; they go back where they were.
    pub unmentioned: Vec<LivePanel>,
}

/// Validation bookkeeping shared by the roots of one window.
struct ResolveState {
    claims: Claims,
    /// Panels already present in an admitted plan.
    seen: HashSet<EntityId>,
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
            self.claims.0.remove(&panel.view().entity_id());
        }
    }
}

/// Resolve every root of `saved` against the live `snapshot`, without touching
/// the live UI.
pub(super) fn resolve_layout(
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
    };
    let resolve =
        |state: &mut ResolveState, tree: &PanelState, window: &mut Window, cx: &mut App| {
            let mut resolver = Resolver {
                project_root: project_root.to_path_buf(),
                snapshot,
                claims: &mut state.claims,
                window,
                cx,
            };
            resolve_tree(&mut resolver, tree)
        };

    // Centre. It must hold a panel of the anchor group (the level editor
    // cannot be closed, so a centre without it is corrupt): leave the live
    // centre as it is rather than orphan the entity everything holds.
    let center = resolve(&mut state, &saved.dock.center, window, cx).and_then(|plan| {
        let holds_anchor = plan
            .leaves()
            .iter()
            .any(|p| snapshot.anchor_panels.contains(&p.view().entity_id()));
        if holds_anchor && state.admit(&plan) {
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
        (DockPlacement::Left, saved.dock.left_dock.as_ref()),
        (DockPlacement::Right, saved.dock.right_dock.as_ref()),
        (DockPlacement::Bottom, saved.dock.bottom_dock.as_ref()),
    ];
    for (placement, saved_dock) in saved_docks {
        let Some(saved_dock) = saved_dock else {
            continue;
        };
        if !dock_area.read(cx).has_dock(placement) {
            continue;
        }
        let Some(plan) = resolve(&mut state, saved_dock.panel(), window, cx) else {
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
    }
}

#[cfg(test)]
mod tests {
    use super::super::format::tests::tab_group;
    use super::*;

    /// Resolves a leaf to its file name; files starting with `gone` are missing.
    struct Fake;

    impl LeafResolver for Fake {
        type Leaf = String;
        fn resolve_leaf(&mut self, leaf: &PanelState) -> Option<String> {
            let file = leaf.file()?;
            (!file.starts_with("gone")).then(|| file.to_string())
        }
    }

    fn split(axis: Axis, sizes: &[f32], children: Vec<PanelState>) -> PanelState {
        PanelState {
            panel_name: "StackPanel".into(),
            children,
            info: PanelInfo::stack(sizes.iter().map(|s| gpui::px(*s)).collect(), axis),
        }
    }

    fn tabs(plan: &Plan<String>) -> (&[String], usize) {
        match plan {
            Plan::Tabs { panels, active } => (panels, *active),
            other => panic!("expected tabs, got {other:?}"),
        }
    }

    #[test]
    fn missing_leaves_are_pruned_and_active_follows_its_tab() {
        let mut group = tab_group(&["a.rs", "gone.rs", "c.rs"]);
        group.info = PanelInfo::tabs(2);
        let plan = resolve_tree(&mut Fake, &group).unwrap();
        assert_eq!(
            tabs(&plan),
            (&["a.rs".to_string(), "c.rs".to_string()][..], 1)
        );

        // The active tab itself is gone: fall back to the first.
        group.info = PanelInfo::tabs(1);
        let plan = resolve_tree(&mut Fake, &group).unwrap();
        assert_eq!(tabs(&plan).1, 0);
    }

    #[test]
    fn an_emptied_group_is_dropped() {
        assert!(resolve_tree(&mut Fake, &tab_group(&["gone.rs"])).is_none());
        assert!(resolve_tree(&mut Fake, &tab_group(&[])).is_none());
    }

    #[test]
    fn a_split_keeps_its_sizes_when_nothing_is_dropped() {
        let tree = split(
            Axis::Horizontal,
            &[300., 500.],
            vec![tab_group(&["a.rs"]), tab_group(&["b.rs"])],
        );
        match resolve_tree(&mut Fake, &tree).unwrap() {
            Plan::Split {
                axis,
                sizes,
                children,
            } => {
                assert_eq!(axis, Axis::Horizontal);
                assert_eq!(sizes, vec![Some(gpui::px(300.)), Some(gpui::px(500.))]);
                assert_eq!(children.len(), 2);
            }
            other => panic!("expected split, got {other:?}"),
        }
    }

    #[test]
    fn a_split_left_with_one_child_collapses_into_it() {
        let tree = split(
            Axis::Vertical,
            &[100., 100.],
            vec![tab_group(&["gone.rs"]), tab_group(&["b.rs"])],
        );
        let plan = resolve_tree(&mut Fake, &tree).unwrap();
        assert_eq!(tabs(&plan).0, &["b.rs".to_string()]);
    }

    #[test]
    fn sizes_are_dropped_when_a_side_is_dropped() {
        let tree = split(
            Axis::Horizontal,
            &[100., 200., 300.],
            vec![
                tab_group(&["a.rs"]),
                tab_group(&["gone.rs"]),
                tab_group(&["c.rs"]),
            ],
        );
        match resolve_tree(&mut Fake, &tree).unwrap() {
            Plan::Split {
                sizes, children, ..
            } => {
                assert_eq!(children.len(), 2);
                assert_eq!(sizes, vec![None, None]);
            }
            other => panic!("expected split, got {other:?}"),
        }
    }

    #[test]
    fn nested_splits_collapse_through_empty_branches() {
        let inner = split(
            Axis::Vertical,
            &[1., 1.],
            vec![tab_group(&["gone.rs"]), tab_group(&["gone2.rs"])],
        );
        let tree = split(
            Axis::Horizontal,
            &[1., 1.],
            vec![inner, tab_group(&["z.rs"])],
        );
        let plan = resolve_tree(&mut Fake, &tree).unwrap();
        assert_eq!(tabs(&plan).0, &["z.rs".to_string()]);
    }

    #[test]
    fn tiles_drop_unresolvable_groups_with_their_bounds() {
        let metas: Vec<TileMeta> = (0..3)
            .map(|z| TileMeta {
                z_index: z,
                ..Default::default()
            })
            .collect();
        let tree = PanelState {
            panel_name: "Tiles".into(),
            children: vec![
                tab_group(&["a.rs"]),
                tab_group(&["gone.rs"]),
                tab_group(&["c.rs"]),
            ],
            info: PanelInfo::tiles(metas.clone()),
        };
        match resolve_tree(&mut Fake, &tree).unwrap() {
            Plan::Tiles {
                metas: kept,
                children,
            } => {
                assert_eq!(children.len(), 2);
                assert_eq!(
                    kept.iter().map(|m| m.z_index).collect::<Vec<_>>(),
                    vec![0, 2]
                );
            }
            other => panic!("expected tiles, got {other:?}"),
        }
    }

    #[test]
    fn a_bare_panel_is_not_a_layout_root() {
        let leaf = PanelState::default().with_file("a.rs");
        assert!(resolve_tree(&mut Fake, &leaf).is_none());
    }

    #[test]
    fn leaves_lists_every_panel_in_tree_order() {
        let tree = split(
            Axis::Horizontal,
            &[1., 1.],
            vec![tab_group(&["a.rs", "b.rs"]), tab_group(&["c.rs"])],
        );
        let plan = resolve_tree(&mut Fake, &tree).unwrap();
        let leaves: Vec<&str> = plan.leaves().into_iter().map(String::as_str).collect();
        assert_eq!(leaves, ["a.rs", "b.rs", "c.rs"]);
    }
}
