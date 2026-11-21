use super::super::format::tests::tab_group;
use super::window::anchor_requirement_met;
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
