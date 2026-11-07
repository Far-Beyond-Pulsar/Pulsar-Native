use std::path::PathBuf;

use helio_snapshot::{render_snapshot, SnapshotConfig, ViewDirection};

fn main() {
    env_logger::init();

    // Resolve test.fbx relative to the workspace root (two levels up from this crate).
    let model_path: PathBuf =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../examples/assets/test.fbx");

    println!("Rendering: {}", model_path.display());

    let img = render_snapshot(
        &model_path,
        SnapshotConfig {
            width: 1024,
            height: 1024,
            view: ViewDirection::Isometric,
            ..Default::default()
        },
    )
    .expect("snapshot failed");

    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("snapshot_output.png");
    img.save(&out).expect("failed to save PNG");
    println!("Saved: {}", out.display());
}
