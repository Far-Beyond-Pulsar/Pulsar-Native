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
//!   (nothing is removed from the live UI), rebuilds restorable panels through
//!   the restorer registry (`plugin_manager::panel_restorers`), or creates file
//!   editors through the plugin manager. A panel created here is not attached
//!   anywhere until [`commit`](super::commit) runs.
//!
//! ## Resolution order for a saved tab
//!
//! 1. `ephemeral` tabs are skipped (debug log): a decision the panel made.
//! 2. A live singleton with the same `panel_name` (agent chat, level editor...).
//! 3. A restorer registered for the tab's restore `kind`.
//! 4. A file editor via `create_editor_for_file`.
//!
//! A tab that is identified (restore key or file) but cannot be built because
//! nothing is registered (unknown kind, no editor for the file type) is
//! *dormant*: [`LeafResolver::shelve`] keeps its original `PanelState` so save
//! writes it back (see [`super::dormant`]). A tab that is gone (file deleted,
//! restorer declined, no identity) is dropped.
//!
//! [`resolve_layout`] resolves every root of a saved window and validates the
//! result: the centre must hold a panel of the anchor group (the level editor),
//! and no live panel may be claimed twice. A root that fails validation is
//! discarded whole and its claims are released, so the panels it took are
//! available to the other roots and to the "unmentioned live panel" pass.
//!
//! ## Extension points
//!
//! - [`resolve_in_order`]: the resolution order (live singleton, restorer,
//!   file) and the one place a new way of naming a panel would be added.
//! - [`ResolvedLayout`]: gains tile / pop-out roots next to `center` and `docks`;
//!   it already carries the zoom target.
//! - [`Plan::leaves`] / [`admit`](ResolveState::admit): the validation hooks a
//!   zoom-target check would use.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Axis, Entity, EntityId, Pixels, Window};
use plugin_manager::{PluginManagerError, RestoreOutcome};
use ui::dock::{
    DockArea, DockPlacement, PanelInfo, PanelState, PanelView, TabPanel, TileMeta, ZoomedState,
};

use super::dormant::{AnchorKey, DormantLeaf, DormantRoot, PendingDormant};
use super::format::SavedWindowLayout;
use super::window_geometry;

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

/// Why a saved leaf produced no panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Unresolved {
    /// The panel is gone (file deleted, restorer declined, no identity). The
    /// tab is dropped.
    Gone,
    /// The panel is identified but nothing is registered to build it (plugin
    /// disabled or missing). The tab is kept dormant.
    Dormant,
}

