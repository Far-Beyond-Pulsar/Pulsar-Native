//! Dormant tabs: identified panels the current session could not rebuild.
//!
//! A saved tab that names a panel (a restore key, or a file) but cannot be
//! resolved *because nothing is registered to build it* -- the plugin that owns
//! it is disabled or missing -- is not dropped. Dropping it would erase the tab
//! the next time the layout is saved, so disabling a plugin for one session
//! would cost the user its tabs. Instead the resolver records a [`DormantLeaf`]
//! carrying the original [`PanelState`] and the layout writes it back verbatim
//! under `dormant`.
//!
//! The life cycle, all of it plain data (no GPUI) and unit tested:
//!
//! 1. **Resolve** finds an unbuildable-but-identified leaf and reports a
//!    [`PendingDormant`] (its state plus the identity of a sibling tab that did
//!    resolve, so it can be put back in the same group).
//! 2. **Save** writes the list ([`prune`] first: bounded, deduplicated, and
//!    without file tabs whose file is gone).
//! 3. **Load** calls [`reinsert`], which puts every dormant leaf back into the
//!    saved tree before resolution. So a dormant leaf is simply offered again
//!    with everything else: if its plugin is back it resolves into a normal tab
//!    (and is no longer dormant); if not, it becomes dormant again.
//!
//! A leaf whose *file is missing*, whose restorer *declined*, or that never had
//! an identity is not dormant: it is gone.

use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use ui::dock::{PanelInfo, PanelState};

/// The most dormant tabs one window keeps. Oldest are dropped first.
pub(super) const MAX_DORMANT: usize = 64;

/// Which saved root a dormant leaf belonged to. A pop-out is a root of its
/// own, named by its stable id rather than its position (positions shift when
/// a pop-out is closed).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DormantRoot {
    Center,
    Left,
    Right,
    Bottom,
    /// The tree of the saved pop-out with this id (`SavedPopout::id`).
    Popout(u32),
}

/// The identity of a tab, used to find the group a dormant leaf sat in.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum AnchorKey {
    File(String),
    Restore { kind: String, data: Value },
    Name(String),
}

impl AnchorKey {
    /// A tab's identity: its file, else its restore key, else its panel name.
    pub fn of(state: &PanelState) -> Self {
        if let Some(file) = state.file() {
            AnchorKey::File(file.to_string())
        } else if let Some((kind, data)) = state.restore_key() {
            AnchorKey::Restore {
                kind: kind.to_string(),
                data: data.clone(),
            }
        } else {
            AnchorKey::Name(state.panel_name.clone())
        }
    }
}

/// A tab kept for a later session.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct DormantLeaf {
    pub root: DormantRoot,
    /// A sibling tab that resolved when this one was shelved. The leaf returns
    /// to that sibling's group; without one it goes to the root's first group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anchor: Option<AnchorKey>,
    /// The saved tab, verbatim.
    pub state: PanelState,
}

/// A leaf the resolver shelved, not yet assigned to a root.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct PendingDormant {
    pub anchor: Option<AnchorKey>,
    pub state: PanelState,
}

impl PendingDormant {
    pub fn into_leaf(self, root: DormantRoot) -> DormantLeaf {
        DormantLeaf {
            root,
            anchor: self.anchor,
            state: self.state,
        }
    }
}

/// Deserialize a list leniently: an entry that does not parse is skipped
/// instead of making the whole window slot unreadable.
pub(super) fn lenient<'de, D>(deserializer: D) -> Result<Vec<DormantLeaf>, D::Error>
where
    D: Deserializer<'de>,
{
    let values = Vec::<Value>::deserialize(deserializer)?;
    Ok(values
        .into_iter()
        .filter_map(|value| match serde_json::from_value(value) {
            Ok(leaf) => Some(leaf),
            Err(error) => {
                tracing::warn!("layout: dropping unreadable dormant tab: {error}");
                None
            }
        })
        .collect())
}

/// Keep `leaves` bounded and meaningful before they are written: duplicates
/// (same root and same identity) collapse to the latest, file tabs whose file
/// no longer exists (`file_exists`) are dropped, and at most [`MAX_DORMANT`]
/// remain (the newest).
pub(super) fn prune(
    leaves: Vec<DormantLeaf>,
    file_exists: impl Fn(&Path) -> bool,
) -> Vec<DormantLeaf> {
    let mut kept: Vec<DormantLeaf> = Vec::new();
    for leaf in leaves {
        // A tab of a file that is gone, or an owned panel of an editor whose
        // file is gone, has nothing to come back to.
        let files = leaf
            .state
            .file()
            .into_iter()
            .chain(leaf.state.owner().map(|(owner, _)| owner));
        if let Some(file) = files.into_iter().find(|f| !file_exists(Path::new(f))) {
            tracing::debug!("layout: dropping dormant {file} (file is gone)");
            continue;
        }
        kept.retain(|k| !(k.root == leaf.root && k.state.same_identity(&leaf.state)));
        kept.push(leaf);
    }
    if kept.len() > MAX_DORMANT {
        kept.drain(..kept.len() - MAX_DORMANT);
    }
    kept
}

