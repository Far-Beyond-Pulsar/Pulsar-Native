//! The engine's import service with the meshes' model importer: linking a
//! source, re-importing it, converting in place, unlinking.

// The model importer registers itself (`asset_import::ImporterRegistration`).
use meshes_component as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use asset_import::{
    ignore_sources, submit_import, submit_reimport, unignore_sources, unlink_source, ImportDb,
    ImportMode, OptionValues, TRASH_DIR,
};
use editor_task_queue::{TaskId, TaskStatus};
use parking_lot::Mutex;

fn wait(id: TaskId) -> (TaskStatus, Option<String>) {
    for _ in 0..600 {
        if let Some(snapshot) = editor_task_queue::global()
            .snapshots()
            .into_iter()
            .find(|snapshot| snapshot.id == id)
        {
            if !matches!(snapshot.status, TaskStatus::Queued | TaskStatus::Running) {
                return (snapshot.status, snapshot.error);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("task did not finish");
}

fn cube() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../../assets/meshes/primitives/SM_Cube.fbx")
}

#[test]
fn link_records_the_source_and_reimport_follows_edits() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let source = root.join("cube.fbx");
    std::fs::copy(cube(), &source).unwrap();
    let options = Arc::new(Mutex::new(OptionValues::new()));

    let ids = submit_import(
        root.clone(),
        vec![source.clone()],
        ImportMode::Link,
        options,
    );
    assert_eq!(wait(ids[0]), (TaskStatus::Succeeded, None));
    assert!(source.exists() && root.join("cube.mesh").exists());
    let db = ImportDb::load(&root);
    let record = db.find_source("cube.fbx").expect("linked");
    assert_eq!(record.asset, "cube.mesh");
    assert_eq!(db.status(&root, record), asset_import::LinkStatus::Current);

    let mut bytes = std::fs::read(&source).unwrap();
    bytes.push(0);
    std::fs::write(&source, bytes).unwrap();
    assert_eq!(db.status(&root, record), asset_import::LinkStatus::OutOfDate);
}

#[test]
fn reimport_keeps_default_materials_and_refreshes_the_record() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let source = root.join("cube.fbx");
    std::fs::copy(cube(), &source).unwrap();
    let options = Arc::new(Mutex::new(OptionValues::new()));
    let ids = submit_import(
        root.clone(),
        vec![source.clone()],
        ImportMode::Link,
        options,
    );
    assert_eq!(wait(ids[0]).0, TaskStatus::Succeeded);

    let asset = root.join("cube.mesh");
    meshes_component::mesh_cache::set_default_materials(&asset, &["materials/Keep.mat".into()])
        .unwrap();
    let mut bytes = std::fs::read(&source).unwrap();
    bytes.push(0);
    std::fs::write(&source, bytes).unwrap();

    let id = submit_reimport(root.clone(), "cube.fbx").expect("linked");
    assert_eq!(wait(id), (TaskStatus::Succeeded, None));
    let (decoded, _) =
        meshes_component::mesh_cache::decode_asset(&std::fs::read(&asset).unwrap()).unwrap();
    assert_eq!(
        decoded.material_slots[0].material_asset,
        "materials/Keep.mat"
    );
    let db = ImportDb::load(&root);
    assert_eq!(
        db.status(&root, db.find_source("cube.fbx").unwrap()),
        asset_import::LinkStatus::Current
    );
}

#[test]
fn convert_in_place_replaces_the_source_and_keeps_no_record() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let source = root.join("cube.fbx");
    std::fs::copy(cube(), &source).unwrap();
    let options = Arc::new(Mutex::new(OptionValues::new()));

    let ids = submit_import(
        root.clone(),
        vec![source.clone()],
        ImportMode::ConvertInPlace,
        options,
    );
    assert_eq!(wait(ids[0]), (TaskStatus::Succeeded, None));
    assert!(!source.exists() && root.join("cube.mesh").exists());
    assert!(ImportDb::load(&root).records.is_empty());

    // The source is recoverable from the trash, byte for byte.
    let trashed: Vec<_> = std::fs::read_dir(root.join(TRASH_DIR))
        .unwrap()
        .map(|entry| entry.unwrap().path().join("cube.fbx"))
        .collect();
    assert_eq!(trashed.len(), 1);
    assert_eq!(
        std::fs::read(&trashed[0]).unwrap(),
        std::fs::read(cube()).unwrap()
    );
}
#[test]
fn unlink_keeps_both_files_and_unignore_offers_the_source_again() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let source = root.join("cube.fbx");
    std::fs::copy(cube(), &source).unwrap();
    let options = Arc::new(Mutex::new(OptionValues::new()));
    let ids = submit_import(
        root.clone(),
        vec![source.clone()],
        ImportMode::Link,
        options,
    );
    assert_eq!(wait(ids[0]).0, TaskStatus::Succeeded);

    let before = asset_import::db_revision();
    unlink_source(&root, "cube.fbx").unwrap();
    assert!(
        asset_import::db_revision() > before,
        "a database write bumps the revision"
    );
    assert!(source.exists() && root.join("cube.mesh").exists());
    let states = asset_import::import_states(&root, &[source.clone(), root.join("cube.mesh")]);
    assert_eq!(states[&source], asset_import::FileImportState::Unlinked);
    assert!(
        !states.contains_key(&root.join("cube.mesh")),
        "the asset is no longer linked"
    );

    ignore_sources(&root, &[source.clone()]).unwrap();
    assert_eq!(
        asset_import::import_states(&root, &[source.clone()])[&source],
        asset_import::FileImportState::Ignored
    );
    unignore_sources(&root, &[source.clone()]).unwrap();
    assert_eq!(
        asset_import::import_states(&root, &[source.clone()])[&source],
        asset_import::FileImportState::Unlinked
    );
}
