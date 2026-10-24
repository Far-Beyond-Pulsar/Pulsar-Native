//! The example voxel levels are Helio's voxel_flight setups
//! (`examples/voxel_flight/worlds.rs`) as Pulsar levels: Earth, Moon and
//! Desert. Each is loaded as the editor loads it and its world, air,
//! haze, clouds, wind and meadow are compared with Helio's own values.

use std::path::Path;

use engine_backend::scene::voxel_frame::VoxelWorldSync;
use helio_component::{
    AtmosphereComponent, FoliageComponent, FoliageSourceRow, GlobalFogComponent,
    VoxelTerrainComponent, WindComponent,
};
use helio_pass_foliage_place::GpuFoliageType;
use helio_pass_sky::CloudLayer;
use helio_pass_voxel_planet::feature_recipe::FeatureRecipe;
use pulsar_scenedb::World;

/// The level `name` in `assets/examples`, loaded and its voxel world's
/// dependents synced (as the editor's scene step does).
fn level(name: &str) -> World {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../../assets/examples/{name}.level"));
    let mut scene = engine_backend::scene::new_scene();
    engine_backend::scene::RuntimeLevel::load_into(&path, &mut scene.world).expect("load the level");
    let mut world = scene.world;
    VoxelWorldSync::new(&world).poll(&mut world);
    world
}

fn terrain(world: &World) -> VoxelTerrainComponent {
    let terrains: Vec<_> = world.query::<&VoxelTerrainComponent>().map(|(_, t)| t.clone()).collect();
    assert_eq!(terrains.len(), 1);
    terrains[0].clone()
}

/// The level's only `$class` component, if any.
macro_rules! one {
    ($world:expr, $class:ty) => {{
        let all: Vec<$class> = $world.query::<&$class>().map(|(_, c)| c.clone()).collect();
        assert!(all.len() <= 1, "one {}", stringify!($class));
        all.into_iter().next()
    }};
}

/// voxel_flight's haze (`Preset::haze`) over a planet of `radius` m.
fn flight_haze(extinction: f32, falloff: f32, anisotropy: f32, albedo: [f32; 3], radius: f64) -> helio_pass_volumetric_fog::GlobalFogComponent {
    helio_pass_volumetric_fog::GlobalFogComponent {
        mode: 1,
        extinction,
        height_falloff: falloff,
        anisotropy,
        albedo,
        ..Default::default()
    }
    .around_planet([0.0; 3], radius)
}

#[test]
fn the_earth_level_is_voxel_flights_earth() {
    let world = level("voxel_planet");
    let terrain = terrain(&world);
    assert_eq!(helio_component::feature_recipe(&terrain.features), FeatureRecipe::earth());
    assert!(terrain.appearance_parameters.contains("\"materials\""), "the look of the features' materials");
    // Earth's air with fair-weather cumulus, and its ground haze.
    let sky = one!(world, AtmosphereComponent).unwrap();
    assert!(sky.enabled);
    assert_eq!(sky.to_row().clouds(), Some(CloudLayer::cumulus()));
    let fog = one!(world, GlobalFogComponent).unwrap();
    assert!(fog.enabled);
    assert_eq!(fog.medium.to_medium(), flight_haze(0.0006, 0.004, 0.6, [1.0; 3], 6_371_000.0));
    // The meadow with tall stands, in a fresh breeze.
    let meadow = helio_pass_voxel_planet::meadow::meadow().with_tall_stands().with_wind([1.0, 0.35], 4.5, 0.8);
    let mut rows: Vec<GpuFoliageType> = world
        .query::<&FoliageComponent>()
        .map(|(_, foliage)| {
            let row = FoliageSourceRow::of(foliage);
            bytemuck::pod_read_unaligned(&bytemuck::bytes_of(&row)[..std::mem::size_of::<GpuFoliageType>()])
        })
        .collect();
    assert_eq!(rows.len(), meadow.types.len());
    for (row, want) in rows.iter().zip(&meadow.types) {
        let want = GpuFoliageType::from(*want);
        // The component's rows carry no impostor (the meadow's is unused).
        let row = GpuFoliageType { mesh_or_impostor_id: want.mesh_or_impostor_id, ..*row };
        assert_eq!(bytemuck::bytes_of(&row), bytemuck::bytes_of(&want), "{row:?}\n{want:?}");
    }
    let wind = one!(world, WindComponent).unwrap().to_row();
    for (a, b) in wind.direction_speed.iter().chain(&wind.gust).zip(meadow.wind.direction_speed.iter().chain(&meadow.wind.gust)) {
        assert!((a - b).abs() < 1e-6, "{wind:?} vs {:?}", meadow.wind);
    }
}

#[test]
fn the_moon_level_is_voxel_flights_moon() {
    let world = level("voxel_moon");
    let terrain = terrain(&world);
    assert_eq!(terrain.planet_radius, 1_737_400.0);
    assert_eq!(helio_component::feature_recipe(&terrain.features), FeatureRecipe::moon());
    assert!(!one!(world, AtmosphereComponent).unwrap().enabled, "no air");
    assert!(one!(world, GlobalFogComponent).is_none_or(|fog| !fog.enabled), "no haze");
    assert_eq!(world.query::<&FoliageComponent>().count(), 0, "no plants");
}

#[test]
fn the_desert_level_is_voxel_flights_desert() {
    let world = level("voxel_desert");
    let terrain = terrain(&world);
    assert_eq!(helio_component::feature_recipe(&terrain.features), FeatureRecipe::desert());
    // Dusty air, a few high clouds, a warm dust haze.
    let earth = helio_pass_sky::AtmosphereComponent::earth();
    let flight = helio_pass_sky::AtmosphereComponent {
        mie_scattering: [9.0e-3, 8.0e-3, 6.5e-3],
        mie_absorption: [6.0e-3, 7.0e-3, 9.0e-3],
        mie_scale_height: 1.6,
        ground_albedo: [0.45, 0.38, 0.28],
        ..earth
    }
    .with_clouds(CloudLayer { bottom_km: 4.5, top_km: 6.0, coverage: 0.2, kind: 0.4, ..CloudLayer::cumulus() });
    let sky = one!(world, AtmosphereComponent).unwrap().to_row();
    assert_eq!((sky.mie_scattering, sky.mie_absorption, sky.mie_scale_height, sky.ground_albedo), (flight.mie_scattering, flight.mie_absorption, flight.mie_scale_height, flight.ground_albedo));
    assert_eq!(sky.clouds(), flight.clouds());
    let fog = one!(world, GlobalFogComponent).unwrap();
    assert_eq!(fog.medium.to_medium(), flight_haze(0.0012, 0.003, 0.7, [0.95, 0.84, 0.68], 6_371_000.0));
}
