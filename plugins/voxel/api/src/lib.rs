//! Renderer-independent voxel data: a voxel world's brush journal (the
//! ordered edits saved with a level, and the chunks kept with them) and the
//! names hosts and renderers share.
//!
//! The SceneDB component owns the journal. No GPU resource or render pass is
//! owned here.

mod brush;
mod ids;

pub use brush::{
    VoxelBrushEdit, VoxelBrushOp, VoxelBrushShape, VoxelEditBase, VoxelEditJournal, VoxelKeptChunks,
};
pub use ids::{VOXEL_TERRAIN_GENERATOR, VOXEL_TERRAIN_GENERATOR_VERSION, VOXEL_TERRAIN_RENDERER};
