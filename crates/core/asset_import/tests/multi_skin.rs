use std::path::Path;

use asset_import::{importer_for, ImportOutput, Imported, OptionValues, Previous};
use skeletal_assets::animation::evaluate_world_transforms;
use skeletal_assets::assets::{load_animseq_asset, load_skeleton_asset};
use skeletal_assets::skinning::{joint_matrices, skin_lod};
use skeletal_assets::skmesh::{load_skmesh_asset, save_skmesh_asset, validate_mesh};

const FIXTURE: &str = include_str!("fixtures/multi_skin.gltf");

fn import(root: &Path, document: &str, previous: &[ImportOutput]) -> Imported {
    let source = root.join("multi.gltf");
    std::fs::write(&source, document).unwrap();
    importer_for("gltf")
        .unwrap()
        .import(
            &source,
            &root.join("multi.mesh"),
            &OptionValues::new(),
            &Previous {
                project_root: root,
                outputs: previous,
            },
        )
        .unwrap()
}

fn output<'a>(imported: &'a Imported, kind: &str, skin: usize) -> &'a ImportOutput {
    imported
        .outputs
        .iter()
        .find(|output| output.kind == kind && output.source_skin == Some(skin))
        .unwrap()
}

#[test]
fn each_skin_gets_its_own_assets_joint_palette_and_animation_bindings() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let imported = import(root, FIXTURE, &[]);
    assert_eq!(
        imported.outputs.len(),
        7,
        "two rig sets and one unskinned instance"
    );
    assert_eq!(imported.asset, root.join("multi.skmesh"));
    let mut skeleton_ids = Vec::new();
    let mut mesh_ids = Vec::new();
    for skin in 0..2 {
        let skeleton =
            load_skeleton_asset(&root.join(&output(&imported, "skeleton", skin).path)).unwrap();
        let mesh_path = root.join(&output(&imported, "skmesh", skin).path);
        let mesh = load_skmesh_asset(&mesh_path).unwrap();
        let sequence =
            load_animseq_asset(&root.join(&output(&imported, "animseq", skin).path)).unwrap();
        assert_eq!(mesh.skeleton.as_ref(), Some(&skeleton.id));
        assert_eq!(sequence.skeleton.as_ref(), Some(&skeleton.id));
        assert_eq!(skeleton.preview_mesh.as_ref(), Some(&mesh.id));
        assert_eq!(sequence.preview_mesh.as_ref(), Some(&mesh.id));
        assert!(validate_mesh(&mesh, Some(&skeleton.skeleton())).is_empty());
        assert!(sequence.clip.orphan_tracks(&skeleton.skeleton()).is_empty());
        assert_eq!(mesh.joints.len(), 9);
        assert_eq!(mesh.lods.len(), 1);
        let lod = &mesh.lods[0];
        assert_eq!(
            lod.vertices.len(),
            6,
            "same-skin node instances share geometry"
        );
        assert_eq!(lod.indices, [0, 1, 2, 3, 4, 5]);
        assert_eq!(lod.sections.len(), 2);
        assert_eq!(
            mesh.material_slots
                .iter()
                .map(|slot| slot.name.as_str())
                .collect::<Vec<_>>(),
            ["Cloth", "Trim"]
        );
        for primitive in lod.vertices.as_chunks::<3>().0 {
            assert_eq!(primitive[0].joints, [8, 6, 5, 4]);
            for (weight, expected) in primitive[0]
                .weights
                .iter()
                .zip([0.625, 0.15625, 0.125, 0.09375])
            {
                assert!((weight - expected).abs() < 1e-6);
            }
            assert_eq!(primitive[1].joints, [0, 1, 2, 3]);
            assert_eq!(primitive[1].weights, [0.25; 4]);
            assert_eq!(primitive[2].weights, [0.75, 0.25, 0.0, 0.0]);
            for vertex in primitive {
                assert!((vertex.weights.iter().sum::<f32>() - 1.0).abs() < 1e-6);
            }
        }
        assert_eq!(mesh.joints[8].bone, if skin == 0 { "A8" } else { "A0" });
        assert_eq!(
            mesh.joints[0].inverse_bind[12],
            if skin == 0 { 0.0 } else { -20.0 }
        );
        assert_eq!(sequence.clip.tracks[0].bone_id, "A0");
        let world = evaluate_world_transforms(&skeleton.skeleton(), &sequence.clip, 1.0);
        let posed = skin_lod(lod, &joint_matrices(&mesh, &world));
        let expected = if skin == 0 { [0.0, 1.0] } else { [6.25, 0.625] };
        assert!((posed[0].position.x - expected[0]).abs() < 1e-5);
        assert!((posed[0].position.y - expected[1]).abs() < 1e-5);
        let round_trip = root.join(format!("roundtrip_{skin}.skmesh"));
        save_skmesh_asset(&round_trip, &mesh).unwrap();
        assert_eq!(
            std::fs::read(round_trip).unwrap(),
            std::fs::read(mesh_path).unwrap()
        );
        skeleton_ids.push(skeleton.id);
        mesh_ids.push(mesh.id);
    }
    assert_ne!(skeleton_ids[0], skeleton_ids[1]);
    assert_ne!(mesh_ids[0], mesh_ids[1]);
    let static_output = imported
        .outputs
        .iter()
        .find(|output| output.kind == "mesh")
        .unwrap();
    assert!(static_output.source_skin.is_none());
    let bytes = std::fs::read(root.join(&static_output.path)).unwrap();
    let (plain, _) = helio_component::mesh_cache::decode_asset(&bytes).unwrap();
    let positions: Vec<_> = plain
        .geometry
        .indices
        .iter()
        .map(|&index| plain.geometry.vertices[index as usize].position)
        .collect();
    assert_eq!(
        positions,
        [
            [5.0, 0.0, 0.0],
            [6.0, 0.0, 0.0],
            [5.0, 1.0, 0.0],
            [5.0, 0.0, 0.0],
            [6.0, 0.0, 0.0],
            [5.0, 1.0, 0.0]
        ]
    );
    assert_eq!(plain.sections.len(), 2);
    assert_eq!(imported.parts.len(), 1);
}

