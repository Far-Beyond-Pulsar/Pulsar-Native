use pulsar_reflection::Reflectable;
use serde::{Deserialize, Serialize};

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
