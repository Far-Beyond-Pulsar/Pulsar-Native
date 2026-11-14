//! The plugin's example render graphs: a read-only `examples/` folder in the
//! plugin's virtual folder (`plugins/<plugin>/examples/`), served from files
//! embedded in the binary (`examples_assets/`).
//!
//! These are teaching graphs. The pipelines the engine ships live in the
//! `pulsar.default_pipelines` plugin's folder, made from every installed
//! plugin's passes; the ones here are small hand-made graphs that show one
//! idea each (the flow chain, a stage, removing a stage, display passes,
//! resource nodes).

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
    example!("Minimal.rendergraph"),
    example!("LeanForward.rendergraph"),
    example!("NoPostProcess.rendergraph"),
    example!("DebugOverlays.rendergraph"),
    example!("Resources.rendergraph"),
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
    use crate::document::decode;
    use crate::model;
    use helio_graph_asset::GraphAsset;
    use helio_presets::graphs::base_registry;

    fn graphs() -> Vec<(&'static str, &'static str)> {
        EXAMPLES
            .iter()
            .filter(|f| f.path.ends_with(".rendergraph"))
            .map(|f| (f.path, std::str::from_utf8(f.bytes).expect("utf-8")))
            .collect()
    }

    #[test]
    fn every_example_is_listed_in_the_readme() {
        let readme = std::str::from_utf8(
            EXAMPLES
                .iter()
                .find(|f| f.path == "examples/README.md")
                .expect("README")
                .bytes,
        )
        .unwrap();
        assert!(graphs().len() >= 4, "several varied graphs");
        for (path, _) in graphs() {
            let name = path.trim_start_matches("examples/");
            assert!(readme.contains(name), "README lists {name}");
        }
    }

    #[test]
    fn every_example_parses_decodes_and_has_no_errors() {
        let registry = base_registry();
        for (path, text) in graphs() {
            let asset = GraphAsset::from_json(text).unwrap_or_else(|e| panic!("{path}: {e}"));
            assert!(!asset.name.is_empty() && !asset.description.is_empty(), "{path} is described");
            assert!(
                !asset.comments.is_empty(),
                "{path} explains itself with comment boxes"
            );
            // The editor's own reader.
            let document = decode(text, &registry).unwrap_or_else(|e| panic!("{path}: {e}"));
            assert_eq!(document.main.nodes.len(), asset.nodes.len(), "{path}");
            // Helio's validation: nothing that stops the graph from building.
            assert_eq!(model::problems(&asset, &registry), Vec::<String>::new(), "{path}");
        }
    }

    #[test]
    fn examples_are_distinct_graphs() {
        let mut shapes = std::collections::HashSet::new();
        for (path, text) in graphs() {
            let asset = GraphAsset::from_json(text).unwrap();
            let kinds: Vec<_> = asset.nodes.iter().map(|n| n.kind.clone()).collect();
            assert!(shapes.insert(kinds), "{path} repeats another example");
        }
    }
}
