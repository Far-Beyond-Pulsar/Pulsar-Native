//! The sky texture layer on the authored component: defaults, saved-level
//! compatibility, independence from the procedural layers, selection,
//! orientation and blend.
use helio_pass_sky::sky_texture::{custom_path, source};
use helio_pass_sky::AtmosphereRow;
use sky_component::{AtmosphereComponent, SkyTexturePreset};

#[test]
fn defaults_and_saved_levels_keep_the_procedural_sky_and_no_texture() {
    let defaults = AtmosphereComponent::default();
    assert!(!defaults.sky_texture_enabled);
    let row = defaults.to_row();
    assert_eq!(row.sky_texture_source, source::NONE);
    assert_eq!(row.night_sky_enabled, 1);
    assert_eq!(row.star_count, AtmosphereRow::earth().star_count);
    // A level saved before the sky texture existed still loads.
    let old = serde_json::json!({ "night_sky_enabled": true, "star_count": 500.0 });
    let loaded: AtmosphereComponent = serde_json::from_value(old).unwrap();
    assert!(!loaded.sky_texture_enabled);
    assert_eq!(loaded.star_count, 500.0);
    assert_eq!(loaded.sky_texture_preset, SkyTexturePreset::GalaxyAndNebula);
}

#[test]
fn texture_and_procedural_layers_are_independent() {
    let both = AtmosphereComponent {
        sky_texture_enabled: true,
        sky_texture_preset: SkyTexturePreset::Galaxy,
        night_sky_enabled: true,
        shooting_stars_enabled: true,
        clouds_enabled: true,
        ..Default::default()
    };
    let row = both.to_row();
    assert_eq!(row.sky_texture_source, source::GALAXY);
    assert_eq!(
        (row.night_sky_enabled, row.shooting_stars_enabled, row.clouds_enabled),
        (1, 1, 1)
    );
    let no_stars = AtmosphereComponent {
        night_sky_enabled: false,
        ..both.clone()
    }
    .to_row();
    assert_eq!((no_stars.night_sky_enabled, no_stars.sky_texture_source), (0, source::GALAXY));
    let no_texture = AtmosphereComponent {
        sky_texture_enabled: false,
        ..both
    }
    .to_row();
    assert_eq!((no_texture.night_sky_enabled, no_texture.sky_texture_source), (1, source::NONE));
}

#[test]
fn preset_selection_orientation_and_blend_reach_the_row() {
    let mut sky = AtmosphereComponent {
        sky_texture_enabled: true,
        ..Default::default()
    };
    for (preset, code) in [
        (SkyTexturePreset::GalaxyAndNebula, source::GALAXY_NEBULA),
        (SkyTexturePreset::Galaxy, source::GALAXY),
        (SkyTexturePreset::Nebula, source::NEBULA),
    ] {
        sky.sky_texture_preset = preset;
        assert_eq!(sky.to_row().sky_texture_source, code);
        assert_eq!(AtmosphereComponent::from_row(&sky.to_row()).sky_texture_preset, preset);
    }
    sky.sky_texture_yaw_deg = 90.0;
    sky.sky_texture_pitch_deg = -30.0;
    sky.sky_texture_roll_deg = 15.0;
    sky.sky_texture_intensity = 3.0;
    sky.sky_texture_opacity = 2.0;
    let row = sky.to_row();
    assert!((row.sky_texture_yaw - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    assert!((row.sky_texture_pitch + 30f32.to_radians()).abs() < 1e-6);
    assert!((row.sky_texture_roll - 15f32.to_radians()).abs() < 1e-6);
    assert_eq!((row.sky_texture_intensity, row.sky_texture_opacity), (3.0, 1.0));
}

#[test]
fn a_custom_file_round_trips_and_an_empty_one_shows_nothing() {
    let absolute = std::env::current_dir().unwrap().join("my_sky.exr");
    let sky = AtmosphereComponent {
        sky_texture_enabled: true,
        sky_texture_preset: SkyTexturePreset::CustomFile,
        sky_texture_file: absolute.to_string_lossy().into_owned(),
        ..Default::default()
    };
    let row = sky.to_row();
    assert_ne!(row.sky_texture_source & source::CUSTOM, 0);
    assert_eq!(custom_path(row.sky_texture_source).unwrap(), absolute);
    assert_eq!(AtmosphereComponent::from_row(&row).sky_texture_file, sky.sky_texture_file);
    let empty = AtmosphereComponent {
        sky_texture_file: String::new(),
        ..sky
    };
    assert_eq!(empty.to_row().sky_texture_source, source::NONE);
}
