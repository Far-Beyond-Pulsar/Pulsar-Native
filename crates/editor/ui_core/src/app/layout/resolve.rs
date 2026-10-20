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
//! 2. A tab with an owner marker (an inner panel of another editor) is looked up
//!    among that editor's live panels and claimed ([`super::owned`]).
//! 3. A live singleton with the same `panel_name` (agent chat, level editor...).
//! 4. A restorer registered for the tab's restore `kind`.
//! 5. A file editor via `create_editor_for_file`; never for a tab with an owner.
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

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use gpui::{App, Axis, Entity, EntityId, Pixels, Window};
use plugin_manager::{PluginManagerError, RestoreOutcome};
use ui::dock::{
    DockArea, DockPlacement, PanelInfo, PanelState, PanelView, TabPanel, TileMeta, ZoomedState,
};

use super::dormant::{AnchorKey, DormantLeaf, DormantRoot, PendingDormant};
use super::format::{SavedPopout, SavedWindowLayout};
use super::owned;
use super::window_geometry;

/// The most pop-outs one restore opens: a bound against a damaged file.
const MAX_POPOUTS: usize = 16;

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

    /// Keep only the leaves `keep` accepts, pruning what that empties exactly as
    /// [`resolve_tree`] does: emptied groups vanish, a split left with one child
    /// collapses into it, and saved sizes are discarded once a side is gone.
    pub fn retain_leaves(self, keep: &mut dyn FnMut(&L) -> bool) -> Option<Plan<L>> {
        match self {
            Plan::Split {
                axis,
                sizes,
                children,
            } => {
                let total = children.len();
                let mut kept = Vec::new();
                let mut kept_sizes = Vec::new();
                for (ix, child) in children.into_iter().enumerate() {
                    if let Some(child) = child.retain_leaves(keep) {
                        kept.push(child);
                        kept_sizes.push(sizes.get(ix).copied().flatten());
                    }
                }
                match kept.len() {
                    0 => None,
                    1 => kept.pop(),
                    n => Some(Plan::Split {
                        axis,
                        sizes: if n < total { vec![None; n] } else { kept_sizes },
                        children: kept,
                    }),
                }
            }
            Plan::Tabs { panels, active } => {
                let mut kept = Vec::new();
                let mut new_active = None;
                for (ix, panel) in panels.into_iter().enumerate() {
                    if keep(&panel) {
                        if ix == active {
                            new_active = Some(kept.len());
                        }
                        kept.push(panel);
                    }
                }
                if kept.is_empty() {
                    return None;
                }
                Some(Plan::Tabs {
                    panels: kept,
                    active: new_active.unwrap_or(0),
                })
            }
            Plan::Tiles { metas, children } => {
                let mut kept_metas = Vec::new();
                let mut kept = Vec::new();
                for (meta, child) in metas.into_iter().zip(children) {
                    if let Some(child @ Plan::Tabs { .. }) = child.retain_leaves(keep) {
                        kept_metas.push(meta);
                        kept.push(child);
                    }
                }
                if kept.is_empty() {
                    return None;
                }
                Some(Plan::Tiles {
                    metas: kept_metas,
                    children: kept,
                })
            }
        }
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

    /// Whether a tab that is not the displayed one of its group may be built
    /// later, one per frame, instead of during this resolve. Building an
    /// editor is the cost of a restore, and a hidden tab is not needed to
    /// paint the first frame.
    fn should_defer(&self, _leaf: &PanelState) -> bool {
        false
    }

    /// Record a deferred tab: it goes into the group of `sibling` (a tab of the
    /// same group that did resolve) at saved position `index`.
    fn defer(
        &mut self,
        _sibling: &Self::Leaf,
        _index: usize,
        _anchor: Option<AnchorKey>,
        _state: PanelState,
    ) {
    }
}

