//! Owned panels: the inner panels of an editor, found again after a restart.
//!
//! An editor with a dock of its own (the Blueprint editor's graph canvas, a level
//! editor's hierarchy...) owns panels the user can drag out into a pop-out
//! window or into the main dock. Such a panel has no file and no restore key of
//! its own, and it cannot be rebuilt on its own: it only exists as a part of its
//! editor. So it is saved with an **owner marker** (`PanelState::with_owner`:
//! the owner editor's file and an `nth`) and, on restore, *looked up* among the
//! live panels of the owner editor (which the resolver has just re-created) and
//! claimed, instead of being created.
//!
//! A panel declares its owner by implementing `Panel::owner_file`; the dock adds
//! the marker to its `dump()`. The host never needs to know which editor a panel
//! belongs to beyond that file.
//!
//! ## Identity and order
//!
//! A saved owned panel is `(panel_name, owner file, nth)`. `nth` tells apart
//! several panels of one name owned by one editor (a Blueprint with several graph
//! tabs): it is the panel's rank among the live panels with the same name and
//! owner, in one **canonical order** shared by save and restore: the dock areas
//! of ordinary windows first (construction order), then pop-outs, and inside a
//! dock area its tab groups in tree order, each group's tabs in order
//! ([`live_owned`]). Save assigns `nth` from that order
//! ([`assign_nth_in_tree`]); restore indexes into the same order ([`pick`]).
//!
//! The order is stable for the usual case (one such panel per owner). With
//! several, `nth` follows the panels' *positions* and not an identity of their
//! own, so after rearranging them by hand the restored pop-out may show a
//! sibling of the one that was saved. Giving the panel an identity is up to the
//! panel.
//!
//! The pure parts here (path comparison, ranking, `nth` assignment, picking) are
//! unit tested; [`live_owned`] is the GPUI part.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use gpui::{App, Entity, EntityId};
use ui::dock::{all_dock_areas, DockPlacement, PanelInfo, PanelState, PanelView, TabPanel};

use super::dormant::DormantRoot;

/// A live owned panel, with where it sits now.
pub(super) struct OwnedLive {
    pub panel: Arc<dyn PanelView>,
    /// The tab group it is in now (its home, if it is claimed and moved).
    pub tab_panel: Entity<TabPanel>,
    /// The dock area that tab group belongs to.
    pub area: EntityId,
    pub placement: DockPlacement,
    pub name: &'static str,
    pub owner: String,
    /// Its rank among the live panels with the same name and owner.
    pub nth: usize,
}

impl OwnedLive {
    pub fn id(&self) -> EntityId {
        self.panel.view().entity_id()
    }
}

/// Every live panel that declares an owner, in the canonical order, with its
/// `nth` filled in.
pub(super) fn live_owned(cx: &App) -> Vec<OwnedLive> {
    let mut out = Vec::new();
    for info in all_dock_areas(cx) {
        let Some(area) = info.dock_area.upgrade() else {
            continue;
        };
        for (placement, tab_panel) in area.read(cx).tab_panels(cx) {
            for panel in tab_panel.read(cx).all_panels() {
                // Name first: most panels are not owned, and dumping them is
                // the expensive part.
                let name = panel.panel_name(cx);
                let state = panel.dump(cx);
                let Some((owner, _)) = state.owner() else {
                    continue;
                };
                out.push(OwnedLive {
                    panel,
                    tab_panel: tab_panel.clone(),
                    area: area.entity_id(),
                    placement,
                    name,
                    owner: owner.to_string(),
                    nth: 0,
                });
            }
        }
    }
    let keys: Vec<(&str, String)> = out
        .iter()
        .map(|l| (l.name, normalize(&l.owner)))
        .collect();
    let ranks = rank_in_order(&keys);
    for (live, rank) in out.iter_mut().zip(ranks) {
        live.nth = rank;
    }
    out
}

/// The live panel a saved owned leaf stands for, if there is one.
pub(super) fn find<'a>(
    live: &'a [OwnedLive],
    panel_name: &str,
    owner: &str,
    nth: usize,
) -> Option<&'a OwnedLive> {
    let owner = normalize(owner);
    pick(live, |l| l.name == panel_name && normalize(&l.owner) == owner, nth)
}

