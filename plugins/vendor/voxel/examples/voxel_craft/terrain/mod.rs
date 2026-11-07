//! The terrain: its recipe, surface queries and biomes.

pub mod recipe;

use helio_pass_voxel_planet::terrain::material;
use helio_pass_voxel_planet::{Cell, Planet};

/// The terrain's topmost block of a column (edits and features aside: what
/// the world was generated with, so features decided on it come out the
/// same whenever they are).
#[derive(Clone, Copy, Debug)]
pub struct Ground {
    pub cell: Cell,
    /// The material id of the block.
    pub material: u32,
}

/// The terrain's top block of column `(i, j)` of `face`.
pub fn ground(planet: &Planet, face: u8, i: i32, j: i32) -> Option<Ground> {
    let cell = Cell::new(face, i, j, planet.terrain_top(face, i, j) - 1);
    Some(Ground {
        cell,
        material: planet.terrain_material(cell),
    })
}

/// Heights of the ground around a column, for slope tests: the largest
/// difference from the column's own top among its neighbours `d` cells away.
pub fn relief(planet: &Planet, face: u8, i: i32, j: i32, d: i32) -> i32 {
    let here = planet.terrain_top(face, i, j);
    [(d, 0), (-d, 0), (0, d), (0, -d)]
        .into_iter()
        .map(|(di, dj)| (planet.terrain_top(face, i + di, j + dj) - here).abs())
        .max()
        .unwrap_or(0)
}

/// What kind of place a ground block says it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Biome {
    /// Grass: meadows and forests.
    Temperate,
    /// Sand and sandstone.
    Desert,
    /// Snow caps and cold slopes.
    Snow,
    /// Bare rock and scree.
    Rock,
}

impl Biome {
    pub fn of(ground_material: u32) -> Biome {
        match ground_material {
            material::GRASS | material::DIRT | material::CLAY => Biome::Temperate,
            material::SAND | material::SANDSTONE => Biome::Desert,
            material::SNOW => Biome::Snow,
            _ => Biome::Rock,
        }
    }
}
