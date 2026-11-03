//! A world's features as an author describes them: what grows (the
//! engine's [`flora`]: trees, bushes, shrubs, cacti, boulders, craters) and
//! where (a [`crate::features::Spawn`]'s rules), sizes in metres. It grows
//! the [`FeatureSet`] of any voxel size and world seed ([`FeatureRecipe::grow`]),
//! so a level saves a few lines per spawn instead of every stamp's shapes,
//! and the same recipe makes the same features wherever it is grown.
//!
//! A spawn's stamps are its growths' variants in order; variant `v` of a
//! growth of seed `s` on a world of seed `w` grows from `w ^ s << 40 ^
//! v * 0x9e3779b9`. Metres become cells as the nearest whole number (at
//! least one): spacings (at most [`MAX_SPACING`]), relief, sink and canopy
//! height.
//!
//! [`FeatureRecipe::earth`], [`FeatureRecipe::moon`] and
//! [`FeatureRecipe::desert`] are setups of it: woods to a tree line, a
//! cratered moon, a stony desert.

use crate::features::{FeatureSet, Spawn, MAX_SPACING};
use crate::flora::{self, Species};
use crate::terrain::{material, MaterialAppearance, TerrainAppearance};
use serde::{Deserialize, Serialize};

/// What a [`Growth`] grows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Form {
    /// A tree of its species: `material` wood, `second_material` leaves.
    #[default]
    Tree,
    /// A bush: a few low clusters of `material` leaves.
    Bush,
    /// A dry-land shrub: `material` stems, `second_material` leaves.
    Shrub,
    /// A cactus of `material`.
    Cactus,
    /// A boulder of `material`, `min_size_m` to `max_size_m` across; a
    /// third of the variants (by seed) of `second_material` when it is not
    /// air.
    Boulder,
    /// An impact crater `min_size_m` to `max_size_m` across: a bowl inside
    /// a rim of `material`, `blocks` lumps of `second_material` thrown out.
    Crater,
}

/// Stamps grown by one of the engine's flora generators: `variants` of
/// them, from seeds of the world's and `seed`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Growth {
    pub form: Form,
    /// A tree's species.
    pub species: Species,
    /// The materials its form names (wood, leaves, body, rim, rocks).
    pub material: u32,
    pub second_material: u32,
    /// Size range (m): a boulder's, a crater's diameter.
    pub min_size_m: f64,
    pub max_size_m: f64,
    /// Blocks a crater throws out.
    pub blocks: u32,
    /// Salt of its variants' seeds: growths of different seeds differ.
    pub seed: u64,
    /// Stamps grown.
    pub variants: u32,
}

impl Default for Growth {
    fn default() -> Self {
        Self {
            form: Form::Tree,
            species: Species::Oak,
            material: material::WOOD,
            second_material: material::LEAVES,
            min_size_m: 0.5,
            max_size_m: 1.3,
            blocks: 0,
            seed: 0,
            variants: 6,
        }
    }
}

impl Growth {
    /// `variants` trees of `species`.
    pub fn tree(species: Species, wood: u32, leaves: u32, seed: u64) -> Self {
        Self {
            form: Form::Tree,
            species,
            material: wood,
            second_material: leaves,
            seed,
            ..Self::default()
        }
    }
    pub fn bush(leaves: u32, seed: u64) -> Self {
        Self {
            form: Form::Bush,
            material: leaves,
            second_material: 0,
            seed,
            ..Self::default()
        }
    }
    pub fn shrub(wood: u32, leaves: u32, seed: u64) -> Self {
        Self {
            form: Form::Shrub,
            material: wood,
            second_material: leaves,
            seed,
            ..Self::default()
        }
    }
    pub fn cactus(body: u32, seed: u64) -> Self {
        Self {
            form: Form::Cactus,
            material: body,
            second_material: 0,
            seed,
            ..Self::default()
        }
    }
    /// Boulders `size_m` across, a third of them of `other` (0: none).
    pub fn boulder(rock: u32, other: u32, size_m: [f64; 2], seed: u64) -> Self {
        Self {
            form: Form::Boulder,
            material: rock,
            second_material: other,
            min_size_m: size_m[0],
            max_size_m: size_m[1],
            seed,
            ..Self::default()
        }
    }
    pub fn crater(rim: u32, rocks: u32, blocks: u32, diameter_m: [f64; 2], seed: u64) -> Self {
        Self {
            form: Form::Crater,
            material: rim,
            second_material: rocks,
            min_size_m: diameter_m[0],
            max_size_m: diameter_m[1],
            blocks,
            seed,
            ..Self::default()
        }
    }