/// Drop leaves that belong to a pop-out which is not part of the save (the
/// user closed it): there is nowhere to offer them again.
pub(super) fn drop_orphaned(leaves: &mut Vec<DormantLeaf>, saved_popouts: &[u32]) {
    leaves.retain(|leaf| match leaf.root {
        DormantRoot::Popout(id) => saved_popouts.contains(&id),
        _ => true,
    });
}

/// Put `leaf` back into the tab group of `tree` where it belongs: the group
/// holding the `anchor` tab, else the first tab group in the tree. `false` if
/// the tree has no tab group at all.
pub(super) fn reinsert(tree: &mut PanelState, leaf: &DormantLeaf) -> bool {
    if let Some(anchor) = &leaf.anchor {
        if insert_by(tree, &leaf.state, &mut |group| {
            group.children.iter().any(|c| &AnchorKey::of(c) == anchor)
        }) {
            return true;
        }
    }
    insert_by(tree, &leaf.state, &mut |_| true)
}

/// Append `state` to the first tab group (depth first) for which `wanted` holds.
fn insert_by(
    tree: &mut PanelState,
    state: &PanelState,
    wanted: &mut dyn FnMut(&PanelState) -> bool,
) -> bool {
    if matches!(tree.info, PanelInfo::Tabs { .. }) && wanted(tree) {
        tree.children.push(state.clone());
        return true;
    }
    tree.children
        .iter_mut()
        .any(|child| insert_by(child, state, wanted))
}

#[cfg(test)]
mod tests {
    use super::super::format::tests::tab_group;
    use super::*;

    fn leaf(root: DormantRoot, anchor: Option<&str>, file: &str) -> DormantLeaf {
        DormantLeaf {
            root,
            anchor: anchor.map(|a| AnchorKey::File(a.into())),
            state: PanelState::default().with_file(file),
        }
    }