/// Resolves one saved leaf (a tab) to a panel, or says why it cannot be.
/// Implementations must not attach anything to the live UI.
pub(super) trait LeafResolver {
    type Leaf;
    fn resolve_leaf(&mut self, leaf: &PanelState) -> Result<Self::Leaf, Unresolved>;
    /// Keep a leaf that resolved to [`Unresolved::Dormant`] for the next save.
    fn shelve(&mut self, dormant: PendingDormant);
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
            // A sibling that resolved, to anchor shelved tabs to this group.
            let mut anchor: Option<AnchorKey> = None;
            let mut shelved: Vec<PanelState> = Vec::new();
            for (ix, child) in state.children.iter().enumerate() {
                // Ephemeral tabs are deliberately not restored: a decision the
                // panel made, not a loss.
                if child.is_ephemeral() {
                    tracing::debug!("layout: skipping ephemeral tab {}", child.panel_name);
                    continue;
                }
                match resolver.resolve_leaf(child) {
                    Ok(panel) => {
                        if ix == *active_index {
                            active = Some(panels.len());
                        }
                        anchor.get_or_insert_with(|| AnchorKey::of(child));
                        panels.push(panel);
                    }
                    Err(Unresolved::Dormant) => shelved.push(child.clone()),
                    Err(Unresolved::Gone) => {}
                }
            }
            for state in shelved {
                resolver.shelve(PendingDormant {
                    anchor: anchor.clone(),
                    state,
                });
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

// ── Resolution order ─────────────────────────────────────────────────────────

/// Outcome of asking the restorer registry about a tab.
pub(super) enum RestoreStep<L> {
    Restored(L),
    /// A restorer exists for the kind but could not rebuild this tab.
    Declined,
    /// No restorer is registered for the kind.
    Unregistered,
}

/// Outcome of opening a tab's file.
pub(super) enum FileStep<L> {
    Opened(L),
    /// The file no longer exists.
    Gone,
    /// The file exists but nothing is registered to edit it (plugin missing).
    NoEditor,
    /// An editor exists and failed.
    Failed,
}

/// The three ways a tab can be named, as the resolver can ask for them. Kept
/// apart from [`resolve_in_order`] so the order is testable without GPUI.
pub(super) trait LeafSources {
    type Leaf;
    /// A live singleton with this `panel_name` that nobody has claimed.
    fn singleton(&mut self, panel_name: &str) -> Option<Self::Leaf>;
    fn restore(&mut self, kind: &str, data: &serde_json::Value) -> RestoreStep<Self::Leaf>;
    fn open_file(&mut self, file: &str) -> FileStep<Self::Leaf>;
}

/// The panel a saved tab stands for, in order:
///
/// 1. a live singleton with the same `panel_name`;
/// 2. a restorer registered for the tab's restore `kind`;
/// 3. a file editor.
///
/// A tab that is identified (restore key or file) but whose kind has no
/// restorer, or whose file type has no editor, is [`Unresolved::Dormant`]; a
/// missing file, a restorer that declines or a failing editor is
/// [`Unresolved::Gone`]. A restorer that declines is final: the file, if the
/// tab also recorded one, is not a second opinion.
pub(super) fn resolve_in_order<S: LeafSources>(
    sources: &mut S,
    leaf: &PanelState,
) -> Result<S::Leaf, Unresolved> {
    if let Some(panel) = sources.singleton(&leaf.panel_name) {
        return Ok(panel);
    }

    let mut unregistered = false;
    if let Some((kind, data)) = leaf.restore_key() {
        match sources.restore(kind, data) {
            RestoreStep::Restored(panel) => return Ok(panel),
            RestoreStep::Declined => return Err(Unresolved::Gone),
            RestoreStep::Unregistered => unregistered = true,
        }
    }

    let Some(file) = leaf.file() else {
        return Err(if unregistered {
            Unresolved::Dormant
        } else {
            Unresolved::Gone
        });
    };
    match sources.open_file(file) {
        FileStep::Opened(panel) => Ok(panel),
        FileStep::NoEditor => Err(Unresolved::Dormant),
        FileStep::Gone => Err(Unresolved::Gone),
        FileStep::Failed if unregistered => Err(Unresolved::Dormant),
        FileStep::Failed => Err(Unresolved::Gone),
    }
}

// ── GPUI resolver ────────────────────────────────────────────────────────────

/// The GPUI [`LeafResolver`]: claims live panels, rebuilds restorable panels
/// through the restorer registry, creates file editors.
pub(super) struct Resolver<'a> {
    project_root: PathBuf,
    /// Whether live panels may be claimed by name. Pop-outs never claim them:
    /// a live panel sits in the main window's dock and is placed by that root.
    claim_live: bool,
    snapshot: &'a LiveSnapshot,
    claims: &'a mut Claims,
    dormant: &'a mut Vec<PendingDormant>,
    window: &'a mut Window,
    cx: &'a mut App,
}

impl LeafSources for Resolver<'_> {
    type Leaf = Arc<dyn PanelView>;

    fn singleton(&mut self, panel_name: &str) -> Option<Self::Leaf> {
        if !self.claim_live {
            return None;
        }
        let live = self.snapshot.panels.iter().find(|live| {
            !self.claims.contains(live.id()) && live.panel.panel_name(self.cx) == panel_name
        })?;
        self.claims.0.insert(live.id());
        Some(live.panel.clone())
    }

    fn restore(&mut self, kind: &str, data: &serde_json::Value) -> RestoreStep<Self::Leaf> {
        match plugin_manager::panel_restorers().restore(kind, data, self.window, self.cx) {
            RestoreOutcome::Restored(panel) => RestoreStep::Restored(panel),
            RestoreOutcome::Declined => {
                tracing::info!("layout: restorer '{kind}' could not rebuild a tab");
                RestoreStep::Declined
            }
            RestoreOutcome::UnknownKind => {
                tracing::info!("layout: no restorer for '{kind}'; keeping its tab dormant");
                RestoreStep::Unregistered
            }
        }
    }

