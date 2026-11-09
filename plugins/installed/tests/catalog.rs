//! The plugin catalog's proofs (Pulsar-Native#1608, phase 1), over every
//! installed plugin plus a fixture plugin (`tests/fixtures/catalog_plugin`:
//! a root crate registering what its component and editor crates hold).
//!
//! - Static fixture: one plugin's runtime and editor registrations, from
//!   several capabilities, all resolve to the plugin's one id.
//! - Renderer fixture: the catalog reports which feature registered which
//!   node, and the engine's graph registry (built from the same
//!   registrations) has the same node ids, owned by the same features.
//! - Disabled plugin: with the plugin disabled, none of its registrations of
//!   any type is handed out, while the catalog still lists them.
//! - Every installed plugin: its contributions come from its root.

use std::collections::BTreeMap;
use std::sync::Mutex;

use engine_backend::subsystems::render::helio_renderer::render_features;
use plugin_manager::catalog::PluginCatalog;
use pulsar_plugins::{Resolution, Summary};

// The installed plugins and the fixture, linked for their registrations.
use catalog_fixture_plugin as _;
use installed_plugins as _;

const FIXTURE: &str = "pulsar.test_catalog";
const FIXTURE_ROOT: &str = "catalog_fixture_plugin";

/// The tests change the process's activated plugins: one at a time.
static ACTIVATION: Mutex<()> = Mutex::new(());

fn fixture_features() -> BTreeMap<&'static str, Vec<&'static str>> {
    catalog_fixture_component::FEATURES
        .iter()
        .map(|(feature, nodes)| (*feature, nodes.to_vec()))
        .collect()
}

#[test]
fn a_plugins_runtime_and_editor_registrations_resolve_to_its_id() {
    let _guard = ACTIVATION
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    pulsar_plugins::deactivate();
    let catalog = PluginCatalog::snapshot();
    let fixture = catalog.plugin(FIXTURE).expect("the fixture is installed");
    assert!(fixture.enabled);
    assert_eq!(fixture.modules, [FIXTURE_ROOT]);
    let categories: Vec<&str> = fixture
        .contributions
        .iter()
        .map(|summary| summary.category)
        .collect();
    for category in [
        // Runtime, from the component crate's code.
        "render.features",
        "game.content_report",
        "settings.project",
        // Editor, from the editor crate's code.
        "editor.thumbnail",
        "editor.ai_tools",
    ] {
        assert!(
            categories.contains(&category),
            "{category} is missing from {categories:?}"
        );
    }
    for summary in fixture.contributions.iter().flat_map(Summary::flatten) {
        assert_eq!(summary.origin, FIXTURE_ROOT, "{summary:?}");
        assert_eq!(
            pulsar_plugins::plugin_of(summary.origin).map(|plugin| plugin.id.as_str()),
            Some(FIXTURE)
        );
    }
    assert!(
        catalog.problems.is_empty(),
        "the catalog holds together: {:?}",
        catalog.problems
    );
}

#[test]
fn the_catalog_reports_feature_to_node_ownership_and_the_graph_has_the_same_nodes() {
    let _guard = ACTIVATION
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    pulsar_plugins::deactivate();
    let catalog = PluginCatalog::snapshot();
    let fixture = catalog.plugin(FIXTURE).unwrap();
    assert_eq!(fixture.feature_nodes(), fixture_features());

    // The engine's graph registry, built from the same registrations as
    // every pipeline it runs.
    let registry = helio_presets::graphs::registry_with(&render_features::plugin_features());
    for (feature, nodes) in fixture_features() {
        for node in nodes {
            assert!(
                registry.get(node).is_some(),
                "{node} is not in the graph registry"
            );
            assert_eq!(registry.feature_of(node), Some(feature));
        }
    }

    // Every installed plugin's nodes, as the catalog reports them, are the
    // graph registry's, owned by the same features.
    for plugin in &catalog.plugins {
        for (feature, nodes) in plugin.feature_nodes() {
            for node in nodes {
                assert_eq!(
                    registry.feature_of(node),
                    Some(feature),
                    "{}: {node} belongs to {feature}",
                    plugin.descriptor.id
                );
            }
        }
    }
}

#[test]
fn a_disabled_plugin_has_every_registration_filtered() {
    let _guard = ACTIVATION
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let everything_but_the_fixture = Resolution {
        enabled: pulsar_plugins::installed()
            .iter()
            .map(|plugin| plugin.id.clone())
            .filter(|id| id != FIXTURE)
            .collect(),
        problems: Vec::new(),
    };
    pulsar_plugins::activate(&everything_but_the_fixture);

    let ours = |summaries: Vec<Summary>| -> Vec<Summary> {
        summaries
            .into_iter()
            .filter(|summary| summary.origin == FIXTURE_ROOT)
            .collect()
    };
    let linked = ours(pulsar_plugins::contributions());
    let handed_out = ours(pulsar_plugins::active_contributions());
    let catalog = PluginCatalog::snapshot();
    let ids = render_features::plugin_features().ids();
    pulsar_plugins::deactivate();

    assert!(
        linked.len() >= 5,
        "its registrations are linked: {linked:?}"
    );
    assert!(
        handed_out.is_empty(),
        "no registry hands out a disabled plugin's registrations: {handed_out:?}"
    );
    let fixture = catalog.plugin(FIXTURE).unwrap();
    assert!(!fixture.enabled);
    assert_eq!(
        fixture.contributions.len(),
        linked.len(),
        "the catalog still lists them, to diagnose"
    );
    for (feature, _) in catalog_fixture_component::FEATURES {
        assert!(!ids.contains(&feature), "{feature} is in the renderer");
    }

    // Enabled again, every one is handed out.
    assert_eq!(
        ours(pulsar_plugins::active_contributions()).len(),
        linked.len()
    );
}

#[test]
fn every_contribution_is_a_plugins_or_the_engines() {
    let _guard = ACTIVATION
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    pulsar_plugins::deactivate();
    let catalog = PluginCatalog::snapshot();
    for plugin in &catalog.plugins {
        if plugin.descriptor.id == FIXTURE {
            continue;
        }
        for summary in &plugin.contributions {
            assert_eq!(summary.origin, plugin.descriptor.root);
        }
    }
    // The engine's own contributions say so; nothing else lacks an owner.
    assert!(!catalog.engine.is_empty(), "the engine registers its own");
    assert!(
        catalog.unknown.is_empty(),
        "every origin is a plugin's root or the engine's: {:?}",
        catalog
            .unknown
            .iter()
            .map(|summary| (summary.origin, summary.category, &summary.id))
            .collect::<Vec<_>>()
    );
    assert!(pulsar_plugins::unknown_origins().is_empty());
    assert!(
        catalog
            .plugins
            .iter()
            .filter(|plugin| plugin.descriptor.runtime())
            .all(|plugin| !plugin.contributions.is_empty()),
        "every runtime plugin contributes something"
    );
}
