use super::registry::SHARED_FILE_INDEXES;
use super::*;
use crate::events::{FsChangeKind, FsEventSource};
use std::collections::HashMap;
use std::sync::Arc;

fn file(path: &str, is_dir: bool, size: u64, modified: Option<u64>) -> ManifestEntry {
    ManifestEntry {
        path: path.to_owned(),
        is_dir,
        size,
        modified,
    }
}

#[test]
fn a_durable_id_on_an_unlisted_file_does_not_fail_the_reopen() {
    // `target/` is left out of the manifest, but a picker can still give
    // a file there a durable id (it refreshes the file in first). The
    // next open's reconcile drops that row; restoring the id must not
    // then trip `asset_ids.path REFERENCES files(path)` and fail the open.
    let temp = tempfile::tempdir().unwrap();
    let root = crate::virtual_fs::canonicalize(temp.path()).unwrap();
    let root = normalize_absolute_path(root);
    let listed = root.join("Assets").join("wood.png");
    let unlisted = root.join("target").join("sky.hdr");
    for path in [&listed, &unlisted] {
        crate::virtual_fs::create_dir_all(path.parent().unwrap()).unwrap();
        crate::virtual_fs::write_file(path, b"x").unwrap();
    }
    let manifest = [
        file("Assets", true, 0, None),
        file("Assets/wood.png", false, 1, Some(1)),
    ];
    let listed_id;
    {
        let index = ProjectFileIndex::open(root.clone()).unwrap();
        index.reconcile(&manifest).unwrap();
        listed_id = index.ensure_typed_asset_id("texture", &listed).unwrap();
        index.ensure_typed_asset_id("texture", &unlisted).unwrap();
    }

    let index = ProjectFileIndex::open(root.clone()).unwrap();
    index.reconcile(&manifest).unwrap();
    index
        .restore_durable_asset_ids()
        .expect("an id on an unlisted file is skipped, not a failed open");
    let connection = index.connection.lock();
    let restored: Vec<(String, String)> = connection
        .prepare("SELECT asset_id, path FROM asset_ids WHERE kind = 'texture'")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<std::result::Result<_, _>>()
        .unwrap();
    assert_eq!(restored, vec![(listed_id, "Assets/wood.png".to_owned())]);
}

#[test]
fn persists_manifest_and_queries_filters_after_reopen() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().to_path_buf();
    let index = ProjectFileIndex::open(root.clone()).unwrap();
    index
        .reconcile(&[
            file("Assets", true, 0, None),
            file("Assets/wood.PNG", false, 4096, Some(100)),
            file("Assets/scene.json", false, 512, Some(200)),
            file(".pulsar/cache/files.db", false, 1, Some(300)),
        ])
        .unwrap();
    assert!(index.database_path().exists());
    let pngs = index
        .query(&FileIndexQuery {
            extension: Some(".PNG".to_owned()),
            folder: Some("Assets".to_owned()),
            modified_after: Some(100),
            min_size: Some(1000),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        pngs.iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["Assets/wood.PNG"]
    );
    drop(index);

    let reopened = ProjectFileIndex::open(root).unwrap();
    assert_eq!(reopened.query(&FileIndexQuery::default()).unwrap().len(), 2);
    assert_eq!(reopened.file_count().unwrap(), 3); // includes the Assets directory
}

#[test]
fn reconcile_removes_stale_paths_and_refresh_can_delete_subtree() {
    let temp = tempfile::tempdir().unwrap();
    let index = ProjectFileIndex::open(temp.path()).unwrap();
    index
        .reconcile(&[
            file("Assets", true, 0, None),
            file("Assets/a.png", false, 10, Some(1)),
            file("Assets/b.png", false, 20, Some(2)),
        ])
        .unwrap();
    index
        .reconcile(&[
            file("Assets", true, 0, None),
            file("Assets/b.png", false, 21, Some(3)),
        ])
        .unwrap();
    assert_eq!(index.query(&FileIndexQuery::default()).unwrap().len(), 1);
    index.remove_path(&temp.path().join("Assets")).unwrap();
    assert!(index
        .query(&FileIndexQuery {
            include_directories: true,
            ..Default::default()
        })
        .unwrap()
        .is_empty());
}

#[test]
fn query_name_search_treats_percent_as_literal_and_sorts() {
    let temp = tempfile::tempdir().unwrap();
    let index = ProjectFileIndex::open(temp.path()).unwrap();
    index
        .reconcile(&[
            file("z/100%.txt", false, 1, Some(1)),
            file("a/100x.txt", false, 2, Some(2)),
        ])
        .unwrap();
    let result = index
        .query(&FileIndexQuery {
            name_contains: Some("100%".to_owned()),
            sort: FileIndexSort::Name,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].path, "z/100%.txt");
}

