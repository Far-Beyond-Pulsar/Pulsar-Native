//! First step of a restore: turn a saved tree into a [`Plan`] without touching the
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
//! - [`Plan::leaves`] / [`admit`](window::ResolveState::admit): the validation hooks a
//!   zoom-target check would use.

mod gpui_resolver;
mod order;
mod snapshot;
mod window;

#[cfg(test)]
mod tests;

pub(crate) use gpui_resolver::lazy_builder;
pub(in crate::app::layout) use snapshot::{LivePanel, LiveSnapshot};
pub(in crate::app::layout) use window::{resolve_layout, PopoutPlan, ResolvedLayout};

use gpui_resolver::Resolver;
use order::{resolve_in_order, FileStep, LeafSources, OwnedStep, RestoreStep};
use snapshot::Claims;

use gpui::{Axis, Pixels};
use ui::dock::{PanelInfo, PanelState, TileMeta};

use super::dormant::{AnchorKey, PendingDormant};

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

    /// A sleeping stand-in for a tab that need not be awake yet: only its saved
    /// state is held, and it is rebuilt when first displayed. `active` says the
    /// tab is the displayed one of its group. `None` builds it now.
    fn sleeping(&mut self, _leaf: &PanelState, _active: bool) -> Option<Self::Leaf> {
        None
    }
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
            for (ix, child) in owned_last {
                // Ephemeral tabs are deliberately not restored: a decision the
                // panel made, not a loss.
                if child.is_ephemeral() {
                    tracing::debug!("layout: skipping ephemeral tab {}", child.panel_name);
                    continue;
                }
                if let Some(panel) = resolver.sleeping(child, ix == *active_index) {
                    results[ix] = Some(Ok(panel));
                    continue;
                }
                results[ix] = Some(resolver.resolve_leaf(child));
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
