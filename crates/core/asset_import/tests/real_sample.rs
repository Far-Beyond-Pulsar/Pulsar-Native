//! Import of a real rigged, animated model, for checking the importer against
//! something not made by hand. Skipped unless `SKM_SAMPLE_DIR` names a folder
//! holding `BrainStem.glb` (Khronos glTF-Sample-Assets; its files are not
//! redistributable, so neither it nor anything made from it is committed).
//! Outputs go to `SKM_OUT_DIR` (default: a temp folder) for the plugin's
//! `render_real_sample` test to draw.

use std::path::{Path, PathBuf};

use asset_import::{importer_for, Imported, OptionValues, Previous};
use skeletal_assets::assets::{load_animseq_asset, load_skeleton_asset};
use skeletal_assets::skmesh::{load_skmesh_asset, validate_mesh};

fn sample() -> Option<PathBuf> {
    let dir = std::env::var_os("SKM_SAMPLE_DIR")?;
    let path = Path::new(&dir).join("BrainStem.glb");
    path.is_file().then_some(path)
}

fn run(source: &Path, out: &Path, previous: &[asset_import::ImportOutput]) -> Imported {
    importer_for("glb")
        .unwrap()
        .import(
            source,
            &out.join("BrainStem.mesh"),
            &OptionValues::new(),
            &Previous { project_root: out, outputs: previous },
        )
        .expect("imports")
}

#[test]
fn a_real_rigged_model_imports_consistently() {
    let Some(source) = sample() else {
        eprintln!("SKM_SAMPLE_DIR not set; skipping");
        return;
    };
    let out = std::env::var_os("SKM_OUT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("skm_real_sample"));
    let _ = std::fs::remove_dir_all(&out);
    std::fs::create_dir_all(&out).unwrap();

    let first = run(&source, &out, &[]);
    let skeleton_path = out.join(&first.outputs.iter().find(|o| o.kind == "skeleton").unwrap().path);
    let skeleton = load_skeleton_asset(&skeleton_path).unwrap();
    let mesh = load_skmesh_asset(&first.asset).unwrap();
    assert_eq!(mesh.skeleton, Some(skeleton.id.clone()));
    let issues = validate_mesh(&mesh, Some(&skeleton.skeleton()));
    assert!(issues.is_empty(), "{issues:?}");
    let anims: Vec<_> = first.outputs.iter().filter(|o| o.kind == "animseq").collect();
    assert!(!anims.is_empty());
    for a in &anims {
        let seq = load_animseq_asset(&out.join(&a.path)).unwrap();
        assert_eq!(seq.skeleton, Some(skeleton.id.clone()));
        assert_eq!(seq.preview_mesh, Some(mesh.id.clone()));
        assert!(seq.clip.orphan_tracks(&skeleton.skeleton()).is_empty());
    }
    let s = mesh.stats();
    eprintln!(
        "bones {} joints {} verts {} tris {} maxinf {} anims {}",
        skeleton.bones.len(), s.joints, s.vertices, s.triangles, s.max_influences, anims.len()
    );

    let second = run(&source, &out, &first.outputs);
    assert_eq!(second.outputs, first.outputs, "ids and paths are stable");
}
