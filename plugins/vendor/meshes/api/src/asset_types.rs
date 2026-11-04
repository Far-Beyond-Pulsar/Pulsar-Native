//! CPU-side asset payloads used to author SceneDB components.
//!
//! These types intentionally contain no GPU buffers, handles, arenas, or
//! renderer state. Frontends convert them into SceneDB component rows.
//!
//! Generic: a vertex-interchange layout (position/normal/tangent/uv packing)
//! any mesh-owning pass can use to author geometry, not a specific pass's
//! resource — the same tier as `GpuCameraUniforms`.

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, Pod, Zeroable)]
pub struct PackedVertex {
    pub position: [f32; 3],
    pub bitangent_sign: f32,
    pub tex_coords0: [f32; 2],
    pub tex_coords1: [f32; 2],
    pub normal: u32,
    pub tangent: u32,
}

unsafe impl pulsar_scenedb::Pod for PackedVertex {}

impl PackedVertex {
    pub fn from_components(
        position: [f32; 3],
        normal: [f32; 3],
        tex_coords: [f32; 2],
        tangent: [f32; 3],
        bitangent_sign: f32,
    ) -> Self {
        Self {
            position,
            bitangent_sign,
            tex_coords0: tex_coords,
            tex_coords1: [0.0, 0.0],
            normal: pack_snorm4x8([normal[0], normal[1], normal[2], 0.0]),
            tangent: pack_snorm4x8([tangent[0], tangent[1], tangent[2], 0.0]),
        }
    }
}

fn pack_snorm4x8(v: [f32; 4]) -> u32 {
    let to_i8 = |x: f32| -> u32 {
        let scaled = (x.clamp(-1.0, 1.0) * 127.0).round() as i8;
        scaled as u8 as u32
    };
    to_i8(v[0]) | (to_i8(v[1]) << 8) | (to_i8(v[2]) << 16) | (to_i8(v[3]) << 24)
}

#[derive(Debug, Clone)]
pub struct MeshUpload {
    pub vertices: Vec<PackedVertex>,
    pub indices: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct SectionedMeshUpload {
    pub vertices: Vec<PackedVertex>,
    pub sections: Vec<Vec<u32>>,
}
