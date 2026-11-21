use super::*;

fn entry(path: &str) -> ManifestEntry {
    ManifestEntry {
        path: path.to_owned(),
        is_dir: false,
        size: 1,
        modified: Some(1),
    }
}

fn dep(source: &str, target: &str, via: &str) -> AssetDep {
    AssetDep {
        source_kind: "anim_graph".into(),
        source_id: source.into(),
        target_kind: "animseq".into(),
        target_id: target.into(),
        role: "hard".into(),
        via: via.into(),
        expect: "owner".into(),
    }
}

fn index() -> (tempfile::TempDir, ProjectFileIndex) {
    let temp = tempfile::tempdir().unwrap();
    let index = ProjectFileIndex::open(temp.path()).unwrap();
    index
        .reconcile(&[
            entry("a.animgraph"),
            entry("b.animgraph"),
            entry("walk.animseq"),
        ])
        .unwrap();
    (temp, index)
}

#[test]
fn used_by_is_derived_from_recorded_edges_and_replaced_wholesale() {
    let (temp, index) = index();
    let a = temp.path().join("a.animgraph");
    let b = temp.path().join("b.animgraph");
    index
        .replace_asset_deps(
            &a,
            "anim_graph",
            "A",
            &[dep("A", "WALK", "n1"), dep("A", "RUN", "n2")],
        )
        .unwrap();
    index
        .replace_asset_deps(&b, "anim_graph", "B", &[dep("B", "WALK", "n1")])
        .unwrap();

    let users = index.asset_used_by("animseq", "WALK").unwrap();
    assert_eq!(
        users
            .iter()
            .map(|u| u.dep.source_id.as_str())
            .collect::<Vec<_>>(),
        ["A", "B"]
    );
    assert_eq!(users[0].path, a);
    assert_eq!(index.asset_deps_of("anim_graph", "A").unwrap().len(), 2);

    // Saving A again with only RUN drops its WALK edge.
    index
        .replace_asset_deps(&a, "anim_graph", "A", &[dep("A", "RUN", "n2")])
        .unwrap();
    let users = index.asset_used_by("animseq", "WALK").unwrap();
    assert_eq!(
        users
            .iter()
            .map(|u| u.dep.source_id.as_str())
            .collect::<Vec<_>>(),
        ["B"]
    );
}

#[test]
fn edges_survive_reopen_follow_moves_and_die_with_the_file() {
    let (temp, index) = index();
    let a = temp.path().join("a.animgraph");
    index
        .replace_asset_deps(&a, "anim_graph", "A", &[dep("A", "WALK", "n1")])
        .unwrap();
    drop(index);

    let index = ProjectFileIndex::open(temp.path()).unwrap();
    assert_eq!(index.asset_used_by("animseq", "WALK").unwrap().len(), 1);

    index
        .move_path(&a, &temp.path().join("moved.animgraph"))
        .unwrap();
    let users = index.asset_used_by("animseq", "WALK").unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0].path, temp.path().join("moved.animgraph"));

    index
        .remove_path(&temp.path().join("moved.animgraph"))
        .unwrap();
    assert!(index.asset_used_by("animseq", "WALK").unwrap().is_empty());
}

#[test]
fn mismatched_source_and_unindexed_files_are_rejected() {
    let (temp, index) = index();
    let a = temp.path().join("a.animgraph");
    assert!(index
        .replace_asset_deps(&a, "anim_graph", "A", &[dep("OTHER", "WALK", "n1")])
        .is_err());
    let missing = temp.path().join("never.animgraph");
    assert!(index
        .replace_asset_deps(&missing, "anim_graph", "A", &[dep("A", "WALK", "n1")])
        .is_err());
    assert!(index.asset_used_by("animseq", "WALK").unwrap().is_empty());
}
