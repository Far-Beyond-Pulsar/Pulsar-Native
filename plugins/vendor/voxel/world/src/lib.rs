//! Destructible voxel worlds, the data: cube-sphere planets and planes, their
//! terrain, edits and queries. Renderer-independent: the GPU half (residency,
//! drawing, lighting) is `helio-pass-voxel-planet`, which evaluates this
//! crate's field on the GPU through the WGSL mirrors in [`wgsl`].
//!
//! * [`grid`] — equal-angle cube-sphere cells aligned with gravity.
//! * [`terrain`] — pluggable terrain generators, evaluated bit-for-bit
//!   identically on CPU and GPU, built from [`noise`]; [`layers`] is the
//!   built-in one (ordered layer stacks, interpreted by [`landform`]).
//! * [`edits`] — ordered brushes with exact integer containment;
//!   [`edit_store`] bakes them into cells, [`edit_yield`] counts what one
//!   changes (gameplay's blocks broken and placed).
//! * [`planet`] — canonical queries, materials and exact ray casts.
//! * [`planet_maps`] — what the terrain knows of its planet as a whole:
//!   drainage, rivers, lakes and climate, baked once per world.
//! * [`features`] — procedural content (trees, rocks, buildings) generated
//!   like the terrain (docs/features.md), from [`stamp`] models; [`flora`]
//!   grows natural ones (trees, shrubs, boulders, craters), and a
//!   [`feature_recipe`] describes a world's (what grows where, in metres)
//!   for any voxel size and seed.
//! * [`volumes`] — the terrain's volumetric shapes: cave networks and
//!   cliff overhangs (docs/volumes.md).
//! * [`volume`] — worlds placed in a scene by f64 transforms, any number
//!   of them, queried by world positions.
pub mod edit_store;
pub mod edit_yield;
pub mod edits;
pub mod feature_recipe;
pub mod features;
pub mod flora;
pub mod grid;
pub mod landform;
pub mod layers;
pub mod noise;
pub mod planet;
pub mod planet_maps;
pub mod queries;
pub mod record;
mod ridge_envelope;
pub mod snapshot;
pub mod stamp;
pub mod streams;
pub mod terrain;
pub mod volume;
pub mod volumes;

pub use edits::{Brush, BrushOp, BrushShape};
pub use grid::{Cell, Grid};
pub use planet::{BrickCell, Matter, Planet, PlanetRecipe, RayHit, Walked};
pub use terrain::{GeneratorInfo, TerrainField, TerrainGenerator, TerrainProgram, TerrainSource};
pub use volume::{VolumeTransform, Volumes, VoxelVolume};

/// The GPU mirrors of this crate's field, bit for bit: what a renderer
/// composes its terrain program from. Paths are relative to [`DIR`], for
/// hot reloading (`HELIO_SHADER_FROM_DISK`).
pub mod wgsl {
    /// This crate's directory (the mirrors' paths are relative to it).
    pub const DIR: &str = env!("CARGO_MANIFEST_DIR");
    /// Integer noise ([`crate::noise`]).
    pub const NOISE: &str = include_str!("../shaders/noise.wgsl");
    pub const NOISE_PATH: &str = "shaders/noise.wgsl";
    /// The grid mapping ([`crate::grid`]): `World`.
    pub const WORLD: &str = include_str!("../shaders/world.wgsl");
    /// The volume layer ([`crate::volumes`], docs/volumes.md).
    pub const VOLUMES: &str = include_str!("../shaders/volumes.wgsl");
}
