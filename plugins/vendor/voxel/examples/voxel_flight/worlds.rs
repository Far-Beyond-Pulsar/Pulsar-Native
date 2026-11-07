//! The flight's worlds (`HELIO_VOXEL_FLIGHT_PRESET=earth|moon|desert`):
//! each one's planet (layer stack, radius, features), its look (materials
//! past the built-in ones), sky, haze and ground cover, all through the
//! engine's generic APIs: the stress test exercises what voxel_craft does,
//! at Earth and Moon scale.
//!
//! * Earth: broadleaf and birch woods in the lowlands, conifers up to the
//!   tree line, boulders on rock and scree, bushes in the meadows, shrubs
//!   and rocks on dry land; the engine's meadow with tall stands in a
//!   breeze; Earth's air with cumulus and a ground haze.
//! * Moon (1737 km): no air, no plants: simple craters of 1 to 36 m with
//!   lumpy rims of the ground they hit (fresh ones bright, with blocks
//!   thrown out), boulders, and boulder fields on fresh ejecta.
//! * Desert: sparse shrubs, rocks and sandstone boulders on the dunes;
//!   dusty air with a few high clouds and a warm haze.

use helio_pass_voxel_planet::feature_recipe::FeatureRecipe;
use helio_pass_voxel_planet::layers::TerrainLayers;
use helio_pass_voxel_planet::meadow::Meadow;
use helio_pass_voxel_planet::planet::EARTH_RADIUS;
use helio_pass_voxel_planet::terrain::TerrainAppearance;

/// The Moon's mean radius (m).
const MOON_RADIUS: f64 = 1_737_400.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Earth,
    Moon,
    Desert,
}

impl Preset {
    /// `HELIO_VOXEL_FLIGHT_PRESET` (Earth).
    pub fn from_env() -> Self {
        match std::env::var("HELIO_VOXEL_FLIGHT_PRESET").as_deref() {
            Ok("moon") => Preset::Moon,
            Ok("desert") => Preset::Desert,
            Ok("earth") | Err(_) => Preset::Earth,
            Ok(other) => panic!("unknown preset {other}"),
        }
    }

    pub fn stack(self) -> TerrainLayers {
        match self {
            Preset::Earth => TerrainLayers::earth(),
            Preset::Moon => TerrainLayers::moon(),
            Preset::Desert => TerrainLayers::desert(),
        }
    }

    pub fn radius_m(self) -> f64 {
        match self {
            Preset::Moon => MOON_RADIUS,
            Preset::Earth | Preset::Desert => EARTH_RADIUS,
        }
    }

    /// What grows and lies on the ground (the engine's preset recipes).
    pub fn features(self) -> FeatureRecipe {
        match self {
            Preset::Earth => FeatureRecipe::earth(),
            Preset::Moon => FeatureRecipe::moon(),
            Preset::Desert => FeatureRecipe::desert(),
        }
    }

    /// The world's look: the generator's (`base`) with the materials its
    /// features add.
    pub fn appearance(self, base: TerrainAppearance) -> TerrainAppearance {
        FeatureRecipe::appearance(base)
    }