    fn open_file(&mut self, file: &str) -> FileStep<Self::Leaf> {
        let path = PathBuf::from(file);
        if !path.exists() {
            tracing::info!("layout: skipping {} (no longer exists)", path.display());
            return FileStep::Gone;
        }
        let Some(manager) = plugin_manager::global() else {
            return FileStep::Failed;
        };
        let mut manager = manager.write();
        manager.set_project_root(Some(self.project_root.clone()));
        crate::app::refresh_plugin_editor_settings(&mut manager);
        match manager.create_editor_for_file(&path, self.window, self.cx) {
            Ok(panel) => FileStep::Opened(panel),
            Err(error) if no_editor_registered(&error) => {
                tracing::info!(
                    "layout: no editor for {} ({error}); keeping its tab dormant",
                    path.display()
                );
                FileStep::NoEditor
            }
            Err(error) => {
                tracing::warn!("layout: could not reopen {}: {error}", path.display());
                FileStep::Failed
            }
        }
    }
}

impl LeafResolver for Resolver<'_> {
    type Leaf = Arc<dyn PanelView>;

    fn resolve_leaf(&mut self, leaf: &PanelState) -> Result<Self::Leaf, Unresolved> {
        resolve_in_order(self, leaf)
    }

    fn shelve(&mut self, dormant: PendingDormant) {
        self.dormant.push(dormant);
    }
}

/// Whether opening a file failed because nothing is registered for it (its
/// plugin is not loaded), as opposed to an editor that exists and failed.
fn no_editor_registered(error: &PluginManagerError) -> bool {
    matches!(
        error,
        PluginManagerError::NoFileTypeForPath { .. }
            | PluginManagerError::NoEditorForFileType { .. }
            | PluginManagerError::EditorNotFound { .. }
            | PluginManagerError::PluginNotFound { .. }
    )
}

// ── Whole-window resolution and validation ───────────────────────────────────

/// A resolved side dock.
pub(super) struct DockPlan {
    pub placement: DockPlacement,
    pub plan: Plan<Arc<dyn PanelView>>,
    pub size: Pixels,
    pub open: bool,
}

/// A resolved pop-out window, ready to open.
pub(super) struct PopoutPlan {
    /// Validated against the connected displays.
    pub bounds: gpui::WindowBounds,
    /// The windowed size a maximized pop-out returns to.
    pub restore_bounds: Option<gpui::Bounds<Pixels>>,
    pub plan: Plan<Arc<dyn PanelView>>,
}

/// A validated, fully resolved window layout, ready to commit.
pub(super) struct ResolvedLayout {
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
}