#[test]
fn reimport_keeps_per_skin_ids_edits_moved_files_and_deleted_outputs() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let first = import(root, FIXTURE, &[]);
    let mut before = first.outputs.clone();
    let b_skeleton = root.join(&output(&first, "skeleton", 1).path);
    let moved = root.join("renamed.skeleton");
    std::fs::rename(&b_skeleton, &moved).unwrap();
    let b_mesh = root.join(&output(&first, "skmesh", 1).path);
    let mut mesh = load_skmesh_asset(&b_mesh).unwrap();
    mesh.material_slots[0].material = Some("materials/custom.mat".into());
    save_skmesh_asset(&b_mesh, &mesh).unwrap();
    let b_sequence = output(&first, "animseq", 1);
    std::fs::remove_file(root.join(&b_sequence.path)).unwrap();
    let second = import(root, FIXTURE, &before);
    before
        .iter_mut()
        .find(|output| output.kind == "skeleton" && output.source_skin == Some(1))
        .unwrap()
        .path = "renamed.skeleton".into();
    assert_eq!(second.outputs, before);
    assert_eq!(
        load_skmesh_asset(&b_mesh).unwrap().material_slots[0]
            .material
            .as_deref(),
        Some("materials/custom.mat")
    );
    let encoded = serde_json::to_string(&second.outputs).unwrap();
    let decoded: Vec<ImportOutput> = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, before);
    assert_eq!(import(root, FIXTURE, &decoded).outputs, decoded);
}

#[test]
fn adding_a_skin_migrates_legacy_single_rig_records_without_changing_ids() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut single: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    single["nodes"][1].as_object_mut().unwrap().remove("skin");
    let first = import(root, &single.to_string(), &[]);
    let mut legacy = first.outputs.clone();
    for output in &mut legacy {
        output.source_skin = None;
    }
    let multi = import(root, FIXTURE, &legacy);
    for kind in ["skeleton", "skmesh", "animseq"] {
        let old = first
            .outputs
            .iter()
            .find(|output| output.kind == kind && output.source_skin == Some(0))
            .unwrap();
        let new = output(&multi, kind, 0);
        assert_eq!(new.id, old.id);
        assert_eq!(new.path, old.path);
    }
    let without_skin_field: Vec<ImportOutput> =
        serde_json::from_str(r#"[{"path":"old.skmesh","kind":"skmesh"}]"#).unwrap();
    assert!(without_skin_field[0].source_skin.is_none());
}