    /// The air (none on the Moon), around a planet of `radius` m. Its
    /// clouds are smooth volumes, or with
    /// `HELIO_VOXEL_FLIGHT_CLOUDS=voxel[:cell_m[,fill]]` voxels (32 m cells
    /// filled from density 0.3). `HELIO_VOXEL_FLIGHT_CLOUD_SEED` picks
    /// another weather.
    pub fn sky(self, radius: f64) -> Option<helio_pass_sky::AtmosphereRow> {
        use helio_pass_sky::{AtmosphereRow, CloudLayer, CloudRepresentation, PlanetWeather};
        let clouds = std::env::var_os("HELIO_VOXEL_FLIGHT_NO_CLOUDS").is_none();
        let representation = match std::env::var("HELIO_VOXEL_FLIGHT_CLOUDS").ok().as_deref() {
            Some(v) if v.starts_with("voxel") => {
                let args: Vec<f32> = v.split_once(':').map_or(Vec::new(), |(_, a)| {
                    a.split(',').map(|x| x.trim().parse().unwrap()).collect()
                });
                CloudRepresentation::Voxels {
                    cell_m: args.first().copied().unwrap_or(32.0),
                    fill: args.get(1).copied().unwrap_or(0.3),
                }
            }
            _ => CloudRepresentation::Volumetric,
        };
        let seed = std::env::var("HELIO_VOXEL_FLIGHT_CLOUD_SEED")
            .ok()
            .map(|v| v.parse().unwrap());
        let layer = |layer: CloudLayer| CloudLayer {
            representation,
            weather: PlanetWeather {
                seed: seed.unwrap_or(layer.weather.seed),
                ..layer.weather
            },
            ..layer
        };
        match self {
            Preset::Moon => None,
            // Earth's air with cumulus, under weather (seed 2) that leaves
            // the spawn broken cumulus rather than inside a deck.
            Preset::Earth => {
                let sky = AtmosphereRow::earth().around_planet([0.0; 3], radius);
                let cumulus = CloudLayer::cumulus();
                let cumulus = CloudLayer {
                    weather: PlanetWeather {
                        seed: 2,
                        ..cumulus.weather
                    },
                    ..cumulus
                };
                Some(if clouds {
                    sky.with_clouds(layer(cumulus))
                } else {
                    sky
                })
            }
            // Dusty air: more and redder aerosols, a sandy ground bouncing
            // light into it, a few high clouds.
            Preset::Desert => {
                let earth = AtmosphereRow::earth();
                let sky = AtmosphereRow {
                    mie_scattering: [9.0e-3, 8.0e-3, 6.5e-3],
                    mie_absorption: [6.0e-3, 7.0e-3, 9.0e-3],
                    mie_scale_height: 1.6,
                    ground_albedo: [0.45, 0.38, 0.28],
                    ..earth
                }
                .around_planet([0.0; 3], radius);
                let high = CloudLayer {
                    bottom_km: 4.5,
                    top_km: 6.0,
                    coverage: 0.2,
                    kind: 0.4,
                    ..CloudLayer::cumulus()
                };
                Some(if clouds {
                    sky.with_clouds(layer(high))
                } else {
                    sky
                })
            }
        }
    }

    /// A ground haze thinning with altitude over the planet (none on the
    /// Moon), unless `HELIO_VOXEL_FLIGHT_NO_HAZE`.
    pub fn haze(self, radius: f64) -> Option<helio_pass_volumetric_fog::GlobalFogRow> {
        use helio_pass_volumetric_fog::GlobalFogRow;
        if std::env::var_os("HELIO_VOXEL_FLIGHT_NO_HAZE").is_some() {
            return None;
        }
        let haze = match self {
            Preset::Moon => return None,
            Preset::Earth => GlobalFogRow {
                mode: 1,
                extinction: 0.0006,
                height_falloff: 0.004,
                anisotropy: 0.6,
                ..Default::default()
            },
            // Dust: thicker, warm, hugging the ground.
            Preset::Desert => GlobalFogRow {
                mode: 1,
                extinction: 0.0012,
                height_falloff: 0.003,
                anisotropy: 0.7,
                albedo: [0.95, 0.84, 0.68],
                ..Default::default()
            },
        };
        Some(haze.around_planet([0.0; 3], radius))
    }

    /// The light the sky gives where it has no air to scatter it (the
    /// renderer's ambient): none on the Moon. Airy worlds' skies light
    /// themselves.
    pub fn ambient(self) -> ([f32; 3], f32) {
        match self {
            Preset::Moon => ([0.0; 3], 0.0),
            Preset::Earth | Preset::Desert => ([0.45, 0.58, 0.8], 0.35),
        }
    }

    /// The camera's exposure: voxel_craft's under air; on the Moon a
    /// camera exposes for sunlit regolith against a black sky (no sky
    /// lights the shade, nothing dims the sun).
    pub fn exposure(self) -> f32 {
        match self {
            Preset::Moon => 3.5,
            Preset::Earth | Preset::Desert => 2.0,
        }
    }

    /// The spawn's ground is level within this many metres 30 m around it:
    /// on the Moon a plain between craters (the others keep the first land
    /// found, as before).
    pub fn level_spawn_m(self) -> Option<f64> {
        match self {
            Preset::Moon => Some(1.5),
            Preset::Earth | Preset::Desert => None,
        }
    }

    /// Foliage over the ground, unless `HELIO_VOXEL_FLIGHT_NO_FOLIAGE`:
    /// Earth's meadows with stands of tall grass in a fresh breeze.
    pub fn meadow(self) -> Option<Meadow> {
        if std::env::var_os("HELIO_VOXEL_FLIGHT_NO_FOLIAGE").is_some() {
            return None;
        }
        match self {
            Preset::Earth => Some(
                helio_pass_voxel_planet::meadow::meadow()
                    .with_tall_stands()
                    .with_wind([1.0, 0.35], 4.5, 0.8),
            ),
            Preset::Moon | Preset::Desert => None,
        }
    }
}
