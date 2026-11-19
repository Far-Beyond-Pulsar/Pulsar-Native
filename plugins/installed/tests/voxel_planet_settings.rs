//! The example planet's project settings (`assets/examples/voxel_planet/
//! .pulsar/project/`), loaded as the editor loads a project's: GI's probes
//! a metre apart over open ground (GI's default, half a metre, is for
//! rooms). The terrain's sky rays follow the probes through the frame
//! (`helio_frame_api::lighting::indirect_occlusion`): no setting of theirs.

// The project's settings groups come from every plugin.
use installed_plugins as _;
use std::path::Path;

#[test]
fn the_example_planet_keeps_open_ground_probe_spacing() {
    let config = engine_state::settings::global_config();
    pulsar_settings::project::register_all(config);
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/examples/voxel_planet");
    engine_state::settings::ProjectSettings::new(&project)
        .expect("the project's settings store")
        .load_all();
    let group = |key: &str| {
        pulsar_settings::project::rendering::groups()
            .into_iter()
            .find(|group| group.key == key)
            .unwrap_or_else(|| panic!("the `{key}` settings group is registered"))
    };
    use engine_backend::subsystems::render::helio_renderer::project_settings;
    let gi = project_settings::resolved(group("gi"));
    assert_eq!(gi.optional("probe_spacing"), Some(1.0));
}
