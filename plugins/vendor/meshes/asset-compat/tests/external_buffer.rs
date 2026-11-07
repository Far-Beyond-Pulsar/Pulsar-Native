//! A glTF with an external `.bin` must load by path no matter what the
//! process's working directory is: the buffer is named relative to the file.

use std::fs;

const TRIANGLE_GLTF: &str = r#"{
  "asset": { "version": "2.0" },
  "scene": 0,
  "scenes": [{ "nodes": [0] }],
  "nodes": [{ "mesh": 0 }],
  "meshes": [{ "primitives": [{ "attributes": { "POSITION": 0 } }] }],
  "buffers": [{ "uri": "triangle.bin", "byteLength": 36 }],
  "bufferViews": [{ "buffer": 0, "byteOffset": 0, "byteLength": 36 }],
  "accessors": [{
    "bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
    "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]
  }]
}"#;

#[test]
fn external_bin_resolves_relative_to_the_gltf_not_the_working_directory() {
    let dir =
        std::env::temp_dir().join(format!("helio_asset_compat_ext_bin_{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let positions: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
    let bytes: Vec<u8> = positions.iter().flat_map(|v| v.to_le_bytes()).collect();
    fs::write(dir.join("triangle.bin"), bytes).unwrap();
    fs::write(dir.join("triangle.gltf"), TRIANGLE_GLTF).unwrap();

    // The test's working directory is the crate root, which has no triangle.bin.
    let scene = helio_asset_compat::load_scene_file(dir.join("triangle.gltf"))
        .expect("external buffer next to the glTF must load");
    let triangles: usize = scene.meshes.iter().map(|m| m.indices.len() / 3).sum();
    assert_eq!(triangles, 1);

    let _ = fs::remove_dir_all(&dir);
}