    /// Its stamps on a world of seed `world_seed`, in cells of `voxel` m.
    pub fn stamps(&self, voxel: f64, world_seed: u64) -> Vec<crate::stamp::Stamp> {
        (0..u64::from(self.variants))
            .map(|v| {
                let seed = world_seed ^ (self.seed << 40) ^ v.wrapping_mul(0x9e37_79b9);
                match self.form {
                    Form::Tree => flora::tree(
                        self.species,
                        self.material,
                        self.second_material,
                        seed,
                        voxel,
                    ),
                    Form::Bush => flora::bush(self.material, seed, voxel),
                    Form::Shrub => flora::shrub(self.material, self.second_material, seed, voxel),
                    Form::Cactus => flora::cactus(self.material, seed, voxel),
                    Form::Boulder => {
                        let rock = if self.second_material != 0 && seed % 3 == 0 {
                            self.second_material
                        } else {
                            self.material
                        };
                        flora::boulder_sized(rock, seed, voxel, [self.min_size_m, self.max_size_m])
                    }
                    Form::Crater => flora::crater(
                        self.material,
                        self.second_material,
                        self.blocks,
                        seed,
                        voxel,
                        [self.min_size_m, self.max_size_m],
                    ),
                }
            })
            .collect()
    }
}

/// Where a spawn's growths stand ([`Spawn`] in metres).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpawnRecipe {
    pub name: String,
    /// Its stamps: every growth's variants.
    pub growths: Vec<Growth>,
    /// Metres a side of the square each candidate lies in. Spawns go from
    /// the widest spacing to the narrowest.
    pub spacing_m: f64,
    /// Share of candidates that grow (before the field).
    pub density: f32,
    /// The noise field gating it: its scale (m; 0: none) and the threshold
    /// it must pass (-1 to 1).
    pub field_scale_m: f64,
    pub field_threshold: f32,
    /// Ground surface materials it grows on (empty: any).
    pub surfaces: Vec<u32>,
    /// Ground heights over the datum it grows between (m), how far they
    /// wander (m) and over what distance (m; 0: they do not).
    pub min_height_m: f64,
    pub max_height_m: f64,
    pub height_wander_m: f64,
    pub height_wander_scale_m: f64,
    /// The climate it grows in: annual mean temperature (°C) and moisture
    /// (0 dry to 1 wet) at its ground (the terrain's `climate`).
    pub min_temperature_c: f64,
    pub max_temperature_c: f64,
    pub min_moisture: f64,
    pub max_moisture: f64,
    /// Most rise (m) between the ground at its anchor and its footprint's
    /// corners; 0: any.
    pub max_relief_m: f64,
    /// Metres its anchor lies under the ground's top.
    pub sink_m: f64,
    /// Its far form's material (0: none) and the canopy's height (m; 0:
    /// its stamps' mean height).
    pub far_material: u32,
    pub far_height_m: f64,
}

impl Default for SpawnRecipe {
    fn default() -> Self {
        Self {
            name: String::new(),
            growths: Vec::new(),
            spacing_m: 8.0,
            density: 0.5,
            field_scale_m: 0.0,
            field_threshold: -1.0,
            surfaces: Vec::new(),
            min_height_m: -1e9,
            max_height_m: 1e9,
            height_wander_m: 0.0,
            height_wander_scale_m: 0.0,
            min_temperature_c: -1e3,
            max_temperature_c: 1e3,
            min_moisture: 0.0,
            max_moisture: 1.0,
            max_relief_m: 0.0,
            sink_m: 0.0,
            far_material: 0,
            far_height_m: 0.0,
        }
    }
}

/// A world's features: spawns in order (see the module doc).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FeatureRecipe {
    pub spawns: Vec<SpawnRecipe>,
}

/// Materials the presets' features add past the built-in ones: conifer
/// needles, birch bark, a dry shrub's grey-green leaves and its wood
/// ([`FeatureRecipe::appearance`]).
pub const NEEDLES: u32 = 22;
pub const BIRCH_BARK: u32 = 23;
pub const SAGE: u32 = 24;
pub const DRY_WOOD: u32 = 25;

impl FeatureRecipe {
    pub fn is_empty(&self) -> bool {
        self.spawns.is_empty()
    }

