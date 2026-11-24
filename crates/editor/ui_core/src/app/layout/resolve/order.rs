//! The order in which a saved tab is resolved to a panel, kept apart from GPUI
//! so it can be tested with fakes.

use ui::dock::PanelState;

use super::*;

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