// ── Pure parts ───────────────────────────────────────────────────────────────

/// A path as compared for ownership: separators unified, no trailing separator,
/// case-insensitive on Windows (where `D:\P\A.class` and `d:/p/a.class` are one
/// file). Saved paths are rebuilt by joining a relative path onto the project
/// root, so they can differ from the live path in separators.
pub(super) fn normalize(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let trimmed = unified.trim_end_matches('/');
    if cfg!(windows) {
        trimmed.to_lowercase()
    } else {
        trimmed.to_string()
    }
}

/// For each key, how many equal keys come before it.
pub(super) fn rank_in_order<K: PartialEq>(keys: &[K]) -> Vec<usize> {
    (0..keys.len())
        .map(|i| keys[..i].iter().filter(|k| **k == keys[i]).count())
        .collect()
}

/// The `nth` item (zero based) of those `matches`, in list order. The count is
/// over *all* matching items, claimed or not, so one saved leaf can never take
/// the place of another.
pub(super) fn pick<T>(items: &[T], matches: impl Fn(&T) -> bool, nth: usize) -> Option<&T> {
    items.iter().filter(|item| matches(item)).nth(nth)
}

/// Whether `owner` (a path as recorded) names a file or folder that exists.
pub(super) fn owner_exists(owner: &str) -> bool {
    Path::new(owner).exists()
}

/// The leaves of `tree` with an owner marker, in tree order.
fn owned_leaves_mut<'a>(tree: &'a mut PanelState, out: &mut Vec<&'a mut PanelState>) {
    if matches!(tree.info, PanelInfo::Panel(_)) {
        if tree.owner().is_some() {
            out.push(tree);
        }
        return;
    }
    for child in &mut tree.children {
        owned_leaves_mut(child, out);
    }
}

/// Write each owned leaf of the saved `tree` the `nth` of the live panel it was
/// dumped from. `live` lists `(panel_name, nth)` of the live owned panels of this
/// root in tree order; the two sequences must line up name for name, otherwise
/// nothing is changed and `false` is returned (the `nth`s stay 0).
pub(super) fn assign_nth_in_tree(tree: &mut PanelState, live: &[(String, usize)]) -> bool {
    let mut leaves = Vec::new();
    owned_leaves_mut(tree, &mut leaves);
    if leaves.len() != live.len()
        || leaves
            .iter()
            .zip(live)
            .any(|(leaf, (name, _))| leaf.panel_name != *name)
    {
        return false;
    }
    for (leaf, (_, nth)) in leaves.into_iter().zip(live) {
        leaf.set_owner_nth(*nth);
    }
    true
}

