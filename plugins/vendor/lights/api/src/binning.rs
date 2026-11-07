//! The lights' screen-tile binning: what the light binning publishes for
//! whatever lights per pixel from a tile's list (deferred lighting, the
//! forward function's drawers, transparency, a terrain's local shadows).
//! A pixel's tile is `(x / TILE_SIZE, y / TILE_SIZE)`, row-major over
//! `num_tiles_x`; its lights are `tile_light_lists[tile * MAX_LIGHTS_PER_TILE
//! + i]` for `i < tile_light_counts[tile]`.

/// Pixels a tile spans along each axis.
pub const TILE_SIZE: u32 = 16;
/// Lights a tile's list holds at most.
pub const MAX_LIGHTS_PER_TILE: u32 = 64;
/// Graph key of the tiles' light indices (a storage buffer of `u32`).
pub const TILE_LIGHT_LISTS: &str = "tile_light_lists";
/// Graph key of the tiles' light counts (a storage buffer of `u32`).
pub const TILE_LIGHT_COUNTS: &str = "tile_light_counts";
/// Graph key of the binning's frame data ([`ClusterLightGrid`]).
pub const CLUSTER_LIGHT_GRID: &str = "cluster_light_grid";

/// The binning's frame data (under [`CLUSTER_LIGHT_GRID`]).
#[derive(Clone, Copy)]
pub struct ClusterLightGrid<'a> {
    pub tile_light_lists: &'a wgpu::Buffer,
    pub tile_light_counts: &'a wgpu::Buffer,
    pub num_tiles_x: u32,
    pub num_tiles_y: u32,
}
