//! Destructible voxel worlds, drawn: the Helio passes of
//! [`helio_voxel_world`]'s planets and planes. The world's modules are
//! re-exported, so a host names one crate.
//!
//! * `chunks` — chunk trees in an octree (docs/chunk-octree.md): GPU
//!   residency, drawing and lighting of every volume, and the Helio pass.
//! * `engine` — what hosts publish for the pass (frames, requests) and the
//!   terrain half of the shaders, checked against the CPU field.
//! * `meadow` — the terrain's foliage types.
//! * [`VoxelFeature`] — the node that adds the chunk pass to preset graphs
//!   (with [`VoxelSettings`]); hosts publish their volumes (`engine::VOXEL_FRAME`).
pub mod chunks;
pub mod engine;
mod feature;
mod water_shore;
pub use water_shore::VoxelShoreTerrain;
pub mod meadow;
pub mod settings;

pub use feature::{VoxelFeature, VoxelSettings};

pub use helio_voxel_world::{
    edit_store, edit_yield, edits, feature_recipe, features, flora, grid, landform, layers, noise,
    planet, planet_maps, queries, record, snapshot, stamp, streams, terrain, volume, volumes,
};
pub use helio_voxel_world::{
    BrickCell, Brush, BrushOp, BrushShape, Cell, GeneratorInfo, Grid, Matter, Planet, PlanetRecipe,
    RayHit, TerrainField, TerrainGenerator, TerrainProgram, TerrainSource, VolumeTransform,
    Volumes, VoxelVolume, Walked,
};
