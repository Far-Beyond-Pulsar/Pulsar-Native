//! The example voxel levels are Helio's voxel_flight worlds
//! (`examples/voxel_flight/worlds.rs`, compiled in here) as Pulsar levels:
//! Earth, Moon and Desert, and Blocks (Earth's world on flat ground). They
//! are also the editor's ready-made worlds (`scene::voxel_presets`). Each is
//! loaded as the editor loads it and its ground, features, air, haze,
//! clouds, wind, meadow, exposure and camera are compared with voxel_flight's.

use std::path::Path;

use engine_backend::scene::voxel_frame::VoxelWorldSync;
use engine_backend::scene::voxel_presets::VoxelWorldPreset;
use engine_backend::scene::EditorCamera;
use glam::DVec3;
use helio_component::{
    AtmosphereComponent, FoliageComponent, FoliageSourceRow, GlobalFogComponent,
    PostProcessVolumeComponent, VoxelTerrainComponent, VoxelWorldShape, WindComponent,
};
use helio_pass_foliage_place::GpuFoliageType;
use helio_pass_voxel_planet::layers::TerrainLayers;
use pulsar_scenedb::{Entity, World};

#[allow(dead_code)]
#[path = "../../../renderer/helio/crates/helio-default-passes/examples/voxel_flight/worlds.rs"]
mod flight;

use flight::Preset;

/// The level of example project `name` in `assets/examples`, loaded and its
/// voxel world's dependents synced (as the editor's scene step does), and
/// its camera.
fn level(name: &str) -> (World, EditorCamera) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join(format!("../../../assets/examples/{name}/scene/default.level"));
    let mut scene = engine_backend::scene::new_scene();
    let extras = engine_backend::scene::RuntimeLevel::load_into(&path, &mut scene.world)
        .expect("load the level");
    let mut world = scene.world;
    VoxelWorldSync::new(&world).poll(&mut world);
    (world, extras.editor_camera.expect("a camera"))
}

/// The level's only `$class` component, if any.
macro_rules! one {
    ($world:expr, $class:ty) => {{
        let all: Vec<$class> = $world.query::<&$class>().map(|(_, c)| c.clone()).collect();
        assert!(all.len() <= 1, "one {}", stringify!($class));
        all.into_iter().next()
    }};
}