#[test]
fn direct_children_and_duplicate_asset_ids_are_indexed() {
    let temp = tempfile::tempdir().unwrap();
    let index = ProjectFileIndex::open(temp.path()).unwrap();
    index
        .reconcile(&[
            file("Assets", true, 0, None),
            file("Assets/a.widget", false, 100, Some(1)),
            file("Assets/nested", true, 0, None),
            file("Assets/nested/b.widget", false, 100, Some(1)),
        ])
        .unwrap();
    let entries = index
        .query(&FileIndexQuery {
            direct_children_of: Some("Assets".to_owned()),
            include_directories: true,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(entries.len(), 2);
    assert!(entries.iter().any(|entry| entry.path == "Assets/nested"));

    index
        .record_asset_id("widget-1", &temp.path().join("Assets/a.widget"))
        .unwrap();
    index
        .record_asset_id("widget-1", &temp.path().join("Assets/nested/b.widget"))
        .unwrap();
    assert_eq!(index.paths_for_asset_id("widget-1").unwrap().len(), 2);
    index
        .remove_path(&temp.path().join("Assets/nested/b.widget"))
        .unwrap();
    assert_eq!(index.paths_for_asset_id("widget-1").unwrap().len(), 1);
}

#[test]
fn a_batch_resolves_like_one_lookup_per_id() {
    let temp = tempfile::tempdir().unwrap();
    let index = ProjectFileIndex::open(temp.path()).unwrap();
    index
        .reconcile(&[
            file("a.tex", false, 1, Some(1)),
            file("b.tex", false, 1, Some(1)),
            file("c.tex", false, 1, Some(1)),
            file("m.mesh", false, 1, Some(1)),
        ])
        .unwrap();
    let path = |name: &str| temp.path().join(name);
    index.record_typed_asset_id("texture", "t1", &path("a.tex")).unwrap();
    // Claimed twice: ambiguous, so absent (as a single lookup answers None).
    index.record_typed_asset_id("texture", "t2", &path("b.tex")).unwrap();
    index.record_typed_asset_id("texture", "t2", &path("c.tex")).unwrap();
    // Another kind.
    index.record_typed_asset_id("mesh", "m1", &path("m.mesh")).unwrap();

    let found = index
        .paths_for_asset_ids_kind("texture", ["t1", "t2", "m1", "missing", "t1"])
        .unwrap();
    assert_eq!(found.len(), 1);
    assert_eq!(found["t1"], path("a.tex"));
    for id in ["t1", "t2", "m1", "missing"] {
        assert_eq!(
            found.get(id).cloned(),
            index.path_for_asset_id_kind("texture", id).unwrap(),
            "{id}"
        );
    }
}

#[test]
fn local_vfs_events_update_only_already_open_indexes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let index = Arc::new(ProjectFileIndex::open(root.clone()).unwrap());
    SHARED_FILE_INDEXES
        .get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
        .lock()
        .insert(root.clone(), index.clone());
    let file_path = root.join("created.widget");
    std::fs::write(&file_path, "{}").unwrap();

    crate::events::emit_with_source(
        file_path.clone(),
        FsChangeKind::Created,
        FsEventSource::Local,
    );
    assert!(index
        .query(&FileIndexQuery {
            extension: Some("widget".to_owned()),
            ..Default::default()
        })
        .unwrap()
        .iter()
        .any(|entry| entry.path == "created.widget"));
    assert!(
        project_file_index_for_path(&file_path).is_some_and(|found| Arc::ptr_eq(&found, &index))
    );

    crate::events::emit_with_source(
        file_path.clone(),
        FsChangeKind::Deleted,
        FsEventSource::Remote,
    );
    assert_eq!(index.query(&FileIndexQuery::default()).unwrap().len(), 1);

    crate::events::emit_with_source(
        file_path.clone(),
        FsChangeKind::Deleted,
        FsEventSource::Local,
    );
    assert!(index.query(&FileIndexQuery::default()).unwrap().is_empty());
    SHARED_FILE_INDEXES.get().unwrap().lock().remove(&root);
}

#[test]
fn local_vfs_rename_moves_rows_and_preserves_stable_ids() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let old_path = root.join("Widgets/Before.widget");
    let new_path = root.join("Widgets/After.widget");
    std::fs::create_dir_all(old_path.parent().unwrap()).unwrap();
    std::fs::write(&old_path, "widget").unwrap();
    let index = Arc::new(ProjectFileIndex::open(root.clone()).unwrap());
    index
        .reconcile(&crate::virtual_fs::manifest(&root).unwrap())
        .unwrap();
    index
        .record_asset_id("widget-stable-id", &old_path)
        .unwrap();
    SHARED_FILE_INDEXES
        .get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
        .lock()
        .insert(root.clone(), index.clone());

    crate::virtual_fs::rename(&old_path, &new_path).unwrap();

    assert_eq!(
        index.paths_for_asset_id("widget-stable-id").unwrap(),
        [normalize_absolute_path(new_path.clone())]
    );
    assert!(index
        .query(&FileIndexQuery::default())
        .unwrap()
        .iter()
        .any(|entry| entry.path == "Widgets/After.widget"));
    SHARED_FILE_INDEXES.get().unwrap().lock().remove(&root);
}

#[test]
fn refreshing_a_directory_preserves_ids_and_removes_deleted_descendants() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let assets = root.join("Assets");
    let existing = assets.join("existing.widget");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(&existing, "{}").unwrap();

    let index = ProjectFileIndex::open(root).unwrap();
    index.refresh_path(&assets).unwrap();
    index.record_asset_id("widget-id", &existing).unwrap();

    std::fs::write(assets.join("new.widget"), "{}").unwrap();
    index.refresh_path(&assets).unwrap();
    assert_eq!(
        index.paths_for_asset_id("widget-id").unwrap(),
        vec![normalize_absolute_path(existing.clone())]
    );
    assert!(index
        .query(&FileIndexQuery {
            extension: Some("widget".to_owned()),
            ..Default::default()
        })
        .unwrap()
        .iter()
        .any(|entry| entry.path == "Assets/new.widget"));

    std::fs::remove_file(&existing).unwrap();
    index.refresh_path(&assets).unwrap();
    assert!(index.paths_for_asset_id("widget-id").unwrap().is_empty());
}