    /// The feature set it grows in cells of `voxel` m on a world of seed
    /// `seed`.
    pub fn grow(&self, voxel: f64, seed: u64) -> FeatureSet {
        let cells = |m: f64| (m / voxel).round().max(1.0) as u32;
        let mut set = FeatureSet::default();
        for s in &self.spawns {
            let first = set.stamps.len() as u32;
            for growth in &s.growths {
                set.stamps.extend(growth.stamps(voxel, seed));
            }
            set.spawns.push(Spawn {
                name: s.name.clone(),
                stamps: (first..set.stamps.len() as u32).collect(),
                spacing: cells(s.spacing_m).min(MAX_SPACING),
                density: s.density,
                field_scale_m: s.field_scale_m,
                field_threshold: s.field_threshold,
                surfaces: s.surfaces.clone(),
                heights_m: [s.min_height_m, s.max_height_m],
                height_wander_m: [s.height_wander_m, s.height_wander_scale_m],
                temperature_c: [s.min_temperature_c, s.max_temperature_c],
                moisture: [s.min_moisture, s.max_moisture],
                max_relief: if s.max_relief_m > 0.0 {
                    cells(s.max_relief_m)
                } else {
                    u32::MAX
                },
                sink: (s.sink_m / voxel).round() as i32,
                far_material: s.far_material,
                far_height: if s.far_height_m > 0.0 {
                    cells(s.far_height_m)
                } else {
                    0
                },
            });
        }
        set
    }

    /// The presets by name: "earth", "moon", "desert", and "none".
    pub const PRESETS: [&'static str; 4] = ["earth", "moon", "desert", "none"];

    /// A preset by name (case-insensitive).
    pub fn preset(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "earth" => Some(Self::earth()),
            "moon" => Some(Self::moon()),
            "desert" => Some(Self::desert()),
            "none" => Some(Self::default()),
            _ => None,
        }
    }

    /// Broadleaf and birch woods where it is mild and wet, conifers where
    /// it is cold up to the tree line (the climate's: lower toward the
    /// poles), boulders on rock, scree and snow, bushes in the meadows,
    /// shrubs and rocks on dry land.
    pub fn earth() -> Self {
        use material::*;
        // Broadleaves give way to conifers under this annual mean
        // temperature (°C), and those stop at the tree line, below the
        // snow; both lines wander a few hundred metres over half a
        // kilometre (no contour lines). Woods need rain.
        let (conifer_line, tree_line, woods_moisture) = (7.5, 0.5, 0.36);
        let ([lowland_wander, lowland_scale], [tree_line_wander, tree_line_scale]) =
            ([150.0, 500.0], [200.0, 450.0]);
        let meadow = vec![GRASS, DIRT];
        let forest = |name: &str,
                      growths: Vec<Growth>,
                      field_scale_m: f64,
                      field_threshold: f32,
                      far_material: u32| SpawnRecipe {
            name: name.into(),
            growths,
            spacing_m: 9.0,
            density: 0.85,
            field_scale_m,
            field_threshold,
            surfaces: meadow.clone(),
            min_temperature_c: conifer_line,
            min_moisture: woods_moisture,
            height_wander_m: lowland_wander,
            height_wander_scale_m: lowland_scale,
            max_relief_m: 3.0,
            far_material,
            ..SpawnRecipe::default()
        };
        let broadleaf = vec![
            Growth::tree(Species::Oak, WOOD, LEAVES, 1),
            Growth::tree(Species::Oak, WOOD, LEAVES, 2),
            Growth::tree(Species::DarkOak, WOOD, LEAVES, 3),
        ];
        let conifers = vec![
            Growth::tree(Species::Spruce, WOOD, NEEDLES, 5),
            Growth::tree(Species::Pine, WOOD, NEEDLES, 6),
        ];
        Self {
            spawns: vec![
                forest("broadleaf woods", broadleaf, 2_400.0, 0.05, LEAVES),
                forest(
                    "birch woods",
                    vec![Growth::tree(Species::Birch, BIRCH_BARK, LEAVES, 4)],
                    900.0,
                    0.45,
                    LEAVES,
                ),
                SpawnRecipe {
                    min_temperature_c: tree_line,
                    max_temperature_c: conifer_line,
                    min_moisture: woods_moisture - 0.08,
                    height_wander_m: tree_line_wander,
                    height_wander_scale_m: tree_line_scale,
                    ..forest("conifers", conifers, 1_600.0, -0.25, NEEDLES)
                },
                SpawnRecipe {
                    name: "boulders".into(),
                    growths: vec![Growth::boulder(STONE, DARK_STONE, [0.5, 1.3], 8)],
                    spacing_m: 7.0,
                    density: 0.1,
                    surfaces: vec![STONE, DARK_STONE, GRAVEL, SNOW, GRASS],
                    ..SpawnRecipe::default()
                },
                SpawnRecipe {
                    name: "dry rocks".into(),
                    growths: vec![Growth::boulder(SANDSTONE, 0, [0.3, 0.9], 11)],
                    spacing_m: 7.0,
                    density: 0.08,
                    surfaces: vec![SAND, SANDSTONE],
                    ..SpawnRecipe::default()
                },
                SpawnRecipe {
                    name: "dry shrubs".into(),
                    growths: vec![Growth::shrub(DRY_WOOD, SAGE, 10)],
                    spacing_m: 5.0,
                    density: 0.18,
                    field_scale_m: 300.0,
                    field_threshold: -0.4,
                    surfaces: vec![SAND],
                    max_relief_m: 1.0,
                    ..SpawnRecipe::default()
                },
                SpawnRecipe {
                    name: "bushes".into(),
                    growths: vec![Growth::bush(LEAVES, 9)],
                    spacing_m: 3.0,
                    density: 0.12,
                    field_scale_m: 200.0,
                    field_threshold: -0.2,
                    surfaces: meadow.clone(),
                    min_temperature_c: tree_line,
                    height_wander_m: tree_line_wander,
                    height_wander_scale_m: tree_line_scale,
                    max_relief_m: 1.0,
                    ..SpawnRecipe::default()
                },
            ],
        }
    }