/// The live owned panels of each saved root, as [`assign_nth_in_tree`] wants
/// them. `main` is the window's own dock area; `popouts` maps each pop-out's dock
/// area to its saved id. Panels in other dock areas (the editors' own docks) are
/// not part of any saved root.
pub(super) fn ranks_by_root(
    live: &[OwnedLive],
    main: EntityId,
    popouts: &[(EntityId, u32)],
) -> HashMap<DormantRoot, Vec<(String, usize)>> {
    let mut by_root: HashMap<DormantRoot, Vec<(String, usize)>> = HashMap::new();
    for panel in live {
        let root = if panel.area == main {
            match panel.placement {
                DockPlacement::Center => DormantRoot::Center,
                DockPlacement::Left => DormantRoot::Left,
                DockPlacement::Right => DormantRoot::Right,
                DockPlacement::Bottom => DormantRoot::Bottom,
            }
        } else if let Some((_, id)) = popouts.iter().find(|(area, _)| *area == panel.area) {
            DormantRoot::Popout(*id)
        } else {
            continue;
        };
        by_root
            .entry(root)
            .or_default()
            .push((panel.name.to_string(), panel.nth));
    }
    by_root
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leaf(name: &str, owner: Option<(&str, usize)>) -> PanelState {
        let state = PanelState {
            panel_name: name.into(),
            ..Default::default()
        };
        match owner {
            Some((file, nth)) => state.with_owner(file, nth),
            None => state.with_file("f.rs"),
        }
    }

    fn group(children: Vec<PanelState>) -> PanelState {
        PanelState {
            panel_name: "TabPanel".into(),
            children,
            info: PanelInfo::tabs(0),
        }
    }

    #[test]
    fn ranks_count_equal_keys_before_each_item() {
        assert_eq!(rank_in_order(&["a", "b", "a", "a", "b"]), [0, 0, 1, 2, 1]);
        assert_eq!(rank_in_order::<u8>(&[]), Vec::<usize>::new());
    }

    #[test]
    fn pick_indexes_the_matches_not_the_list() {
        let items = ["x", "a", "x", "a", "a"];
        let is_a = |s: &&str| *s == "a";
        assert_eq!(pick(&items, is_a, 0), Some(&"a"));
        // The third "a" is at list position 4.
        assert!(std::ptr::eq(pick(&items, is_a, 2).unwrap(), &items[4]));
        assert!(pick(&items, is_a, 3).is_none());
    }

    #[test]
    fn owner_paths_compare_across_separators_and_trailing_slashes() {
        assert_eq!(normalize("/p/A.class/"), normalize("/p/A.class"));
        assert_eq!(normalize(r"C:\p\a/b.class"), normalize("C:/p/a/b.class"));
        assert_ne!(normalize("/p/A.class"), normalize("/p/B.class"));
    }

    #[test]
    fn nth_is_written_to_owned_leaves_in_tree_order() {
        let mut tree = group(vec![
            leaf("graph-canvas", Some(("A", 0))),
            leaf("Editor", None),
            leaf("graph-canvas", Some(("A", 0))),
        ]);
        let live = vec![("graph-canvas".to_string(), 1), ("graph-canvas".to_string(), 2)];
        assert!(assign_nth_in_tree(&mut tree, &live));
        assert_eq!(tree.children[0].owner(), Some(("A", 1)));
        assert_eq!(tree.children[2].owner(), Some(("A", 2)));
        assert_eq!(tree.children[1].owner(), None);
    }

    #[test]
    fn a_mismatched_pairing_changes_nothing() {
        let mut tree = group(vec![leaf("graph-canvas", Some(("A", 0)))]);
        // Wrong name.
        assert!(!assign_nth_in_tree(&mut tree, &[("palette".into(), 3)]));
        // Wrong count.
        assert!(!assign_nth_in_tree(&mut tree, &[]));
        assert!(!assign_nth_in_tree(
            &mut tree,
            &[("graph-canvas".into(), 1), ("graph-canvas".into(), 2)]
        ));
        assert_eq!(tree.children[0].owner(), Some(("A", 0)));
        // Nothing owned on either side is fine.
        let mut plain = group(vec![leaf("Editor", None)]);
        assert!(assign_nth_in_tree(&mut plain, &[]));
    }

    #[test]
    fn save_then_restore_pair_up_by_the_same_rank() {
        // Two canvases of editor A live in A's own dock; B has one. The popout
        // holds A's second canvas.
        let live_names = [("g", "A"), ("g", "A"), ("g", "B")];
        let keys: Vec<(&str, String)> = live_names
            .iter()
            .map(|(n, o)| (*n, normalize(o)))
            .collect();
        let ranks = rank_in_order(&keys);
        assert_eq!(ranks, [0, 1, 0]);

        // Save: the popout tree holds the second canvas of A.
        let mut popout = group(vec![leaf("g", Some(("A", 0)))]);
        assert!(assign_nth_in_tree(&mut popout, &[("g".into(), ranks[1])]));
        let (owner, nth) = popout.children[0].owner().unwrap();
        assert_eq!((owner, nth), ("A", 1));

        // Restore: the fresh editors have the same three panels in the same
        // order; the saved leaf finds the same one.
        let found = pick(
            &live_names,
            |(n, o)| *n == "g" && normalize(o) == normalize(owner),
            nth,
        )
        .unwrap();
        assert!(std::ptr::eq(found, &live_names[1]));
    }
}
