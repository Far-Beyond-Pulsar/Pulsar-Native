//! The shipped pipelines as files: one per preset and per lighting-model
//! pipeline, each a valid graph that builds the preset's nodes in order, made
//! for whatever nodes the registry holds, and the same whatever the project's
//! renderer settings are.

use helio_graph_asset::GraphAsset;
use helio_presets::Preset;
use render_pipelines_plugin::{asset_path, files, shipped};

fn registry() -> helio_graph_asset::NodeRegistry {
    helio_presets::graphs::registry_with(&Default::default())
}

#[test]
fn every_shipped_pipeline_is_a_file_that_validates() {
    let registry = registry();
    let files = files(&registry);
    let names: Vec<&str> = files.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        [
            "Deferred.rendergraph",
            "DeferredFxaa.rendergraph",
            "Forward.rendergraph"
        ],
        "Helio's base registry has no lighting model pipelines"
    );
    for (name, bytes) in &files {
        let asset = GraphAsset::from_json(std::str::from_utf8(bytes).unwrap())
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let validation = helio_graph_asset::validate(&asset, &registry);
        assert!(
            !validation.has_errors(),
            "{name}: {:?}",
            validation.diagnostics
        );
    }
}

#[test]
fn the_deferred_file_is_the_deferred_preset_and_follows_the_projects_antialiasing() {
    let registry = registry();
    let text = String::from_utf8(files(&registry)["Deferred.rendergraph"].clone()).unwrap();
    let asset = GraphAsset::from_json(&text).unwrap();
    assert_eq!(
        asset,
        {
            let mut expected = helio_presets::graphs::graph_asset(Preset::Deferred, &registry);
            expected.name = "Deferred".into();
            expected.description = asset.description.clone();
            expected
        },
        "the file is the preset's graph"
    );
    // Anti-aliasing is chosen when the graph builds, not baked in.
    let kinds: Vec<&str> = asset
        .nodes
        .iter()
        .chain(
            asset
                .sections
                .iter()
                .flat_map(|section| section.nodes.iter()),
        )
        .map(|node| node.kind.as_str())
        .collect();
    assert!(kinds.contains(&helio_presets::features::ANTIALIASING));
    assert!(!kinds.contains(&"helio.pass.tsr") && !kinds.contains(&"helio.pass.fxaa"));
}

#[test]
fn the_old_built_in_ids_name_the_files_that_replace_them() {
    let registry = registry();
    let replaced = |old: &str| {
        shipped(&registry)
            .into_iter()
            .find(|shipped| shipped.replaces.iter().any(|id| id == old))
            .map(|shipped| asset_path(&shipped.stem))
    };
    let deferred = Some("plugins/pulsar.render_pipelines/Deferred.rendergraph".to_string());
    assert_eq!(replaced("helio.deferred"), deferred);
    assert_eq!(replaced("helio.deferred_graph"), deferred);
    assert_eq!(
        replaced("helio.fxaa").as_deref(),
        Some("plugins/pulsar.render_pipelines/DeferredFxaa.rendergraph")
    );
    assert_eq!(
        replaced("helio.forward_only").as_deref(),
        Some("plugins/pulsar.render_pipelines/Forward.rendergraph")
    );
    assert_eq!(replaced("graphs/mine.rendergraph"), None);
}