    #[test]
    fn an_owned_dormant_tab_survives_pruning_and_dies_with_its_owner() {
        let owned = |owner: &str, nth| DormantLeaf {
            root: DormantRoot::Popout(0),
            anchor: None,
            state: PanelState {
                panel_name: "graph-canvas".into(),
                ..Default::default()
            }
            .with_owner(owner, nth),
        };
        let kept = prune(
            vec![
                owned("a.class", 0),
                owned("a.class", 1),
                owned("gone.class", 0),
            ],
            |p| !p.to_string_lossy().starts_with("gone"),
        );
        // Both canvases of an existing owner stay (they differ in nth); the one
        // of a vanished owner goes. The owner marker is carried verbatim.
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[1].state.owner(), Some(("a.class", 1)));
        // The same identity collapses to the latest.
        let kept = prune(vec![owned("a.class", 0), owned("a.class", 0)], |_| true);
        assert_eq!(kept.len(), 1);
    }

    #[test]
    fn anchor_key_prefers_file_then_restore_then_name() {
        let file = PanelState::default()
            .with_file("a.rs")
            .with_restore("k", Value::Null);
        assert_eq!(AnchorKey::of(&file), AnchorKey::File("a.rs".into()));
        let restore = PanelState::default().with_restore("k", Value::from(1));
        assert_eq!(
            AnchorKey::of(&restore),
            AnchorKey::Restore {
                kind: "k".into(),
                data: Value::from(1)
            }
        );
        let named = PanelState {
            panel_name: "Level Editor".into(),
            ..Default::default()
        };
        assert_eq!(
            AnchorKey::of(&named),
            AnchorKey::Name("Level Editor".into())
        );
    }

    #[test]
    fn a_dormant_leaf_returns_to_its_anchors_group() {
        let mut tree = PanelState {
            panel_name: "StackPanel".into(),
            children: vec![tab_group(&["a.rs"]), tab_group(&["b.rs", "c.rs"])],
            info: PanelInfo::stack(vec![gpui::px(1.), gpui::px(1.)], gpui::Axis::Horizontal),
        };
        assert!(reinsert(
            &mut tree,
            &leaf(DormantRoot::Center, Some("c.rs"), "x.dat")
        ));
        assert_eq!(tree.children[0].children.len(), 1);
        let second = &tree.children[1].children;
        assert_eq!(second.len(), 3);
        assert_eq!(second[2].file(), Some("x.dat"));
    }

    #[test]
    fn without_a_findable_anchor_it_goes_to_the_first_group() {
        for anchor in [None, Some("vanished.rs")] {
            let mut tree = tab_group(&["a.rs"]);
            assert!(reinsert(
                &mut tree,
                &leaf(DormantRoot::Center, anchor, "x.dat")
            ));
            assert_eq!(tree.children.len(), 2);
        }
    }

    #[test]
    fn a_tree_with_no_tab_group_takes_nothing() {
        let mut tree = PanelState::default();
        assert!(!reinsert(
            &mut tree,
            &leaf(DormantRoot::Left, None, "x.dat")
        ));
    }

    #[test]
    fn prune_drops_gone_files_dedups_and_bounds() {
        let leaves = vec![
            leaf(DormantRoot::Center, None, "keep.dat"),
            leaf(DormantRoot::Center, None, "gone.dat"),
            leaf(DormantRoot::Center, Some("a"), "keep.dat"),
            leaf(DormantRoot::Left, None, "keep.dat"),
        ];
        let kept = prune(leaves, |p| !p.to_string_lossy().starts_with("gone"));
        // `gone` dropped; the two Center `keep.dat` collapse to the later one;
        // the Left one is a different root.
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].anchor, Some(AnchorKey::File("a".into())));
        assert_eq!(kept[1].root, DormantRoot::Left);

        let many: Vec<_> = (0..MAX_DORMANT + 10)
            .map(|i| leaf(DormantRoot::Center, None, &format!("f{i}.dat")))
            .collect();
        let kept = prune(many, |_| true);
        assert_eq!(kept.len(), MAX_DORMANT);
        assert_eq!(kept[0].state.file(), Some("f10.dat"));
        assert_eq!(
            kept.last().unwrap().state.file(),
            Some(format!("f{}.dat", MAX_DORMANT + 9).as_str())
        );
    }

    #[test]
    fn leaves_of_a_closed_popout_are_dropped() {
        let mut leaves = vec![
            leaf(DormantRoot::Center, None, "a.dat"),
            leaf(DormantRoot::Popout(1), None, "b.dat"),
            leaf(DormantRoot::Popout(2), None, "c.dat"),
        ];
        drop_orphaned(&mut leaves, &[2]);
        assert_eq!(leaves.len(), 2);
        assert_eq!(leaves[1].root, DormantRoot::Popout(2));
    }

    #[test]
    fn restore_keyed_dormant_leaves_are_kept_without_a_file_check() {
        let l = DormantLeaf {
            root: DormantRoot::Right,
            anchor: None,
            state: PanelState::default().with_restore("plug.panel", Value::Null),
        };
        assert_eq!(prune(vec![l.clone()], |_| false), vec![l]);
    }

    #[test]
    fn unreadable_entries_are_skipped_not_fatal() {
        #[derive(Deserialize)]
        struct Holder(#[serde(deserialize_with = "lenient")] Vec<DormantLeaf>);
        let good = serde_json::to_value(leaf(DormantRoot::Center, None, "a.dat")).unwrap();
        let holder: Holder =
            serde_json::from_value(serde_json::json!([good, { "root": "nowhere" }, 7])).unwrap();
        assert_eq!(holder.0.len(), 1);
    }

    #[test]
    fn popout_roots_round_trip_beside_the_old_string_roots() {
        let l = leaf(DormantRoot::Popout(3), None, "x.dat");
        let json = serde_json::to_value(&l).unwrap();
        assert_eq!(json["root"], serde_json::json!({ "popout": 3 }));
        assert_eq!(serde_json::from_value::<DormantLeaf>(json).unwrap(), l);
        // Files written before pop-out roots existed still read.
        let old = serde_json::json!({ "root": "center", "state": l.state });
        assert_eq!(
            serde_json::from_value::<DormantLeaf>(old).unwrap().root,
            DormantRoot::Center
        );
    }

    #[test]
    fn leaves_round_trip_through_json() {
        let l = leaf(DormantRoot::Bottom, Some("a.rs"), "x.dat");
        let back: DormantLeaf = serde_json::from_value(serde_json::to_value(&l).unwrap()).unwrap();
        assert_eq!(back, l);
    }
}
