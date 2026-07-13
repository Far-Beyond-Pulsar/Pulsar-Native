use pulsar_reflection::Reflectable;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Reflectable)]
pub enum VoxelDataSource {
    Empty,
    Procedural,
    FromAsset,
}

impl Default for VoxelDataSource {
    fn default() -> Self {
        Self::Empty
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Reflectable)]
pub enum MeshingAlgorithm {
    Simple,
    Greedy,
    SurfaceNets,
    MarchingCubes,
}

impl Default for MeshingAlgorithm {
    fn default() -> Self {
        Self::Greedy
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Reflectable)]
pub enum VoxelMaterialSource {
    Single,
    Palette,
    Texture,
}

impl Default for VoxelMaterialSource {
    fn default() -> Self {
        Self::Single
    }
}
