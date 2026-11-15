//! The plugin's example files: a read-only `examples/` folder in the plugin's
//! virtual folder (`plugins/<plugin>/examples/`), served from files embedded
//! in the binary (`examples_assets/`). The viewer registers three file types
//! (`.fbx`, `.mesh`, `.png`); each has several varied examples to open in it.

use std::sync::Arc;

use engine_fs::{EmbeddedFile, FileProvider, FolderSource, InGame, MemoryFolder, PluginFolder};

macro_rules! example {
    ($name:literal) => {
        EmbeddedFile {
            path: concat!("examples/", $name),
            bytes: include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/examples_assets/",
                $name
            )),
        }
    };
}

/// Every embedded file, by its path inside the plugin's folder.
pub(crate) static EXAMPLES: &[EmbeddedFile] = &[
    example!("README.md"),
    example!("Pyramid.fbx"),
    example!("TwoMaterialBox.fbx"),
    example!("Pyramid.mesh"),
    example!("TwoMaterialBox.mesh"),
    example!("UvGrid.png"),
    example!("BumpNormals.png"),
    example!("RoughnessRamp.png"),
    example!("AlphaDisc.png"),
    example!("Bricks.png"),
];

fn open(folder: PluginFolder) -> Arc<dyn FolderSource> {
    Arc::new(MemoryFolder::with_files(folder, EXAMPLES))
}

/// Registers the plugin's example files.
pub(crate) fn build(p: &mut pulsar_plugins::PluginBuilder) {
    p.add(FileProvider {
        origin: p.origin(),
        open,
        in_game: InGame::Snapshot,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_extension(ext: &str) -> Vec<&'static EmbeddedFile> {
        EXAMPLES
            .iter()
            .filter(|file| file.path.ends_with(&format!(".{ext}")))
            .collect()
    }

    #[test]
    fn every_registered_file_type_has_several_examples_listed_in_the_readme() {
        let readme = std::str::from_utf8(
            EXAMPLES
                .iter()
                .find(|f| f.path == "examples/README.md")
                .expect("README")
                .bytes,
        )
        .unwrap();
        for ext in ["fbx", "mesh", "png"] {
            let files = with_extension(ext);
            assert!(files.len() >= 2, ".{ext} has several examples");
            for file in files {
                let name = file.path.trim_start_matches("examples/");
                assert!(readme.contains(name), "README lists {name}");
            }
        }
    }

    #[test]
    fn every_png_decodes() {
        for file in with_extension("png") {
            let img = image::load_from_memory_with_format(file.bytes, image::ImageFormat::Png)
                .unwrap_or_else(|e| panic!("{}: {e}", file.path));
            assert!(img.width() > 0 && img.height() > 0, "{}", file.path);
        }
    }

    #[test]
    fn every_fbx_loads_as_a_model_with_geometry() {
        let dir =
            std::env::temp_dir().join(format!("asset_viewer_examples_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for file in with_extension("fbx") {
            let name = file.path.trim_start_matches("examples/");
            let path = dir.join(name);
            std::fs::write(&path, file.bytes).unwrap();
            let values = meshes_component::mesh_cache::resolve_options(&path, "fbx");
            let scene = meshes_component::mesh_cache::load_model_scene(&path, &values)
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(!scene.parts.is_empty(), "{name} has geometry");
        }
    }

    #[test]
    fn every_native_mesh_decodes_with_the_meshes_codec() {
        for file in with_extension("mesh") {
            let (asset, _id) = meshes_component::mesh_cache::decode_asset(file.bytes)
                .unwrap_or_else(|| panic!("{} does not decode", file.path));
            assert!(!asset.sections.is_empty(), "{} has sections", file.path);
            assert!(
                !asset.geometry.vertices.is_empty(),
                "{} has vertices",
                file.path
            );
        }
    }

    #[test]
    fn the_two_material_box_keeps_two_material_slots() {
        let file = EXAMPLES
            .iter()
            .find(|f| f.path == "examples/TwoMaterialBox.mesh")
            .unwrap();
        let (asset, _) = meshes_component::mesh_cache::decode_asset(file.bytes).unwrap();
        assert_eq!(asset.material_slots.len(), 2);
    }
}
