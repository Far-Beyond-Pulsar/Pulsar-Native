//! The planet's recipe: a small, rolling, Minecraft-scaled world.
//!
//! Terrain is the voxel planet's own layered generator (`helio.terrain`), set
//! up as data: warped rolling hills, mountain ranges with snow caps, metre
//! detail, caves, and rules that pick each ground block from height, slope,
//! moisture and depth (meadows, deserts, beaches, rock, strata).

use crate::config::{PLANET_RADIUS_M, VOXEL_M, WORLD_SEED};
use helio_pass_voxel_planet::layers::{
    self, Climate, Layer, LayerKind, LayerMask, MaterialRule, MaterialStyle, TerrainLayers,
};
use helio_pass_voxel_planet::terrain::TerrainSource;
use helio_pass_voxel_planet::volumes::{Caves, Volumes};
use helio_pass_voxel_planet::PlanetRecipe;

fn rule(material: &str, f: impl FnOnce(&mut MaterialRule)) -> MaterialRule {
    let mut r = MaterialRule::new(material);
    f(&mut r);
    r
}

/// The layer stack and material rules of the world.
pub fn terrain_layers() -> TerrainLayers {
    use LayerKind::*;
    let layer = |kind, f: &dyn Fn(&mut Layer)| {
        let mut l = Layer::new(kind);
        f(&mut l);
        l
    };
    TerrainLayers {
        layers: vec![
            // Bends everything after it: no straight ranges or round hills.
            layer(Warp, &|l| l.scale_km = 3.0),
            // Broad swells and valleys.
            layer(Hills, &|l| {
                l.height_m = 34.0;
                l.scale_km = 3.2;
                l.octaves = 4;
                l.persistence = 0.5;
            }),
            // Rolling knolls near the player.
            layer(Hills, &|l| {
                l.height_m = 9.0;
                l.scale_km = 0.42;
                l.octaves = 4;
                l.persistence = 0.5;
            }),
            // Mountain ranges in regions over the land.
            layer(Mountains, &|l| {
                l.height_m = 120.0;
                l.scale_km = 2.2;
                l.octaves = 6;
                l.persistence = 0.5;
                l.region_km = 9.0;
                l.coverage = 0.28;
            }),
            // Block-scale detail: one-block bumps and pits.
            layer(Roughness, &|l| {
                l.scale_km = 0.064;
                l.octaves = 5;
                l.ratio = 0.045;
                l.mask = LayerMask::Everywhere;
            }),
        ],
        // Minecraft-scaled cave systems: shallow, many entrances.
        volumes: Volumes {
            caves: Caves {
                spacing_m: 120.0,
                share: 0.6,
                depth_min_m: 12.0,
                depth_max_m: 45.0,
                max_depth_m: 90.0,
                cover_m: 4.0,
                chamber_min_m: 6.0,
                chamber_max_m: 16.0,
                tunnel_min_m: 1.6,
                tunnel_max_m: 3.0,
                entrances: 0.45,
                ..Caves::default()
            },
            ..Volumes::none()
        },
        materials: MaterialStyle::Rules,
        // Half wet everywhere (a 16 km world under one sky).
        climate: Climate::uniform(20.0, 1.0),
        soil_depth_m: 4.0,
        surface: "Grass".into(),
        soil: "Dirt".into(),
        rock: "Stone".into(),
        rules: vec![
            // Peaks.
            rule("Snow", |r| {
                r.min_height_m = 92.0;
                r.max_depth_m = 1.2;
            }),
            // Cliffs and steep slopes are bare rock.
            rule("Stone", |r| {
                r.min_slope = 1.25;
                r.max_depth_m = 8.0;
            }),
            // Scree where the ground steepens.
            rule("Gravel", |r| {
                r.min_slope = 0.55;
                r.max_depth_m = 1.2;
                r.patch_km = 0.05;
                r.patch_share = 0.5;
            }),
            // Deserts: dry land is sand over sandstone (no beaches: there is no sea, and a height rule samples once per 4x4x4 leaf, so it patched meadows with sand).
            rule("Sand", |r| {
                r.max_moisture = 0.32;
                r.max_depth_m = 4.0;
            }),
            rule("Sandstone", |r| {
                r.max_moisture = 0.32;
                r.max_depth_m = 14.0;
            }),
            // Clay in the lowest, wettest ground.
            rule("Clay", |r| {
                r.max_height_m = 6.0;
                r.min_moisture = 0.7;
                r.min_depth_m = 1.0;
                r.max_depth_m = 3.0;
            }),
            // Turf over soil.
            rule("Grass", |r| r.max_depth_m = 0.9),
            rule("Dirt", |r| r.max_depth_m = 3.9),
            // Pockets of andesite and gravel in the rock.
            rule("DarkStone", |r| {
                r.min_depth_m = 4.0;
                r.patch_km = 0.045;
                r.patch_share = 0.18;
            }),
            rule("Gravel", |r| {
                r.min_depth_m = 5.0;
                r.patch_km = 0.03;
                r.patch_share = 0.06;
            }),
        ],
        ..TerrainLayers::earth()
    }
}

/// The world: a 16 km planet of 1 m blocks.
pub fn recipe() -> PlanetRecipe {
    PlanetRecipe {
        radius_m: PLANET_RADIUS_M,
        voxel_size_m: VOXEL_M,
        terrain: TerrainSource {
            generator: layers::ID.into(),
            version: 0,
            seed: WORLD_SEED,
            settings: serde_json::to_string(&terrain_layers()).expect("terrain layers serialize"),
        },
        features: crate::world::decorate::features(),
        ..PlanetRecipe::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recipe_builds_a_valid_planet() {
        let planet = helio_pass_voxel_planet::Planet::new(recipe()).expect("planet");
        assert!(planet.grid().cells() > 1000);
        terrain_layers().validate().expect("valid layers");
    }
}