/// A saved tab that restore left for later; see [`LeafResolver::should_defer`].
pub(super) struct DeferredTab {
    pub root: DormantRoot,
    /// A resolved tab of the group this one belongs in.
    pub sibling: Arc<dyn PanelView>,
    /// Its position in the saved group.
    pub index: usize,
    pub anchor: Option<AnchorKey>,
    pub state: PanelState,
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
            // Owned panels live inside an editor that may be a sibling tab, so
            // they are resolved after the others; the saved order is kept.
            let mut results: Vec<Option<Result<R::Leaf, Unresolved>>> =
                state.children.iter().map(|_| None).collect();
            let owned_last = state
                .children
                .iter()
                .enumerate()
                .filter(|(_, c)| c.owner().is_none())
                .chain(
                    state
                        .children
                        .iter()
                        .enumerate()
                        .filter(|(_, c)| c.owner().is_some()),
                );
            let mut deferred_ix: Vec<usize> = Vec::new();
            for (ix, child) in owned_last {
                // Ephemeral tabs are deliberately not restored: a decision the
                // panel made, not a loss.
                if child.is_ephemeral() {
                    tracing::debug!("layout: skipping ephemeral tab {}", child.panel_name);
                    continue;
                }
                if ix != *active_index && resolver.should_defer(child) {
                    deferred_ix.push(ix);
                    continue;
                }
                results[ix] = Some(resolver.resolve_leaf(child));
            }
            // With no resolved tab there is nothing to hang the deferred ones
            // on (the displayed tab failed): build them now.
            if !results.iter().any(|r| matches!(r, Some(Ok(_)))) {
                for ix in deferred_ix.drain(..) {
                    results[ix] = Some(resolver.resolve_leaf(&state.children[ix]));
                }
            }
            for (ix, (child, result)) in state.children.iter().zip(results).enumerate() {
                let Some(result) = result else {
                    continue;
                };
                match result {
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
            if let Some(sibling) = panels.first() {
                for ix in deferred_ix {
                    resolver.defer(sibling, ix, anchor.clone(), state.children[ix].clone());
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

/// Outcome of looking up an owned panel (a panel of another editor's dock).
pub(super) enum OwnedStep<L> {
    Found(L),
    /// The owner's file no longer exists: the panel is gone with it.
    OwnerGone,
    /// The owner is not live (its plugin is missing, or it failed to open), or
    /// it has no such panel: the leaf is kept dormant.
    Missing,
}

/// The ways a tab can be named, as the resolver can ask for them. Kept apart
/// from [`resolve_in_order`] so the order is testable without GPUI.
pub(super) trait LeafSources {
    type Leaf;
    /// A live panel of the editor that owns `owner_file`: the `nth` panel
    /// named `panel_name` among that owner's (see [`owned`]). Claims it.
    fn owned(&mut self, panel_name: &str, owner_file: &str, nth: usize) -> OwnedStep<Self::Leaf>;
    /// A live singleton with this `panel_name` that nobody has claimed.
    fn singleton(&mut self, panel_name: &str) -> Option<Self::Leaf>;
    fn restore(&mut self, kind: &str, data: &serde_json::Value) -> RestoreStep<Self::Leaf>;
    fn open_file(&mut self, file: &str) -> FileStep<Self::Leaf>;
}

/// The panel a saved tab stands for, in order:
///
/// 1. for a leaf with an owner marker, the owner editor's live panel (claimed,
///    never created);
/// 2. a live singleton with the same `panel_name`;
/// 3. a restorer registered for the tab's restore `kind`;
/// 4. a file editor, **never for a leaf with an owner**: opening the file would
///    create a second editor next to the one that owns the panel.
///
/// A leaf with an owner that no step finds is [`Unresolved::Dormant`] (its
/// owner may only be missing this session), or [`Unresolved::Gone`] when the
/// owner's file no longer exists.
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
    let owned = leaf.owner().is_some();
    if let Some((owner, nth)) = leaf.owner() {
        match sources.owned(&leaf.panel_name, owner, nth) {
            OwnedStep::Found(panel) => return Ok(panel),
            OwnedStep::OwnerGone => return Err(Unresolved::Gone),
            OwnedStep::Missing => {}
        }
    }
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

    let file = leaf.file().filter(|_| !owned);
    let Some(file) = file else {
        return Err(if unregistered || owned {
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
    /// Where each claimed owned panel sits now, so commit can take it out.
    homes: &'a mut HashMap<EntityId, Entity<TabPanel>>,
    dormant: &'a mut Vec<PendingDormant>,
    /// Which saved root this resolves, for the tabs it defers.
    root: DormantRoot,
    /// Files some saved tab names as its owner. Such an editor is built now so
    /// its inner panels can be claimed.
    owners: &'a HashSet<String>,
    deferred: &'a mut Vec<DeferredTab>,
    window: &'a mut Window,
    cx: &'a mut App,
}

/// Build one tab that restore deferred. Nothing live is claimed: a deferred tab
/// is never an owned panel or a live singleton.
pub(super) fn build_deferred(
    state: &PanelState,
    project_root: &std::path::Path,
    window: &mut Window,
    cx: &mut App,
) -> Result<Arc<dyn PanelView>, Unresolved> {
    let snapshot = LiveSnapshot {
        panels: Vec::new(),
        anchor_panels: HashSet::new(),
    };
    let (mut claims, mut homes, mut dormant, mut deferred) =
        (Claims::default(), HashMap::new(), Vec::new(), Vec::new());
    let owners = HashSet::new();
    let mut resolver = Resolver {
        project_root: project_root.to_path_buf(),
        claim_live: false,
        snapshot: &snapshot,
        claims: &mut claims,
        homes: &mut homes,
        dormant: &mut dormant,
        root: DormantRoot::Center,
        owners: &owners,
        deferred: &mut deferred,
        window,
        cx,
    };
    resolve_in_order(&mut resolver, state)
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

impl LeafSources for Resolver<'_> {
    type Leaf = Arc<dyn PanelView>;

    fn owned(&mut self, panel_name: &str, owner_file: &str, nth: usize) -> OwnedStep<Self::Leaf> {
        if !owned::owner_exists(owner_file) {
            tracing::info!("layout: the owner {owner_file} of a {panel_name} tab is gone");
            return OwnedStep::OwnerGone;
        }
        // Looked up fresh each time: editors created earlier in this same
        // resolve pass have built their docks by now.
        let live = owned::live_owned(self.cx);
        match owned::find(&live, panel_name, owner_file, nth) {
            Some(found) if !self.claims.contains(found.id()) => {
                self.claims.0.insert(found.id());
                self.homes.insert(found.id(), found.tab_panel.clone());
                OwnedStep::Found(found.panel.clone())
            }
            Some(_) => {
                tracing::debug!("layout: the {panel_name} #{nth} of {owner_file} is already used");
                OwnedStep::Missing
            }
            None => {
                tracing::info!(
                    "layout: no live {panel_name} #{nth} of {owner_file}; keeping its tab dormant"
                );
                OwnedStep::Missing
            }
        }
    }

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
        // No plugin manager means nothing is registered to edit the file; keep
        // the tab for a session that has one rather than dropping it.
        let Some(manager) = plugin_manager::global() else {
            return FileStep::NoEditor;
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
        let result = resolve_in_order(self, leaf);
        let outcome = match &result {
            Ok(panel) => format!("panel {}", panel.panel_name(self.cx)),
            Err(why) => format!("{why:?}"),
        };
        tracing::info!(
            leaf = %leaf.panel_name,
            file = ?leaf.file(),
            owner = ?leaf.owner(),
            claim_live = self.claim_live,
            "layout: resolved a saved tab -> {outcome}"
        );
        result
    }

    fn shelve(&mut self, dormant: PendingDormant) {
        self.dormant.push(dormant);
    }

    fn should_defer(&self, leaf: &PanelState) -> bool {
        // Pop-outs build everything: their window opens at commit.
        self.claim_live
            && leaf.owner().is_none()
            && (leaf.file().is_some() || leaf.restore_key().is_some())
            && !leaf.file().is_some_and(|file| self.owners.contains(file))
            // A live panel of this name would be claimed, not built.
            && !self
                .snapshot
                .panels
                .iter()
                .any(|live| live.panel.panel_name(self.cx) == leaf.panel_name)
    }

    fn defer(
        &mut self,
        sibling: &Self::Leaf,
        index: usize,
        anchor: Option<AnchorKey>,
        state: PanelState,
    ) {
        self.deferred.push(DeferredTab {
            root: self.root,
            sibling: sibling.clone(),
            index,
            anchor,
            state,
        });
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
    /// Owned panels the plans claimed (panels of an editor's own dock) and the
    /// tab group each sits in now. Commit takes them out of it.
    pub owned_homes: HashMap<EntityId, Entity<TabPanel>>,
    /// Saved pop-outs that open nothing because everything they held is
    /// dormant. They stay in the file (empty, with their id and bounds) so the
    /// dormant tabs have a pop-out to come back to.
    pub ghost_popouts: Vec<SavedPopout>,
    /// Hidden tabs not built yet; each joins the group of a tab in this layout.
    pub deferred: Vec<DeferredTab>,
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
    /// Tabs left for after the first frame, across all roots.
    deferred: Vec<DeferredTab>,
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
        homes: HashMap::new(),
        deferred: Vec::new(),
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
                deferred: &mut state.deferred,
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

    // A deferred tab whose group sat in a discarded plan has nowhere to go.
    let seen = &state.seen;
    let deferred = state
        .deferred
        .into_iter()
        .filter(|d| seen.contains(&d.sibling.view().entity_id()))
        .collect();

    ResolvedLayout {
        center,
        docks,
        unmentioned,
        deferred,
        dormant: state.dormant,
        zoomed: saved.dock.zoomed.clone(),
        popouts,
        owned_homes: state.homes,
        ghost_popouts,
    }
}

/// Whether a saved centre satisfies the anchor rule. The centre must hold a
/// panel of the anchor group (the level editor) only when this window has one:
/// a window built without a level editor has an empty anchor group and any
/// centre is acceptable.
fn anchor_requirement_met(anchor_panel_count: usize, centre_holds_anchor: bool) -> bool {
    anchor_panel_count == 0 || centre_holds_anchor
}

#[cfg(test)]
mod tests {
    use super::super::format::tests::tab_group;
    use super::*;

    #[test]
    fn the_anchor_rule_applies_only_to_windows_that_have_an_anchor() {
        assert!(anchor_requirement_met(0, false));
        assert!(anchor_requirement_met(1, true));
        assert!(!anchor_requirement_met(1, false));
    }

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
        /// `(panel_name, owner, nth)` of live owned panels.
        owned_panels: Vec<(&'static str, &'static str, usize)>,
        calls: Vec<String>,
    }

    impl LeafSources for Script {
        type Leaf = String;

        fn owned(&mut self, name: &str, owner: &str, nth: usize) -> OwnedStep<String> {
            self.calls.push(format!("owned:{name}:{owner}:{nth}"));
            if owner.starts_with("gone") {
                OwnedStep::OwnerGone
            } else if self.owned_panels.contains(&(name, owner, nth)) {
                OwnedStep::Found(format!("owned:{name}:{owner}:{nth}"))
            } else {
                OwnedStep::Missing
            }
        }

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
    fn an_owned_leaf_is_found_among_its_owners_panels_first() {
        let mut s = Script {
            owned_panels: vec![("graph-canvas", "/p/A.class", 1)],
            singletons: vec!["graph-canvas"],
            ..Default::default()
        };
        let leaf = named("graph-canvas").with_owner("/p/A.class", 1);
        assert_eq!(
            resolve_in_order(&mut s, &leaf),
            Ok("owned:graph-canvas:/p/A.class:1".into())
        );
        assert_eq!(s.calls, ["owned:graph-canvas:/p/A.class:1"]);
    }

    #[test]
    fn an_owned_leaf_never_falls_through_to_opening_a_file() {
        // The owner is not live, and the leaf also carries a file that has an
        // editor: opening it would create a second editor next to the owner.
        let mut s = Script {
            editable_files: vec!["/p/A.class"],
            ..Default::default()
        };
        let leaf = named("graph-canvas")
            .with_file("/p/A.class")
            .with_owner("/p/A.class", 0);
        assert_eq!(resolve_in_order(&mut s, &leaf), Err(Unresolved::Dormant));
        assert!(
            !s.calls.iter().any(|c| c.starts_with("file:")),
            "{:?}",
            s.calls
        );
        // The same when no live owned panel matches the nth.
        let mut s = Script {
            owned_panels: vec![("graph-canvas", "/p/A.class", 0)],
            editable_files: vec!["/p/A.class"],
            ..Default::default()
        };
        let leaf = named("graph-canvas").with_owner("/p/A.class", 1);
        assert_eq!(resolve_in_order(&mut s, &leaf), Err(Unresolved::Dormant));
        assert!(!s.calls.iter().any(|c| c.starts_with("file:")));
    }

    #[test]
    fn an_owned_leaf_whose_owner_file_is_gone_is_gone_not_dormant() {
        let mut s = Script::default();
        let leaf = named("graph-canvas").with_owner("gone/A.class", 0);
        assert_eq!(resolve_in_order(&mut s, &leaf), Err(Unresolved::Gone));
        assert_eq!(s.calls, ["owned:graph-canvas:gone/A.class:0"]);
    }

    #[test]
    fn an_owned_leaf_may_still_use_a_restorer_after_a_miss() {
        let mut s = Script {
            known_kinds: vec!["k"],
            ..Default::default()
        };
        let leaf = named("p")
            .with_owner("/p/A", 0)
            .with_restore("k", serde_json::Value::Null);
        assert_eq!(resolve_in_order(&mut s, &leaf), Ok("restored:k".into()));
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

    /// A resolver that records the order leaves are asked in.
    #[derive(Default)]
    struct Order {
        asked: Vec<String>,
    }

    impl LeafResolver for Order {
        type Leaf = String;
        fn resolve_leaf(&mut self, leaf: &PanelState) -> Result<String, Unresolved> {
            let id = leaf
                .owner()
                .map(|(o, n)| format!("{}@{o}#{n}", leaf.panel_name))
                .or_else(|| leaf.file().map(String::from))
                .unwrap();
            self.asked.push(id.clone());
            Ok(id)
        }
        fn shelve(&mut self, _: PendingDormant) {}
    }

    #[test]
    fn owned_tabs_resolve_after_their_siblings_but_keep_their_place() {
        let tree = group(
            vec![
                named("graph-canvas").with_owner("/p/A", 0),
                named("P").with_file("a.class"),
                named("P").with_file("b.rs"),
            ],
            0,
        );
        let mut order = Order::default();
        let plan = resolve_tree(&mut order, &tree).unwrap();
        // The editor tabs are asked before the panel that lives inside one.
        assert_eq!(order.asked, ["a.class", "b.rs", "graph-canvas@/p/A#0"]);
        // The saved order and the active tab (index 0) are unchanged.
        assert_eq!(
            tabs(&plan),
            (
                &[
                    "graph-canvas@/p/A#0".to_string(),
                    "a.class".to_string(),
                    "b.rs".to_string()
                ][..],
                0
            )
        );
    }

    #[test]
    fn retain_leaves_prunes_like_resolve_does() {
        let metas = |n: usize| (0..n).map(|_| TileMeta::default()).collect::<Vec<_>>();
        let plan: Plan<&str> = Plan::Split {
            axis: Axis::Horizontal,
            sizes: vec![Some(gpui::px(1.)), Some(gpui::px(2.)), Some(gpui::px(3.))],
            children: vec![
                Plan::Tabs {
                    panels: vec!["a", "x"],
                    active: 1,
                },
                Plan::Tabs {
                    panels: vec!["x"],
                    active: 0,
                },
                Plan::Tiles {
                    metas: metas(1),
                    children: vec![Plan::Tabs {
                        panels: vec!["c"],
                        active: 0,
                    }],
                },
            ],
        };
        let kept = plan.clone().retain_leaves(&mut |p| *p != "x").unwrap();
        match kept {
            Plan::Split {
                sizes, children, ..
            } => {
                // The emptied middle group is gone and the sizes with it.
                assert_eq!(children.len(), 2);
                assert_eq!(sizes, vec![None, None]);
                // The active tab was removed: back to the first.
                assert_eq!(
                    children[0],
                    Plan::Tabs {
                        panels: vec!["a"],
                        active: 0
                    }
                );
            }
            other => panic!("expected split, got {other:?}"),
        }
        // Nothing removed: the plan is unchanged, sizes included.
        assert_eq!(
            plan.clone().retain_leaves(&mut |_| true),
            Some(plan.clone())
        );
        // Everything removed: nothing is left.
        assert!(plan.retain_leaves(&mut |_| false).is_none());

        // A split left with one child collapses into it.
        let two: Plan<&str> = Plan::Split {
            axis: Axis::Vertical,
            sizes: vec![None, None],
            children: vec![
                Plan::Tabs {
                    panels: vec!["x"],
                    active: 0,
                },
                Plan::Tabs {
                    panels: vec!["b"],
                    active: 0,
                },
            ],
        };
        assert_eq!(
            two.retain_leaves(&mut |p| *p == "b"),
            Some(Plan::Tabs {
                panels: vec!["b"],
                active: 0
            })
        );
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