    /// Simple craters from 36 m down to a metre, many more small than
    /// large, rimmed with the ground they hit (a third of them fresh:
    /// bright ejecta and thrown blocks); boulders, thick on fresh ejecta.
    pub fn moon() -> Self {
        use material::*;
        let mut spawns = Vec::new();
        for (n, (diameter, spacing_m, density, blocks)) in [
            ([14.0, 36.0], 100.0, 0.3, 14),
            ([5.0, 14.0], 40.0, 0.35, 8),
            ([1.2, 5.0], 12.0, 0.35, 0),
        ]
        .into_iter()
        .enumerate()
        {
            let seed = 20 + 4 * n as u64;
            let craters =
                |name: String, growth: Growth, surfaces: Vec<u32>, density: f32| SpawnRecipe {
                    name,
                    growths: vec![growth],
                    spacing_m,
                    density,
                    surfaces,
                    // Gentle ground: a crater's rim stays level.
                    max_relief_m: diameter[1] * 0.12,
                    ..SpawnRecipe::default()
                };
            let size = format!("{:.0}-{:.0} m", diameter[0], diameter[1]);
            // Fresh craters are rarer: a third of the young-looking ones.
            spawns.push(craters(
                format!("fresh craters {size}"),
                Growth::crater(EJECTA, BOULDER, blocks, diameter, seed + 2),
                vec![REGOLITH, MARE, EJECTA],
                density * 0.3,
            ));
            spawns.push(craters(
                format!("highland craters {size}"),
                Growth::crater(REGOLITH, BOULDER, blocks / 2, diameter, seed),
                vec![REGOLITH, EJECTA],
                density,
            ));
            spawns.push(craters(
                format!("mare craters {size}"),
                Growth::crater(MARE, BASALT, blocks / 2, diameter, seed + 1),
                vec![MARE],
                density,
            ));
        }
        spawns.push(SpawnRecipe {
            name: "boulders".into(),
            growths: vec![Growth::boulder(BOULDER, 0, [0.3, 1.6], 40)],
            spacing_m: 6.0,
            density: 0.08,
            ..SpawnRecipe::default()
        });
        spawns.push(SpawnRecipe {
            name: "boulder fields".into(),
            growths: vec![Growth::boulder(BOULDER, 0, [0.1, 0.45], 41)],
            spacing_m: 2.5,
            density: 0.12,
            surfaces: vec![EJECTA],
            ..SpawnRecipe::default()
        });
        Self { spawns }
    }

