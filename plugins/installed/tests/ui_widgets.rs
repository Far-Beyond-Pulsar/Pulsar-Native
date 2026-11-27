//! The UI plugin in the installed set: every pipeline the engine ships draws
//! widgets, and a project that disables the plugin has none of it (its class,
//! feature, render-host part, pointer claim and asset loader), keeping a
//! level's widgets as saved.
//!
//! One test: the plugin set is process-wide.

use engine_backend::scene::game_hooks::PointerClaimRegistration;
use engine_backend::scene::{attachments, RuntimeLevel, SceneWorldExt};
use engine_backend::subsystems::render::helio_renderer::{render_features, RenderHostPartRegistration};
use helio_component::asset_component::AssetLoaderRegistration;
use serde_json::json;

// Every installed plugin is linked, as the editor links them.
use installed_plugins as _;

fn project(enabled: bool) -> tempfile::TempDir {
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("Pulsar.toml"),
        format!("[plugins]\n\"pulsar.ui\" = {enabled}\n"),
    )
    .unwrap();
    let level = json!({
        "version": "2.1",
        "objects": [{
            "id": "hud", "name": "HUD", "object_type": "Empty",
            "transform": { "position": [0.0, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0], "scale": [1.0, 1.0, 1.0] },
            "parent": null, "visible": true, "locked": false, "props": {}
        }],
        "components": { "hud": [
            { "index": 0, "class_name": "WidgetComponent", "enabled": true, "data": {
                "enabled": true, "widget_asset": "ui/Hud.widget", "parameters": { "score": "7" }
            } }
        ] },
        "metadata": {}
    });
    std::fs::write(project.path().join("ui.level"), level.to_string()).unwrap();
    project
}

fn registered() -> (bool, bool, bool, bool, bool) {
    (
        pulsar_world_registry::component_id_for_class("WidgetComponent").is_some(),
        render_features::plugin_features().contains("helio.ui"),
        pulsar_plugins::registered::<RenderHostPartRegistration>().any(|r| r.name == "ui.widgets"),
        pulsar_plugins::registered::<PointerClaimRegistration>().any(|r| r.name == "ui.buttons"),
        pulsar_plugins::registered::<AssetLoaderRegistration>().any(|r| r.kind == "widget"),
    )
}

#[test]
fn widgets_are_in_every_shipped_pipeline_and_absent_when_disabled() {
    engine_state::EngineContext::new().set_global();
    let on = project(true);
    engine_state::set_project_path(on.path().display().to_string());
    assert_eq!(registered(), (true, true, true, true, true));

    // Every pipeline the shipped files are made from places both nodes.
    let mut registry = helio_feature_api::NodeRegistry::new();
    render_features::plugin_features().register_into(&mut registry);
    for preset in [
        helio_presets::Preset::Deferred,
        helio_presets::Preset::Forward,
    ] {
        let kinds: Vec<String> = preset
            .nodes(&registry)
            .into_iter()
            .map(|node| node.kind)
            .collect();
        let at = |kind: &str| kinds.iter().position(|k| k == kind);
        let world = at(helio_ui_api::WORLD_NODE)
            .unwrap_or_else(|| panic!("{preset:?} has no world widgets: {kinds:?}"));
        let screen = at(helio_ui_api::SCREEN_NODE).expect("screen widgets");
        let tone_map = at("post_process").or_else(|| at("helio.pass.post_process"));
        assert!(world < screen, "{kinds:?}");
        assert!(tone_map.is_none_or(|t| t < world), "{kinds:?}");
    }

    let off = project(false);
    engine_state::set_project_path(off.path().display().to_string());
    assert!(!pulsar_plugins::is_enabled("pulsar.ui"));
    assert_eq!(registered(), (false, false, false, false, false));
    let scene = RuntimeLevel::load(&off.path().join("ui.level"))
        .expect("the level loads")
        .scene();
    {
        let scene = scene.read();
        let hud = scene.world.entity_for("hud").unwrap();
        let records = pulsar_world_registry::component_records(&scene.world, hud);
        assert_eq!(records[0].class_name, "WidgetComponent");
        assert_eq!(records[0].data["parameters"]["score"], json!("7"));
        let instances = attachments::instances(&scene.world, hud);
        assert!(scene
            .world
            .get::<attachments::UnresolvedComponent>(instances[0])
            .is_some());
    }

    engine_state::set_project_path(on.path().display().to_string());
    assert_eq!(registered(), (true, true, true, true, true));
}