/// Validation bookkeeping shared by the roots of one window.
struct ResolveState {
    claims: Claims,
    /// Panels already present in an admitted plan.
    seen: HashSet<EntityId>,
    /// Tabs shelved by the resolver, across all roots.
    dormant: Vec<DormantLeaf>,
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
        dormant: Vec::new(),
    };
    let resolve = |state: &mut ResolveState,
                   root: Option<DormantRoot>,
                   tree: &PanelState,
                   window: &mut Window,
                   cx: &mut App| {
        let mut pending = Vec::new();
        let plan = {
            let mut resolver = Resolver {
                project_root: project_root.to_path_buf(),
                claim_live: root.is_some(),
                snapshot,
                claims: &mut state.claims,
                dormant: &mut pending,
                window,
                cx,
            };
            resolve_tree(&mut resolver, tree)
        };
        match root {
            Some(root) => state
                .dormant
                .extend(pending.into_iter().map(|p| p.into_leaf(root))),
            // A tab no plugin can build in a pop-out is not kept (the dormant
            // list has no pop-out root; see the module docs).
            None => {
                for p in pending {
                    tracing::info!(
                        "layout: a pop-out tab cannot be rebuilt and is dropped: {}",
                        p.state.panel_name
                    );
                }
            }
        }
        plan
    };

    // Centre. It must hold a panel of the anchor group (the level editor
    // cannot be closed, so a centre without it is corrupt): leave the live
    // centre as it is rather than orphan the entity everything holds.
    let center = resolve(
        &mut state,
        Some(DormantRoot::Center),
        &saved.dock.center,
        window,
        cx,
    )
    .and_then(|plan| {
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
        let Some(plan) = resolve(&mut state, Some(root), saved_dock.panel(), window, cx) else {
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
    for saved_popout in &saved.popouts {
        let Some(plan) = resolve(&mut state, None, &saved_popout.dock, window, cx) else {
            tracing::info!("layout: a saved pop-out resolves to nothing; dropped");
            continue;
        };
        if !state.admit(&plan) {
            state.release(&plan);
            continue;
        }
        popouts.push(PopoutPlan {
            bounds: window_geometry::fit_to_connected_displays(saved_popout.bounds, cx),
            restore_bounds: Some(saved_popout.bounds.bounds),
            plan,
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
    }
}

#[cfg(test)]
mod tests {
    use super::super::format::tests::tab_group;
    use super::*;

    /// Resolves a leaf to its file name; files starting with `gone` are missing.
    #[derive(Default)]
    struct Fake {
        shelved: Vec<PendingDormant>,
    }

    impl LeafResolver for Fake {
        type Leaf = String;
        fn resolve_leaf(&mut self, leaf: &PanelState) -> Result<String, Unresolved> {
            if let Some((kind, _)) = leaf.restore_key() {
                return if kind.starts_with("unregistered") {
                    Err(Unresolved::Dormant)
                } else if kind.starts_with("declined") {
                    Err(Unresolved::Gone)
                } else {
                    Ok(format!("restored:{kind}"))
                };
            }
            let file = leaf.file().ok_or(Unresolved::Gone)?;
            if file.starts_with("noeditor") {
                Err(Unresolved::Dormant)
            } else if file.starts_with("gone") {
                Err(Unresolved::Gone)
            } else {
                Ok(file.to_string())
            }
        }

        fn shelve(&mut self, dormant: PendingDormant) {
            self.shelved.push(dormant);
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
        let plan = resolve_tree(&mut Fake::default(), &group).unwrap();
        assert_eq!(
            tabs(&plan),
            (&["a.rs".to_string(), "c.rs".to_string()][..], 1)
        );

        // The active tab itself is gone: fall back to the first.
        group.info = PanelInfo::tabs(1);
        let plan = resolve_tree(&mut Fake::default(), &group).unwrap();
        assert_eq!(tabs(&plan).1, 0);
    }

    #[test]
    fn an_emptied_group_is_dropped() {
        assert!(resolve_tree(&mut Fake::default(), &tab_group(&["gone.rs"])).is_none());
        assert!(resolve_tree(&mut Fake::default(), &tab_group(&[])).is_none());
    }

    #[test]
    fn a_split_keeps_its_sizes_when_nothing_is_dropped() {
        let tree = split(
            Axis::Horizontal,
            &[300., 500.],
            vec![tab_group(&["a.rs"]), tab_group(&["b.rs"])],
        );
        match resolve_tree(&mut Fake::default(), &tree).unwrap() {
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
        let plan = resolve_tree(&mut Fake::default(), &tree).unwrap();
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
        match resolve_tree(&mut Fake::default(), &tree).unwrap() {
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
        let plan = resolve_tree(&mut Fake::default(), &tree).unwrap();
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
        match resolve_tree(&mut Fake::default(), &tree).unwrap() {
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
        assert!(resolve_tree(&mut Fake::default(), &leaf).is_none());
    }

    #[test]
    fn leaves_lists_every_panel_in_tree_order() {
        let tree = split(
            Axis::Horizontal,
            &[1., 1.],
            vec![tab_group(&["a.rs", "b.rs"]), tab_group(&["c.rs"])],
        );
        let plan = resolve_tree(&mut Fake::default(), &tree).unwrap();
        let leaves: Vec<&str> = plan.leaves().into_iter().map(String::as_str).collect();
        assert_eq!(leaves, ["a.rs", "b.rs", "c.rs"]);
    }

    // ── Resolution order and dormant tabs ───────────────────────────────────

    /// Sources that log which lookups were made, with scripted answers.
    #[derive(Default)]
    struct Script {
        singletons: Vec<&'static str>,
        known_kinds: Vec<&'static str>,
        declining_kinds: Vec<&'static str>,
        editable_files: Vec<&'static str>,
        failing_files: Vec<&'static str>,
        calls: Vec<String>,
    }

    impl LeafSources for Script {
        type Leaf = String;

        fn singleton(&mut self, name: &str) -> Option<String> {
            self.calls.push(format!("singleton:{name}"));
            self.singletons
                .contains(&name)
                .then(|| format!("singleton:{name}"))
        }

        fn restore(&mut self, kind: &str, _: &serde_json::Value) -> RestoreStep<String> {
            self.calls.push(format!("restore:{kind}"));
            if self.declining_kinds.contains(&kind) {
                RestoreStep::Declined
            } else if self.known_kinds.contains(&kind) {
                RestoreStep::Restored(format!("restored:{kind}"))
            } else {
                RestoreStep::Unregistered
            }
        }

        fn open_file(&mut self, file: &str) -> FileStep<String> {
            self.calls.push(format!("file:{file}"));
            if file.starts_with("gone") {
                FileStep::Gone
            } else if self.failing_files.contains(&file) {
                FileStep::Failed
            } else if self.editable_files.contains(&file) {
                FileStep::Opened(format!("file:{file}"))
            } else {
                FileStep::NoEditor
            }
        }
    }

    fn named(name: &str) -> PanelState {
        PanelState {
            panel_name: name.into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_live_singleton_wins_over_a_restorer_and_a_file() {
        let mut s = Script {
            singletons: vec!["Panel"],
            known_kinds: vec!["k"],
            editable_files: vec!["a.rs"],
            ..Default::default()
        };
        let leaf = named("Panel")
            .with_file("a.rs")
            .with_restore("k", serde_json::Value::Null);
        assert_eq!(
            resolve_in_order(&mut s, &leaf),
            Ok("singleton:Panel".into())
        );
        assert_eq!(s.calls, ["singleton:Panel"]);
    }

    #[test]
    fn a_restorer_wins_over_the_file() {
        let mut s = Script {
            known_kinds: vec!["k"],
            editable_files: vec!["a.rs"],
            ..Default::default()
        };
        let leaf = named("P")
            .with_file("a.rs")
            .with_restore("k", serde_json::Value::Null);
        assert_eq!(resolve_in_order(&mut s, &leaf), Ok("restored:k".into()));
        assert_eq!(s.calls, ["singleton:P", "restore:k"]);
    }

    #[test]
    fn a_file_is_the_last_resort() {
        let mut s = Script {
            editable_files: vec!["a.rs"],
            ..Default::default()
        };
        assert_eq!(
            resolve_in_order(&mut s, &named("P").with_file("a.rs")),
            Ok("file:a.rs".into())
        );
        assert_eq!(s.calls, ["singleton:P", "file:a.rs"]);
    }

    #[test]
    fn an_unregistered_kind_falls_through_to_the_file_then_goes_dormant() {
        let leaf = named("P")
            .with_file("a.rs")
            .with_restore("later", serde_json::Value::Null);
        // The file still opens: no dormancy needed.
        let mut s = Script {
            editable_files: vec!["a.rs"],
            ..Default::default()
        };
        assert_eq!(resolve_in_order(&mut s, &leaf), Ok("file:a.rs".into()));
        // The file has no editor either: dormant.
        let mut s = Script::default();
        assert_eq!(resolve_in_order(&mut s, &leaf), Err(Unresolved::Dormant));
        // An editor that exists but fails, behind an unknown kind: still dormant.
        let mut s = Script {
            failing_files: vec!["a.rs"],
            ..Default::default()
        };
        assert_eq!(resolve_in_order(&mut s, &leaf), Err(Unresolved::Dormant));
    }

    #[test]
    fn identified_but_unregistered_is_dormant_and_missing_is_gone() {
        let kind_only = named("P").with_restore("later", serde_json::Value::Null);
        assert_eq!(
            resolve_in_order(&mut Script::default(), &kind_only),
            Err(Unresolved::Dormant)
        );
        // No editor for the file type: dormant.
        assert_eq!(
            resolve_in_order(&mut Script::default(), &named("P").with_file("a.xyz")),
            Err(Unresolved::Dormant)
        );
        // The file is deleted: gone, not dormant.
        assert_eq!(
            resolve_in_order(&mut Script::default(), &named("P").with_file("gone.rs")),
            Err(Unresolved::Gone)
        );
        // An editor that exists and fails: gone.
        let mut s = Script {
            failing_files: vec!["a.rs"],
            ..Default::default()
        };
        assert_eq!(
            resolve_in_order(&mut s, &named("P").with_file("a.rs")),
            Err(Unresolved::Gone)
        );
        // No identity at all: gone.
        assert_eq!(
            resolve_in_order(&mut Script::default(), &named("P")),
            Err(Unresolved::Gone)
        );
    }

    #[test]
    fn a_declining_restorer_is_final() {
        let mut s = Script {
            declining_kinds: vec!["k"],
            editable_files: vec!["a.rs"],
            ..Default::default()
        };
        let leaf = named("P")
            .with_file("a.rs")
            .with_restore("k", serde_json::Value::Null);
        assert_eq!(resolve_in_order(&mut s, &leaf), Err(Unresolved::Gone));
        assert_eq!(s.calls, ["singleton:P", "restore:k"]);
    }

    fn restore_tab(kind: &str) -> PanelState {
        named("P").with_restore(kind, serde_json::Value::Null)
    }

    fn group(children: Vec<PanelState>, active: usize) -> PanelState {
        PanelState {
            panel_name: "TabPanel".into(),
            children,
            info: PanelInfo::tabs(active),
        }
    }

    #[test]
    fn ephemeral_tabs_are_skipped_without_asking_the_resolver() {
        let mut fake = Fake::default();
        let tree = group(
            vec![
                named("P").with_file("a.rs"),
                named("P").with_file("b.rs").ephemeral(),
            ],
            0,
        );
        let plan = resolve_tree(&mut fake, &tree).unwrap();
        assert_eq!(tabs(&plan).0, &["a.rs".to_string()]);
        assert!(fake.shelved.is_empty());
    }

    #[test]
    fn dormant_tabs_are_shelved_with_their_state_and_a_sibling_anchor() {
        let mut fake = Fake::default();
        let dormant = restore_tab("unregistered.thing");
        let tree = group(vec![dormant.clone(), named("P").with_file("b.rs")], 1);
        let plan = resolve_tree(&mut fake, &tree).unwrap();
        // The dormant tab is not in the plan; the active tab still follows b.rs.
        assert_eq!(tabs(&plan), (&["b.rs".to_string()][..], 0));
        assert_eq!(
            fake.shelved,
            vec![PendingDormant {
                anchor: Some(AnchorKey::File("b.rs".into())),
                state: dormant,
            }]
        );
    }

    #[test]
    fn a_group_of_only_dormant_tabs_vanishes_but_keeps_them() {
        let mut fake = Fake::default();
        let tree = group(
            vec![restore_tab("unregistered.a"), restore_tab("unregistered.b")],
            0,
        );
        assert!(resolve_tree(&mut fake, &tree).is_none());
        assert_eq!(fake.shelved.len(), 2);
        assert!(fake.shelved.iter().all(|d| d.anchor.is_none()));
    }

    #[test]
    fn gone_tabs_are_not_shelved() {
        let mut fake = Fake::default();
        let tree = group(
            vec![
                named("P").with_file("gone.rs"),
                restore_tab("declined.x"),
                named("P").with_file("a.rs"),
            ],
            0,
        );
        resolve_tree(&mut fake, &tree).unwrap();
        assert!(fake.shelved.is_empty());
    }

    #[test]
    fn a_shelved_tab_comes_back_when_its_kind_registers() {
        use super::super::dormant::{reinsert, DormantRoot};

        // Session 1: the kind is unknown, the tab is shelved and saved.
        let saved = group(
            vec![named("P").with_file("b.rs"), restore_tab("unregistered.x")],
            0,
        );
        let mut fake = Fake::default();
        resolve_tree(&mut fake, &saved).unwrap();
        let leaves: Vec<_> = fake
            .shelved
            .drain(..)
            .map(|p| p.into_leaf(DormantRoot::Center))
            .collect();
        assert_eq!(leaves.len(), 1);

        // Session 2: the dump has only the resolved tab; the dormant tab is
        // offered again and, with the kind now known, resolves normally.
        let mut dump = group(vec![named("P").with_file("b.rs")], 0);
        assert!(reinsert(&mut dump, &leaves[0]));
        dump.children[1] = restore_tab("known.x");
        let mut fake = Fake::default();
        let plan = resolve_tree(&mut fake, &dump).unwrap();
        assert_eq!(
            tabs(&plan).0,
            &["b.rs".to_string(), "restored:known.x".to_string()]
        );
        assert!(fake.shelved.is_empty(), "no longer dormant");
    }
}
