//! A source with several meshes becomes one native mesh per source mesh, not
//! one merged mesh.

use std::sync::Arc;

use asset_import::{ImportDb, ImportMode, OptionValues};
use editor_task_queue::TaskStatus;
use parking_lot::Mutex;

fn wait(id: editor_task_queue::TaskId) -> TaskStatus {
    for _ in 0..600 {
        if let Some(s) = editor_task_queue::global().snapshots().into_iter().find(|s| s.id == id) {
            if !matches!(s.status, TaskStatus::Queued | TaskStatus::Running) {
                return s.status;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("task did not finish");
}

/// Two objects with a triangle each, one at x = 0 and one at x = 10.
const TWO_OBJECTS: &str = "\
o Crate
v 0 0 0
v 1 0 0
v 0 1 0
f 1 2 3
o Barrel
v 10 0 0
v 11 0 0
v 10 1 0
f 4 5 6
";

#[test]
fn each_mesh_of_a_source_gets_its_own_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let source = root.join("props.obj");
    std::fs::write(&source, TWO_OBJECTS).unwrap();

    let ids = asset_import::submit_import_with_companions(
        root.to_path_buf(),
        vec![source],
        ImportMode::Link,
        Arc::new(Mutex::new(OptionValues::new())),
        Vec::new(),
    );
    assert_eq!(wait(ids[0]), TaskStatus::Succeeded);

    let record = ImportDb::load(root).find_source("props.obj").unwrap().clone();
    let mut meshes: Vec<String> = std::iter::once(record.asset.clone())
        .chain(record.outputs.iter().filter(|o| o.kind == "mesh").map(|o| o.path.clone()))
        .collect();
    meshes.sort();
    assert_eq!(meshes, ["props_Barrel.mesh", "props_Crate.mesh"], "{record:?}");
    for mesh in &meshes {
        assert!(root.join(mesh).exists(), "{mesh} was written");
    }
    assert!(!root.join("props.mesh").exists(), "no merged mesh");

    // Each file holds only its own geometry, with the node transform baked in.
    let read = |name: &str| {
        let bytes = std::fs::read(root.join(name)).unwrap();
        helio_component::mesh_cache::decode_asset(&bytes).unwrap().0
    };
    let crate_mesh = read("props_Crate.mesh");
    let barrel = read("props_Barrel.mesh");
    assert_eq!(crate_mesh.geometry.vertices.len(), 3);
    assert_eq!(barrel.geometry.vertices.len(), 3);
    let min_x = |m: &helio_component::mesh_cache::MeshAssetUpload| {
        m.geometry.vertices.iter().map(|v| v.position[0]).fold(f32::MAX, f32::min)
    };
    assert!(min_x(&crate_mesh) < 5.0 && min_x(&barrel) > 5.0, "geometry kept in place");
}
