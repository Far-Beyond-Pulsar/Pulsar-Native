//! Import companions run after the native asset exists, record what they
//! wrote, and never replace a material the user already assigned. The
//! companion here is a stub: the pipeline must not need to know what a
//! companion makes.

use std::path::Path;
use std::sync::Arc;

use asset_import::{ImportDb, ImportMode, OptionValues};
use editor_task_queue::TaskStatus;
use parking_lot::Mutex;
use plugin_editor_api::{
    CompanionOutput, ImportCompanion, ImportCompanionContext, ImportOffer, ImportProbe,
};

struct Stub;

impl ImportCompanion for Stub {
    fn id(&self) -> &'static str {
        "test.stub"
    }
    fn offer(&self, probe: &ImportProbe) -> Option<ImportOffer> {
        (probe.importer == "mesh").then(|| ImportOffer {
            label: "Stub".into(),
            description: String::new(),
            default_enabled: true,
        })
    }
    fn run(&self, context: &ImportCompanionContext) -> Result<Vec<CompanionOutput>, String> {
        let made = context.project_root.join("made.thing");
        std::fs::write(&made, b"x").map_err(|e| e.to_string())?;
        Ok(vec![CompanionOutput {
            path: made,
            kind: "thing".into(),
            source_name: None,
            assign_to_slot: Some(0),
        }])
    }
}

fn wait(id: editor_task_queue::TaskId) -> TaskStatus {
    for _ in 0..600 {
        if let Some(s) = editor_task_queue::global()
            .snapshots()
            .into_iter()
            .find(|s| s.id == id)
        {
            if !matches!(s.status, TaskStatus::Queued | TaskStatus::Running) {
                return s.status;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("task did not finish");
}

#[test]
fn accepted_companions_run_record_outputs_and_fill_only_empty_slots() {
    asset_import::register_companion(Arc::new(Stub));
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let source = root.join("cube.fbx");
    std::fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../assets/meshes/primitives/SM_Cube.fbx"),
        &source,
    )
    .unwrap();

    let offers = asset_import::offers_for("mesh", &[source.clone()]);
    assert!(offers.iter().any(|o| o.companion_id == "test.stub"));

    let ids = asset_import::submit_import_with_companions(
        root.to_path_buf(),
        vec![source],
        ImportMode::Link,
        Arc::new(Mutex::new(OptionValues::new())),
        vec!["test.stub".into()],
    );
    assert_eq!(wait(ids[0]), TaskStatus::Succeeded);

    let db = ImportDb::load(root);
    let record = db.find_source("cube.fbx").unwrap();
    assert_eq!(record.companions, ["test.stub"]);
    assert!(record
        .outputs
        .iter()
        .any(|o| o.path == "made.thing" && o.kind == "thing"));
    let mesh = std::fs::read(root.join("cube.mesh")).unwrap();
    let (decoded, _) = helio_component::mesh_cache::decode_asset(&mesh).unwrap();
    assert_eq!(decoded.material_slots[0].material_asset, "made.thing");

    // The user reassigns the slot; a reimport (which re-runs the companion)
    // leaves their choice alone.
    helio_component::mesh_cache::set_default_materials(
        &root.join("cube.mesh"),
        &["mine.mat".into()],
    )
    .unwrap();
    let mut bytes = std::fs::read(root.join("cube.fbx")).unwrap();
    bytes.push(0);
    std::fs::write(root.join("cube.fbx"), bytes).unwrap();
    let id = asset_import::submit_reimport(root.to_path_buf(), "cube.fbx").unwrap();
    assert_eq!(wait(id), TaskStatus::Succeeded);
    let (decoded, _) =
        helio_component::mesh_cache::decode_asset(&std::fs::read(root.join("cube.mesh")).unwrap())
            .unwrap();
    assert_eq!(decoded.material_slots[0].material_asset, "mine.mat");
}