#[test]
fn changing_node_order_does_not_exchange_skin_identities() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let first = import(root, FIXTURE, &[]);
    let mut document: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    document["scenes"][0]["nodes"] = serde_json::json!([1, 0, 2, 20, 21]);
    let second = import(root, &document.to_string(), &first.outputs);
    for skin in 0..2 {
        for kind in ["skeleton", "skmesh", "animseq"] {
            assert_eq!(output(&first, kind, skin), output(&second, kind, skin));
        }
    }
}

#[test]
fn distinct_meshes_and_zero_weight_fallback_keep_each_skin_palette() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut document: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    let mut other = document["meshes"][0].clone();
    other["name"] = serde_json::json!("Other");
    document["meshes"].as_array_mut().unwrap().push(other);
    document["nodes"][1]["mesh"] = serde_json::json!(1);
    let imported = import(root, &document.to_string(), &[]);
    for skin in 0..2 {
        let mesh = load_skmesh_asset(&root.join(&output(&imported, "skmesh", skin).path)).unwrap();
        assert_eq!(mesh.lods[0].vertices.len(), 6);
        assert_eq!(mesh.joints[0].bone, if skin == 0 { "A0" } else { "B0" });
    }
    for set in 0..3 {
        let index = document["meshes"][0]["primitives"][0]["attributes"][format!("WEIGHTS_{set}")]
            .as_u64()
            .unwrap() as usize;
        document["accessors"][index]
            .as_object_mut()
            .unwrap()
            .remove("bufferView");
    }
    let reimported = import(root, &document.to_string(), &imported.outputs);
    for skin in 0..2 {
        let mesh =
            load_skmesh_asset(&root.join(&output(&reimported, "skmesh", skin).path)).unwrap();
        for vertex in &mesh.lods[0].vertices {
            assert_eq!(vertex.joints, [0; 4]);
            assert_eq!(vertex.weights, [1.0, 0.0, 0.0, 0.0]);
        }
    }
}

#[test]
fn retained_legacy_clips_cannot_be_claimed_by_another_skin() {
    use skeletal_assets::assets::{save_animseq_asset, AnimSeqAsset};
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut single: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    single["nodes"][1].as_object_mut().unwrap().remove("skin");
    let first = import(root, &single.to_string(), &[]);
    let sequence = load_animseq_asset(&root.join(&output(&first, "animseq", 0).path)).unwrap();
    let mut lost = AnimSeqAsset::new(sequence.skeleton.clone(), sequence.clip.clone());
    lost.clip.name = "Lost".into();
    save_animseq_asset(&root.join("lost.animseq"), &lost).unwrap();
    let mut legacy = first.outputs;
    legacy.push(ImportOutput {
        path: "lost.animseq".into(),
        kind: "animseq".into(),
        id: Some(lost.id.to_string()),
        source_name: Some("Lost".into()),
        source_skin: None,
    });
    for output in &mut legacy {
        output.source_skin = None;
    }
    let migrated = import(root, FIXTURE, &legacy);
    let retained = migrated
        .outputs
        .iter()
        .find(|output| output.id.as_deref() == Some(lost.id.as_str()))
        .unwrap();
    assert_eq!(retained.source_skin, Some(0));
    let mut other: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    for node in [0, 20] {
        other["nodes"][node].as_object_mut().unwrap().remove("skin");
    }
    other["animations"][0]["name"] = serde_json::json!("Lost");
    let imported = import(root, &other.to_string(), &migrated.outputs);
    let new_clip = output(&imported, "animseq", 1);
    assert_ne!(new_clip.id.as_deref(), Some(lost.id.as_str()));
    assert_eq!(
        load_animseq_asset(&root.join("lost.animseq"))
            .unwrap()
            .skeleton,
        lost.skeleton
    );
}

#[test]
fn mixed_static_parts_with_duplicate_names_keep_distinct_outputs_on_reimport() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let mut document: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    for _ in 0..2 {
        let mut mesh = document["meshes"][0].clone();
        mesh["name"] = serde_json::json!("Same");
        let mesh_index = document["meshes"].as_array().unwrap().len();
        document["meshes"].as_array_mut().unwrap().push(mesh);
        let node = document["nodes"].as_array().unwrap().len();
        document["nodes"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"mesh":mesh_index}));
        document["scenes"][0]["nodes"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!(node));
    }
    let first = import(root, &document.to_string(), &[]);
    assert_eq!(
        first
            .outputs
            .iter()
            .filter(|output| output.kind == "mesh")
            .count(),
        3
    );
    let second = import(root, &document.to_string(), &first.outputs);
    assert_eq!(second.outputs, first.outputs);
}