/// Example `name` is voxel_flight's `world`, on flat ground when `flat`.
fn check(name: &str, world: Preset, flat: bool) {
    let (level, camera) = level(name);
    let terrains: Vec<(Entity, VoxelTerrainComponent)> = level
        .query::<&VoxelTerrainComponent>()
        .map(|(instance, terrain)| (instance, terrain.clone()))
        .collect();
    let [(instance, terrain)] = &terrains[..] else {
        panic!("{name}: one terrain");
    };
    let radius = world.radius_m();

    // The ground: form, layer stack and features, editable.
    if flat {
        assert_eq!(terrain.shape, VoxelWorldShape::InfinitePlane, "{name}");
    } else {
        assert_eq!((terrain.shape, terrain.planet_radius), (VoxelWorldShape::Sphere, radius), "{name}");
    }
    let settings = helio_component::voxel_world::generator_settings(&level, *instance, terrain);
    let layers: TerrainLayers = serde_json::from_str(&settings).unwrap();
    assert_eq!(layers, if flat { TerrainLayers::flat_at(0.0) } else { world.stack() }, "{name}");
    assert_eq!(helio_component::feature_recipe(&terrain.features), world.features(), "{name}");
    assert!(terrain.appearance_parameters.contains("\"materials\""), "{name}: the look of the features' materials");
    assert!(terrain.enabled && terrain.editable, "{name}: sculpted as it is");

    // The air: sky and clouds, the ground haze.
    let sky = one!(level, AtmosphereComponent);
    match world.sky(radius) {
        Some(want) => {
            let sky = sky.expect("a sky");
            assert!(sky.enabled, "{name}");
            let got = sky.to_row();
            let air = |row: &helio_pass_sky::AtmosphereComponent| {
                (
                    row.rayleigh_scattering,
                    row.mie_scattering,
                    row.mie_absorption,
                    row.mie_scale_height,
                    row.ozone_absorption,
                    row.ground_albedo,
                )
            };
            assert_eq!(air(&got), air(&want), "{name}");
            assert_eq!(got.clouds(), want.clouds(), "{name}");
        }
        None => assert!(sky.is_none_or(|sky| !sky.enabled), "{name}: no air"),
    }
    let haze = one!(level, GlobalFogComponent).filter(|fog| fog.enabled);
    match world.haze(radius) {
        Some(want) => {
            // Over flat ground heights are along world Y.
            let want = if flat {
                helio_pass_volumetric_fog::GlobalFogComponent { planet: [0.0; 4], ..want }
            } else {
                want
            };
            assert_eq!(haze.expect("a haze").medium.to_medium(), want, "{name}");
        }
        None => assert!(haze.is_none(), "{name}: no haze"),
    }

    // The meadow, in its wind.
    let mut rows: Vec<GpuFoliageType> = level
        .query::<&FoliageComponent>()
        .map(|(_, foliage)| {
            let row = FoliageSourceRow::of(foliage);
            bytemuck::pod_read_unaligned(&bytemuck::bytes_of(&row)[..std::mem::size_of::<GpuFoliageType>()])
        })
        .collect();
    match world.meadow() {
        Some(meadow) => {
            assert_eq!(rows.len(), meadow.types.len(), "{name}");
            for (row, want) in rows.iter_mut().zip(&meadow.types) {
                let want = GpuFoliageType::from(*want);
                // The component's rows carry no impostor (the meadow's is unused).
                row.mesh_or_impostor_id = want.mesh_or_impostor_id;
                assert_eq!(bytemuck::bytes_of(row), bytemuck::bytes_of(&want), "{name}: {row:?}\n{want:?}");
            }
            let wind = one!(level, WindComponent).expect("a wind").to_row();
            for (a, b) in wind
                .direction_speed
                .iter()
                .chain(&wind.gust)
                .zip(meadow.wind.direction_speed.iter().chain(&meadow.wind.gust))
            {
                assert!((a - b).abs() < 1e-6, "{name}: {wind:?} vs {:?}", meadow.wind);
            }
        }
        None => assert!(rows.is_empty(), "{name}: no plants"),
    }

    // The camera's exposure.
    let look = one!(level, PostProcessVolumeComponent).expect("a post-process look");
    assert!(look.unbound && look.overrides.override_tonemap, "{name}");
    assert_eq!(look.settings.tonemap_exposure, world.exposure(), "{name}");

    // The camera starts over the ground, not under it or in space.
    let planet = helio_component::voxel_world::terrain_world(&level, *instance).unwrap();
    let eye = DVec3::from_array(camera.position);
    let (ground, _) = helio_component::voxel_world::frame_view(&planet, eye, DVec3::X, 0.0);
    let up = if flat { DVec3::Y } else { ground.normalize() };
    let height = (eye - ground).dot(up);
    assert!((2.0..1_000.0).contains(&height), "{name}: the camera is {height} m over the ground");
}

#[test]
fn the_earth_level_is_voxel_flights_earth() {
    check("voxel_planet", Preset::Earth, false);
}

#[test]
fn the_moon_level_is_voxel_flights_moon() {
    check("voxel_moon", Preset::Moon, false);
}

#[test]
fn the_desert_level_is_voxel_flights_desert() {
    check("voxel_desert", Preset::Desert, false);
}

#[test]
fn the_blocks_level_is_voxel_flights_earth_on_flat_ground() {
    check("voxel_blocks", Preset::Earth, true);
}

/// Every ready-made world is one of these examples.
#[test]
fn the_presets_are_the_examples() {
    let examples: Vec<&str> = VoxelWorldPreset::ALL.iter().map(|preset| preset.example()).collect();
    assert_eq!(examples, ["voxel_planet", "voxel_moon", "voxel_desert", "voxel_blocks"]);
}