    /// Sandstone boulders, rocks and sparse shrubs over the dunes.
    pub fn desert() -> Self {
        use material::*;
        Self {
            spawns: vec![
                SpawnRecipe {
                    name: "sandstone boulders".into(),
                    growths: vec![Growth::boulder(SANDSTONE, 0, [1.5, 3.5], 50)],
                    spacing_m: 36.0,
                    density: 0.25,
                    field_scale_m: 900.0,
                    field_threshold: -0.2,
                    ..SpawnRecipe::default()
                },
                SpawnRecipe {
                    name: "rocks".into(),
                    growths: vec![Growth::boulder(STONE, DARK_STONE, [0.25, 0.9], 51)],
                    spacing_m: 8.0,
                    density: 0.07,
                    ..SpawnRecipe::default()
                },
                SpawnRecipe {
                    name: "shrubs".into(),
                    growths: vec![Growth::shrub(DRY_WOOD, SAGE, 52)],
                    spacing_m: 6.0,
                    density: 0.12,
                    field_scale_m: 450.0,
                    field_threshold: -0.35,
                    max_relief_m: 1.0,
                    ..SpawnRecipe::default()
                },
            ],
        }
    }

    /// `base` with the look of the materials the presets add
    /// ([`NEEDLES`], [`BIRCH_BARK`], [`SAGE`], [`DRY_WOOD`]), leaves varying
    /// by patches over the land as the turf does.
    pub fn appearance(mut base: TerrainAppearance) -> TerrainAppearance {
        let unit = |c: [u8; 3]| c.map(|v| f32::from(v) / 255.0);
        let look =
            |c: [u8; 3], roughness: f32, patches: Option<[[u8; 3]; 3]>, transmission: f32| {
                let [r, g, b] = unit(c);
                MaterialAppearance {
                    colour: [r, g, b, roughness],
                    patches: patches.map(|p| p.map(unit)),
                    transmission,
                    ..MaterialAppearance::default()
                }
            };
        base.materials[material::LEAVES as usize].patches =
            Some([[86, 112, 50], [59, 102, 52], [40, 80, 40]].map(unit));
        base.materials[NEEDLES as usize] = look(
            [42, 68, 48],
            0.9,
            Some([[62, 84, 54], [42, 68, 48], [30, 54, 42]]),
            0.5,
        );
        base.materials[BIRCH_BARK as usize] = look([212, 208, 196], 0.8, None, 0.0);
        base.materials[SAGE as usize] = look(
            [146, 150, 116],
            0.92,
            Some([[170, 164, 124], [146, 150, 116], [116, 126, 96]]),
            0.4,
        );
        base.materials[DRY_WOOD as usize] = look([110, 94, 78], 0.9, None, 0.0);
        base
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layers::TerrainLayers;
    use crate::planet::{Planet, PlanetRecipe, EARTH_RADIUS};

    /// Every preset grows a valid set of its planet, the same set from the
    /// same voxel size and seed, and another from another seed.
    #[test]
    fn presets_grow_valid_planets() {
        for (recipe, stack, radius) in [
            (FeatureRecipe::earth(), TerrainLayers::earth(), EARTH_RADIUS),
            (FeatureRecipe::moon(), TerrainLayers::moon(), 1_737_400.0),
            (
                FeatureRecipe::desert(),
                TerrainLayers::desert(),
                EARTH_RADIUS,
            ),
        ] {
            let set = recipe.grow(0.1, 7);
            assert_eq!(set.spawns.len(), recipe.spawns.len());
            assert_eq!(set, recipe.grow(0.1, 7));
            assert_ne!(
                set.stamps,
                recipe.grow(0.1, 8).stamps,
                "another world grows other variants"
            );
            Planet::new(PlanetRecipe {
                radius_m: radius,
                terrain: stack.source(7),
                features: set,
                ..Default::default()
            })
            .unwrap();
        }
    }

    /// A recipe is a few kilobytes of JSON and comes back the same; metres
    /// become cells of the voxel size.
    #[test]
    fn recipes_round_trip_and_scale_with_the_voxel() {
        let earth = FeatureRecipe::earth();
        let json = serde_json::to_string(&earth).unwrap();
        assert!(json.len() < 8_000, "{} bytes", json.len());
        assert!(json.starts_with('['), "a recipe is its list of spawns");
        assert_eq!(serde_json::from_str::<FeatureRecipe>(&json).unwrap(), earth);
        let (fine, coarse) = (earth.grow(0.1, 7), earth.grow(0.5, 7));
        assert_eq!((fine.spawns[0].spacing, coarse.spawns[0].spacing), (90, 18));
        assert_eq!(
            (fine.spawns[0].max_relief, fine.spawns[3].max_relief),
            (30, u32::MAX)
        );
        let empty: FeatureRecipe = serde_json::from_str("[{}]").unwrap();
        assert_eq!(
            empty.spawns[0],
            SpawnRecipe::default(),
            "every field has a default"
        );
    }
}
