//! The plugin's example files: `*.trait.json` files, provided read-only into
//! every project under the plugin's folder at `examples/`.

use std::sync::Arc;

use engine_fs::{EmbeddedFile, FolderSource, MemoryFolder, PluginFolder};

/// Every embedded example, at its path inside the plugin's folder.
pub const EXAMPLES: &[EmbeddedFile] = engine_fs::embedded_files!("examples_assets":
    "examples/README.md",
    "examples/Damageable.trait.json",
    "examples/Interactable.trait.json",
    "examples/Lootable.trait.json",
);

fn open(folder: PluginFolder) -> Arc<dyn FolderSource> {
    Arc::new(MemoryFolder::with_files(folder, EXAMPLES))
}

/// Register the plugin's file provider (the plugin's `build` calls this).
pub(crate) fn build(p: &mut pulsar_plugins::PluginBuilder) {
    p.add(engine_fs::FileProvider {
        origin: p.origin(),
        open,
        // Embedded in the plugin; a game serves the packager's snapshot.
        in_game: engine_fs::InGame::Snapshot,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_trait_example_loads_with_the_trait_loader() {
        let mut count = 0;
        for file in EXAMPLES.iter().filter(|f| f.path.ends_with(".trait.json")) {
            let value: serde_json::Value = serde_json::from_slice(file.bytes)
                .unwrap_or_else(|e| panic!("{}: not JSON: {e}", file.path));
            let value = crate::normalize_legacy_trait(value).unwrap();
            let asset: ui_types_common::TraitAsset = serde_json::from_value(value)
                .unwrap_or_else(|e| panic!("{}: not a trait: {e}", file.path));
            assert!(
                ui_types_common::validate_name(&asset.name).is_ok(),
                "{}",
                file.path
            );
            assert!(asset.id.is_some(), "{}: a stable id", file.path);
            assert!(!asset.methods.is_empty(), "{}", file.path);
            count += 1;
        }
        assert!(count >= 3);
    }

    #[test]
    fn the_readme_is_embedded() {
        assert!(EXAMPLES
            .iter()
            .any(|f| f.path == "examples/README.md" && !f.bytes.is_empty()));
    }
}
