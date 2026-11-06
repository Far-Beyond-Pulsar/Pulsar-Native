//! Import of a rigged glTF into the linked skeletal assets. The model is built
//! here, byte by byte, so no binary fixture is committed.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use asset_import::{importer_for, ImportDb, ImportMode, Imported, OptionValues, Previous};
use editor_task_queue::{TaskId, TaskStatus};
use parking_lot::Mutex;
use skeletal_assets::animation::{evaluate_world_transforms, AnimationClip};
use skeletal_assets::assets::{
    load_animseq_asset, load_skeleton_asset, AnimSeqAsset, SkeletonAsset,
};
use skeletal_assets::math::Vec3;
use skeletal_assets::skinning::{joint_matrices, skin_lod};
use skeletal_assets::skmesh::{
    load_skmesh_asset, save_skmesh_asset, validate_mesh, SkeletalMeshAsset,
};

// ── The model ──────────────────────────────────────────────────────────────

fn f32s(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn u16s(values: &[u16]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | (*b as u32) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Two bones, `Root` and `Arm` (one unit along +X from it), and a four-vertex
/// strip skinned to them:
///
/// - vertex 0 (0,1,0): Root
/// - vertex 1 (1,0,0): Arm, at the arm's own origin
/// - vertex 2 (1,1,0): 25% Root, 75% Arm
/// - vertex 3 (2,0,0): Arm
///
/// Two animations: `Wave` turns the Arm 90 degrees about Z, `Bob` lifts the
/// Root one unit up.
fn rigged_gltf() -> String {
    let half = std::f32::consts::FRAC_PI_4;
    let parts: Vec<Vec<u8>> = vec![
        // 0: positions
        f32s(&[0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 2.0, 0.0, 0.0]),
        // 1: joints
        u16s(&[0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 1, 0, 0, 0]),
        // 2: weights
        f32s(&[
            1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.25, 0.75, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0,
        ]),
        // 3: indices
        u16s(&[0, 1, 2, 2, 1, 3]),
        // 4: inverse bind matrices (Root: identity, Arm: translate -1 in X)
        f32s(&[
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, //
            1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, -1.0, 0.0, 0.0, 1.0,
        ]),
        // 5, 6: Wave times and rotations
        f32s(&[0.0, 1.0]),
        f32s(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, half.sin(), half.cos()]),
        // 7, 8: Bob times and translations
        f32s(&[0.0, 0.5]),
        f32s(&[0.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
    ];
    let mut buffer = Vec::new();
    let mut views = Vec::new();
    for part in &parts {
        views.push(format!(
            r#"{{ "buffer": 0, "byteOffset": {}, "byteLength": {} }}"#,
            buffer.len(),
            part.len()
        ));
        buffer.extend_from_slice(part);
    }
    let accessor = |view: usize, component: u32, count: usize, kind: &str, extra: &str| {
        format!(
            r#"{{ "bufferView": {view}, "componentType": {component}, "count": {count}, "type": "{kind}"{extra} }}"#
        )
    };
    let accessors = [
        accessor(
            0,
            5126,
            4,
            "VEC3",
            r#", "min": [0, 0, 0], "max": [2, 1, 0]"#,
        ),
        accessor(1, 5123, 4, "VEC4", ""),
        accessor(2, 5126, 4, "VEC4", ""),
        accessor(3, 5123, 6, "SCALAR", ""),
        accessor(4, 5126, 2, "MAT4", ""),
        accessor(5, 5126, 2, "SCALAR", r#", "min": [0], "max": [1]"#),
        accessor(6, 5126, 2, "VEC4", ""),
        accessor(7, 5126, 2, "SCALAR", r#", "min": [0], "max": [0.5]"#),
        accessor(8, 5126, 2, "VEC3", ""),
    ];
    format!(
        r#"{{
  "asset": {{ "version": "2.0" }},
  "scene": 0,
  "scenes": [ {{ "nodes": [0, 1] }} ],
  "nodes": [
    {{ "name": "Body", "mesh": 0, "skin": 0 }},
    {{ "name": "Root", "children": [2] }},
    {{ "name": "Arm", "translation": [1, 0, 0] }}
  ],
  "materials": [ {{ "name": "Skin" }} ],
  "meshes": [ {{ "name": "BodyMesh", "primitives": [ {{
    "attributes": {{ "POSITION": 0, "JOINTS_0": 1, "WEIGHTS_0": 2 }}, "indices": 3, "material": 0 }} ] }} ],
  "skins": [ {{ "joints": [1, 2], "inverseBindMatrices": 4 }} ],
  "animations": [
    {{ "name": "Wave",
       "channels": [ {{ "sampler": 0, "target": {{ "node": 2, "path": "rotation" }} }} ],
       "samplers": [ {{ "input": 5, "output": 6, "interpolation": "LINEAR" }} ] }},
    {{ "name": "Bob",
       "channels": [ {{ "sampler": 0, "target": {{ "node": 1, "path": "translation" }} }} ],
       "samplers": [ {{ "input": 7, "output": 8, "interpolation": "LINEAR" }} ] }}
  ],
  "buffers": [ {{ "byteLength": {}, "uri": "data:application/octet-stream;base64,{}" }} ],
  "bufferViews": [ {} ],
  "accessors": [ {} ]
}}"#,
        buffer.len(),
        base64(&buffer),
        views.join(", "),
        accessors.join(", ")
    )
}

// ── Helpers ────────────────────────────────────────────────────────────────

struct Project {
    dir: tempfile::TempDir,
}

impl Project {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }

    fn rig_source(&self) -> PathBuf {
        let source = self.root().join("rig.gltf");
        std::fs::write(&source, rigged_gltf()).unwrap();
        source
    }

    fn import(&self, previous: &[asset_import::ImportOutput]) -> Imported {
        let source = self.rig_source();
        importer_for("gltf")
            .expect("a gltf importer")
            .import(
                &source,
                &self.root().join("rig.mesh"),
                &OptionValues::new(),
                &Previous {
                    project_root: self.root(),
                    outputs: previous,
                },
            )
            .expect("the rigged model imports")
    }
}

fn path_of(imported: &Imported, root: &Path, kind: &str) -> PathBuf {
    let output = imported
        .outputs
        .iter()
        .find(|o| o.kind == kind)
        .unwrap_or_else(|| panic!("no {kind} output"));
    root.join(&output.path)
}

fn near(a: Vec3, b: Vec3) {
    assert!(a.sub(b).length() < 1e-4, "{a:?} != {b:?}");
}

fn wait(id: TaskId) -> (TaskStatus, Option<String>) {
    for _ in 0..1200 {
        if let Some(snapshot) = editor_task_queue::global()
            .snapshots()
            .into_iter()
            .find(|s| s.id == id)
        {
            if !matches!(snapshot.status, TaskStatus::Queued | TaskStatus::Running) {
                return (snapshot.status, snapshot.error);
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    panic!("task did not finish");
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[test]
fn a_rigged_model_imports_to_a_skeleton_a_mesh_and_animations_linked_by_id() {
    let project = Project::new();
    let imported = project.import(&[]);
    let root = project.root();

    assert_eq!(
        imported.asset,
        root.join("rig.skmesh"),
        "the skeletal mesh is the primary asset"
    );
    assert!(
        !root.join("rig.mesh").exists(),
        "no static mesh is written for a rigged model"
    );
    let kinds: Vec<&str> = imported.outputs.iter().map(|o| o.kind.as_str()).collect();
    assert_eq!(kinds, ["skeleton", "skmesh", "animseq", "animseq"]);

    // Skeleton: the two joints, with the arm one unit from the root.
    let skeleton = load_skeleton_asset(&path_of(&imported, root, "skeleton")).unwrap();
    let ids: Vec<&str> = skeleton.bones.iter().map(|b| b.id.as_str()).collect();
    assert_eq!(ids, ["Root", "Arm"]);
    assert_eq!(skeleton.bones[1].parent.as_deref(), Some("Root"));
    near(
        skeleton.bones[1].bind_transform.translation,
        Vec3::new(1.0, 0.0, 0.0),
    );
    assert_eq!(
        imported.outputs[0].id.as_deref(),
        Some(skeleton.id.as_str())
    );

    // Skeletal mesh: refers to that skeleton by id, and is consistent with it.
    let mesh = load_skmesh_asset(&imported.asset).unwrap();
    assert_eq!(mesh.skeleton, Some(skeleton.id.clone()));
    assert_eq!(imported.outputs[1].id.as_deref(), Some(mesh.id.as_str()));
    let joint_bones: Vec<&str> = mesh.joints.iter().map(|j| j.bone.as_str()).collect();
    assert_eq!(joint_bones, ["Root", "Arm"]);
    assert_eq!(mesh.joints[1].inverse_bind[12], -1.0);
    assert!(validate_mesh(&mesh, Some(&skeleton.skeleton())).is_empty());
    let stats = mesh.stats();
    assert_eq!(
        (stats.vertices, stats.triangles, stats.max_influences),
        (4, 2, 2)
    );
    assert_eq!(mesh.material_slots.len(), 1);
    assert_eq!(mesh.material_slots[0].name, "Skin");
    assert_eq!(mesh.lods[0].sections.len(), 1);

    // The editors show the mesh with the skeleton and its animations.
    assert_eq!(skeleton.preview_mesh, Some(mesh.id.clone()));

    // Skinned at the bind pose, every vertex is where the source put it.
    let bind = AnimationClip {
        name: "Bind".into(),
        duration: 1.0,
        fps: 30.0,
        tracks: Vec::new(),
    };
    let world = evaluate_world_transforms(&skeleton.skeleton(), &bind, 0.0);
    let posed = skin_lod(&mesh.lods[0], &joint_matrices(&mesh, &world));
    for (posed, vertex) in posed.iter().zip(&mesh.lods[0].vertices) {
        near(
            posed.position,
            Vec3::new(vertex.position[0], vertex.position[1], vertex.position[2]),
        );
    }
    near(posed[2].position, Vec3::new(1.0, 1.0, 0.0));

    // Each animation refers to the skeleton and previews the mesh.
    let sequences: Vec<AnimSeqAsset> = imported
        .outputs
        .iter()
        .filter(|o| o.kind == "animseq")
        .map(|o| load_animseq_asset(&root.join(&o.path)).unwrap())
        .collect();
    let names: Vec<&str> = sequences.iter().map(|s| s.clip.name.as_str()).collect();
    assert_eq!(names, ["Wave", "Bob"]);
    for sequence in &sequences {
        assert_eq!(sequence.skeleton, Some(skeleton.id.clone()));
        assert_eq!(sequence.preview_mesh, Some(mesh.id.clone()));
        assert!(sequence.clip.orphan_tracks(&skeleton.skeleton()).is_empty());
    }
    assert_eq!(imported.outputs[2].source_name.as_deref(), Some("Wave"));

    // At the end of Wave the arm is turned a quarter: the vertex at the arm's
    // origin stays, the one a unit further along +X swings up to (1, 1, 0).
    let wave = &sequences[0].clip;
    assert_eq!(wave.tracks.len(), 1);
    assert_eq!(wave.tracks[0].bone_id, "Arm");
    assert!((wave.duration - 1.0).abs() < 1e-6);
    let world = evaluate_world_transforms(&skeleton.skeleton(), wave, 1.0);
    let posed = skin_lod(&mesh.lods[0], &joint_matrices(&mesh, &world));
    near(posed[1].position, Vec3::new(1.0, 0.0, 0.0));
    near(posed[3].position, Vec3::new(1.0, 1.0, 0.0));
    near(posed[0].position, Vec3::new(0.0, 1.0, 0.0));

    // Bob lifts the root, and with it everything.
    let bob = &sequences[1].clip;
    let world = evaluate_world_transforms(&skeleton.skeleton(), bob, 0.5);
    let posed = skin_lod(&mesh.lods[0], &joint_matrices(&mesh, &world));
    near(posed[3].position, Vec3::new(2.0, 1.0, 0.0));
}

#[test]
fn reimporting_keeps_every_id_and_the_edits_made_since() {
    let project = Project::new();
    let root = project.root();
    let first = project.import(&[]);
    let skeleton_path = path_of(&first, root, "skeleton");
    let skeleton = load_skeleton_asset(&skeleton_path).unwrap();
    let mesh = load_skmesh_asset(&first.asset).unwrap();

    // The user assigns a material and picks another preview mesh.
    let mut edited = mesh.clone();
    edited.material_slots[0].material = Some("materials/Skin.mat".into());
    save_skmesh_asset(&first.asset, &edited).unwrap();
    let mut edited_skeleton = skeleton.clone();
    let other = skeletal_assets::ids::SkeletalMeshId::new();
    edited_skeleton.preview_mesh = Some(other.clone());
    skeletal_assets::assets::save_skeleton_asset(&skeleton_path, &edited_skeleton).unwrap();

    let second = project.import(&first.outputs);
    assert_eq!(second.outputs, first.outputs, "same files, same ids");
    let again = load_skmesh_asset(&second.asset).unwrap();
    assert_eq!(again.id, mesh.id);
    assert_eq!(again.skeleton, Some(skeleton.id.clone()));
    assert_eq!(
        again.material_slots[0].material.as_deref(),
        Some("materials/Skin.mat")
    );
    let skeleton_again = load_skeleton_asset(&skeleton_path).unwrap();
    assert_eq!(skeleton_again.id, skeleton.id);
    assert_eq!(skeleton_again.preview_mesh, Some(other));
    // No second copy of anything. `.pulsar` holds the project's file index,
    // not assets.
    let mut files: Vec<String> = std::fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != ".pulsar")
        .collect();
    files.sort();
    assert_eq!(
        files.len(),
        1 + 4,
        "the source plus four outputs: {files:?}"
    );
}

#[test]
fn a_moved_output_is_found_by_its_id_and_a_deleted_one_comes_back_with_it() {
    let project = Project::new();
    let root = project.root();
    let first = project.import(&[]);
    let skeleton_id = load_skeleton_asset(&path_of(&first, root, "skeleton"))
        .unwrap()
        .id;
    let mesh_id = load_skmesh_asset(&first.asset).unwrap().id;

    // Moved and renamed in the file manager.
    std::fs::create_dir_all(root.join("shared")).unwrap();
    let moved = root.join("shared").join("Biped.skeleton");
    std::fs::rename(path_of(&first, root, "skeleton"), &moved).unwrap();
    // The mesh is deleted.
    std::fs::remove_file(&first.asset).unwrap();

    let second = project.import(&first.outputs);
    assert_eq!(
        path_of(&second, root, "skeleton"),
        moved,
        "rewritten where it now is"
    );
    assert!(
        !root.join("rig.skeleton").exists(),
        "not recreated beside the moved one"
    );
    assert_eq!(load_skeleton_asset(&moved).unwrap().id, skeleton_id);
    let mesh = load_skmesh_asset(&second.asset).unwrap();
    assert_eq!(mesh.id, mesh_id, "recreated with the id it had");
    assert_eq!(mesh.skeleton, Some(skeleton_id));
}

#[test]
fn a_new_import_never_overwrites_an_unrelated_file() {
    let project = Project::new();
    let root = project.root();
    std::fs::write(root.join("rig.skeleton"), "mine").unwrap();
    let imported = project.import(&[]);
    assert_eq!(
        std::fs::read_to_string(root.join("rig.skeleton")).unwrap(),
        "mine"
    );
    assert_eq!(
        path_of(&imported, root, "skeleton"),
        root.join("rig 2.skeleton")
    );
    // The mesh still refers to the skeleton that was written.
    let skeleton =
        SkeletonAsset::parse(&std::fs::read_to_string(root.join("rig 2.skeleton")).unwrap())
            .unwrap();
    assert_eq!(
        load_skmesh_asset(&imported.asset).unwrap().skeleton,
        Some(skeleton.id)
    );
}

#[test]
fn an_animation_the_source_lost_stays_on_disk_and_in_the_record() {
    let project = Project::new();
    let root = project.root();
    let first = project.import(&[]);
    let mut earlier = first.outputs.clone();
    // Pretend the previous import also had an animation the source no longer has.
    let gone = root.join("rig_Gone.animseq");
    std::fs::copy(path_of(&first, root, "animseq"), &gone).unwrap();
    earlier.push(asset_import::ImportOutput {
        path: "rig_Gone.animseq".into(),
        kind: "animseq".into(),
        id: None,
        source_name: Some("Gone".into()),
        source_skin: None,
    });
    let second = project.import(&earlier);
    assert!(gone.exists());
    assert!(second
        .outputs
        .iter()
        .any(|o| o.source_name.as_deref() == Some("Gone")));
}

#[test]
fn linked_import_records_every_output_and_reimport_keeps_the_ids() {
    let project = Project::new();
    let root = project.root().to_path_buf();
    let source = project.rig_source();
    let options = Arc::new(Mutex::new(OptionValues::new()));

    let ids = asset_import::submit_import(root.clone(), vec![source], ImportMode::Link, options);
    assert_eq!(wait(ids[0]), (TaskStatus::Succeeded, None));
    let db = ImportDb::load(&root);
    let record = db.find_source("rig.gltf").expect("linked");
    assert_eq!(record.asset, "rig.skmesh");
    let paths: Vec<&str> = record.outputs.iter().map(|o| o.path.as_str()).collect();
    assert_eq!(
        paths,
        [
            "rig.skeleton",
            "rig.skmesh",
            "rig_Wave.animseq",
            "rig_Bob.animseq"
        ]
    );
    assert!(
        db.find_asset("rig.skeleton").is_some(),
        "every output is linked to the source"
    );
    assert!(db.find_asset("rig_Bob.animseq").is_some());
    let before = record.outputs.clone();

    let reimport = asset_import::submit_reimport(root.clone(), "rig.gltf").expect("linked");
    assert_eq!(wait(reimport), (TaskStatus::Succeeded, None));
    let db = ImportDb::load(&root);
    let record = db.find_source("rig.gltf").unwrap();
    assert_eq!(record.outputs, before, "the same ids at the same paths");
    assert_eq!(record.asset, "rig.skmesh");
}

#[test]
fn a_static_model_still_imports_to_a_plain_mesh_with_no_extra_outputs() {
    let project = Project::new();
    let root = project.root().to_path_buf();
    let source = root.join("cube.fbx");
    let cube =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../assets/meshes/primitives/SM_Cube.fbx");
    std::fs::copy(cube, &source).unwrap();
    let options = Arc::new(Mutex::new(OptionValues::new()));

    let ids = asset_import::submit_import(root.clone(), vec![source], ImportMode::Link, options);
    assert_eq!(wait(ids[0]), (TaskStatus::Succeeded, None));
    let db = ImportDb::load(&root);
    let record = db.find_source("cube.fbx").unwrap();
    assert_eq!(record.asset, "cube.mesh");
    assert!(record.outputs.is_empty());
    assert!(root.join("cube.mesh").exists());
    let extras: Vec<_> = std::fs::read_dir(&root)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".skeleton") || n.ends_with(".skmesh") || n.ends_with(".animseq"))
        .collect();
    assert!(extras.is_empty(), "{extras:?}");
    // It is readable as the same kind of file it always was.
    let bytes = std::fs::read(root.join("cube.mesh")).unwrap();
    assert!(helio_component::mesh_cache::decode_asset(&bytes).is_some());
    assert!(SkeletalMeshAsset::peek_id(&bytes).is_none());
}

/// An independent reference: convert endpoint derivatives to Bezier control
/// points and evaluate with de Casteljau in f64 (rather than the runtime's
/// f32 Hermite basis).
fn cubic_reference<const N: usize>(time: f32, times: &[f32], values: &[[f32; N]]) -> [f32; N] {
    if time <= times[0] {
        return values[1];
    }
    if time >= times[times.len() - 1] {
        return values[(times.len() - 1) * 3 + 1];
    }
    let next = times.partition_point(|t| *t <= time);
    let a = next - 1;
    let dt = (times[next] - times[a]) as f64;
    let u = (time - times[a]) as f64 / dt;
    std::array::from_fn(|i| {
        let p0 = values[a * 3 + 1][i] as f64;
        let p1 = p0 + dt * values[a * 3 + 2][i] as f64 / 3.0;
        let p3 = values[next * 3 + 1][i] as f64;
        let p2 = p3 - dt * values[next * 3][i] as f64 / 3.0;
        let lerp = |a: f64, b: f64| a * (1.0 - u) + b * u;
        lerp(
            lerp(lerp(p0, p1), lerp(p1, p2)),
            lerp(lerp(p1, p2), lerp(p2, p3)),
        ) as f32
    })
}

#[test]
fn cubic_gltf_survives_import_save_reopen_sampling_and_skinning() {
    use skeletal_assets::animation::KeyInterpolation;
    use skeletal_assets::assets::save_animseq_asset;
    use skeletal_assets::runtime::clip::CompiledClip;
    use skeletal_assets::runtime::pose::{BoneTransform, RuntimeSkeleton};

    let project = Project::new();
    let source = project.root().join("cubic.gltf");
    std::fs::write(&source, include_str!("fixtures/cubic_spline.gltf")).unwrap();
    let imported = importer_for("gltf")
        .unwrap()
        .import(
            &source,
            &project.root().join("cubic.mesh"),
            &OptionValues::new(),
            &Previous {
                project_root: project.root(),
                outputs: &[],
            },
        )
        .unwrap();
    let skeleton = load_skeleton_asset(&path_of(&imported, project.root(), "skeleton")).unwrap();
    let mesh = load_skmesh_asset(&imported.asset).unwrap();
    let path = path_of(&imported, project.root(), "animseq");
    let sequence = load_animseq_asset(&path).unwrap();
    assert_eq!(sequence.version, 5);
    save_animseq_asset(&path, &sequence).unwrap();
    let sequence = load_animseq_asset(&path).unwrap();
    let root = sequence.clip.track("Root").unwrap();
    let arm = sequence.clip.track("Arm").unwrap();
    assert_eq!(
        root.keyframes.len(),
        8,
        "only the union of authored channel timestamps"
    );
    assert_eq!(arm.keyframes.len(), 3, "bones have independent timing");
    assert_eq!(root.keyframes[1].out_tangent.rotation, [1.0, 0.0, 2.0, 1.0]);
    assert_eq!(root.keyframes[6].in_tangent.rotation, [0.0, 1.0, -1.0, 2.0]);
    assert_eq!(
        root.keyframes[1].rotation_interpolation,
        KeyInterpolation::CubicSpline
    );
    assert_eq!(
        root.keyframes[6].rotation_interpolation,
        KeyInterpolation::Step
    );
    assert_eq!(arm.keyframes[0].scale_interpolation, KeyInterpolation::Step);
    assert_eq!(
        arm.keyframes[0].rotation_interpolation,
        KeyInterpolation::Linear
    );
    let runtime_skeleton =
        RuntimeSkeleton::new(skeleton.id.to_string(), &skeleton.skeleton()).unwrap();
    let compiled = CompiledClip::compile(&sequence, &runtime_skeleton);
    let rotation_values = [
        [0.0; 4],
        [0.0, 0.0, 0.0, 1.0],
        [1.0, 0.0, 2.0, 1.0],
        [0.0, 1.0, -1.0, 2.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0; 4],
    ];
    let translation_values: Vec<[f32; 3]> = [
        [0., 0., 0.],
        [0.1, 0., 0.],
        [0.2, 0., 0.],
        [1., 2., 0.],
        [2., 0., -1.],
    ]
    .into_iter()
    .flat_map(|p| [[1., -1., 2.], p, [2., 1., -1.]])
    .collect();
    let scale_values: Vec<[f32; 3]> = [[1., 1., 1.], [0., 2., -1.], [-1., 1., 3.]]
        .into_iter()
        .flat_map(|p| [[-1., 1., 0.], p, [1., 0., -2.]])
        .collect();
    // Check exact endpoints, asynchronous knots, the sub-microsecond interval,
    // and the samples produced at several playback frame rates.
    let mut times = vec![0.0, 0.25, 0.5, 0.5000001, 0.50000024, 1.0, 1.5, 2.0, 3.0];
    for fps in [24, 60, 144] {
        times.extend((0..=3 * fps).map(|f| f as f32 / fps as f32));
    }
    for time in times {
        let translation =
            cubic_reference(time, &[0.0, 0.5, 0.50000024, 1.0, 3.0], &translation_values);
        let scale = cubic_reference(time, &[0.5, 1.5, 3.0], &scale_values);
        let q = glam::Quat::from_array(cubic_reference(time, &[0.25, 2.0], &rotation_values))
            .normalize();
        let rt = compiled.sample_bone(0, time, BoneTransform::IDENTITY);
        assert!(
            rt.rotation.dot(q) > 1.0 - 2e-6,
            "rotation at {time}: {:?} != {q:?}",
            rt.rotation
        );
        assert!((rt.rotation.length() - 1.0).abs() < 2e-6);
        assert!(
            rt.translation.distance(glam::Vec3::from_array(translation)) < 2e-5,
            "translation at {time}"
        );
        assert!(
            rt.scale.distance(glam::Vec3::from_array(scale)) < 2e-5,
            "scale at {time}: {:?} != {scale:?}",
            rt.scale
        );
        let pose = root.sample(time).unwrap();
        near(
            pose.translation,
            Vec3::new(translation[0], translation[1], translation[2]),
        );
        near(pose.scale, Vec3::new(scale[0], scale[1], scale[2]));
        let root_matrix = glam::Mat4::from_scale_rotation_translation(
            glam::Vec3::from_array(scale),
            q,
            glam::Vec3::from_array(translation),
        );
        assert!(pose.to_matrix().approx_eq(
            &skeletal_assets::math::Mat4(root_matrix.to_cols_array()),
            2e-5
        ));
        // Vertex zero is attached entirely to Root. This exercises the real
        // asset pose -> joint palette -> skinned geometry path.
        let world = evaluate_world_transforms(&skeleton.skeleton(), &sequence.clip, time);
        let posed = skin_lod(&mesh.lods[0], &joint_matrices(&mesh, &world));
        let expected =
            root_matrix.transform_point3(glam::Vec3::from_array(mesh.lods[0].vertices[0].position));
        near(
            posed[0].position,
            Vec3::new(expected.x, expected.y, expected.z),
        );
        let arm_pose = compiled.sample_bone(1, time, BoneTransform::IDENTITY);
        let arm_scale = if time < 1.0 {
            1.0
        } else if time < 3.0 {
            2.0
        } else {
            0.5
        };
        assert_eq!(arm_pose.scale, glam::Vec3::splat(arm_scale));
        let arm_translation = cubic_reference(
            time,
            &[0.0, 3.0],
            &[
                [0.0; 3],
                [1.0, 0.0, 0.0],
                [2.0, 1.0, 0.0],
                [-1.0, 2.0, 1.0],
                [2.0, 1.0, 0.0],
                [0.0; 3],
            ],
        );
        let arm_rotation = glam::Quat::from_rotation_z(std::f32::consts::PI * time / 3.0);
        assert!(
            arm_pose
                .translation
                .distance(glam::Vec3::from_array(arm_translation))
                < 2e-5
        );
        assert!(arm_pose.rotation.dot(arm_rotation).abs() > 1.0 - 2e-6);
        let arm_matrix = root_matrix
            * glam::Mat4::from_scale_rotation_translation(
                glam::Vec3::splat(arm_scale),
                arm_rotation,
                glam::Vec3::from_array(arm_translation),
            )
            * glam::Mat4::from_translation(glam::Vec3::new(-1.0, 0.0, 0.0));
        let shared = glam::Vec3::from_array(mesh.lods[0].vertices[2].position);
        let expected = root_matrix.transform_point3(shared) * 0.25
            + arm_matrix.transform_point3(shared) * 0.75;
        near(
            posed[2].position,
            Vec3::new(expected.x, expected.y, expected.z),
        );
    }
}
