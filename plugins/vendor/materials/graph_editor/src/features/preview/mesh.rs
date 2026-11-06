//! Preview mesh definitions - sphere, quad, cube, and cinderblock

use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct PreviewVertex {
    pub position: [f32; 3],
    pub normal: [f32; 3],
    pub uv: [f32; 2],
}

pub struct PreviewMeshData {
    pub vertices: Vec<PreviewVertex>,
    pub indices: Vec<u32>,
    pub index_count: u32,
}

impl PreviewMeshData {
    /// Radius of the smallest sphere centered on the origin that contains
    /// every vertex — used to frame the preview camera around the mesh.
    pub fn bounding_radius(&self) -> f32 {
        self.vertices
            .iter()
            .map(|v| {
                let [x, y, z] = v.position;
                (x * x + y * y + z * z).sqrt()
            })
            .fold(0.0f32, f32::max)
    }
}

pub fn generate_sphere(radius: f32, sectors: u32, stacks: u32) -> PreviewMeshData {
    let mut vertices = Vec::new();
    let mut indices = Vec::new();

    for stack in 0..=stacks {
        let phi = std::f32::consts::PI * stack as f32 / stacks as f32;
        for sector in 0..=sectors {
            let theta = 2.0 * std::f32::consts::PI * sector as f32 / sectors as f32;

            let x = radius * phi.sin() * theta.cos();
            let y = radius * phi.cos();
            let z = radius * phi.sin() * theta.sin();

            let nx = x / radius;
            let ny = y / radius;
            let nz = z / radius;

            let u = sector as f32 / sectors as f32;
            let v = stack as f32 / stacks as f32;

            vertices.push(PreviewVertex {
                position: [x, y, z],
                normal: [nx, ny, nz],
                uv: [u, v],
            });
        }
    }

    for stack in 0..stacks {
        for sector in 0..sectors {
            let first = stack * (sectors + 1) + sector;
            let second = first + sectors + 1;

            indices.push(first);
            indices.push(second);
            indices.push(first + 1);

            indices.push(second);
            indices.push(second + 1);
            indices.push(first + 1);
        }
    }

    let index_count = indices.len() as u32;
    PreviewMeshData {
        vertices,
        indices,
        index_count,
    }
}

pub fn generate_quad() -> PreviewMeshData {
    let vertices = vec![
        PreviewVertex { position: [-1.0, -1.0, 0.0], normal: [0.0, 0.0, 1.0], uv: [0.0, 1.0] },
        PreviewVertex { position: [1.0, -1.0, 0.0], normal: [0.0, 0.0, 1.0], uv: [1.0, 1.0] },
        PreviewVertex { position: [1.0, 1.0, 0.0], normal: [0.0, 0.0, 1.0], uv: [1.0, 0.0] },
        PreviewVertex { position: [-1.0, 1.0, 0.0], normal: [0.0, 0.0, 1.0], uv: [0.0, 0.0] },
    ];
    let indices = vec![0, 1, 2, 0, 2, 3];
    let index_count = indices.len() as u32;
    PreviewMeshData { vertices, indices, index_count }
}

pub fn generate_cube() -> PreviewMeshData {
    let positions: [[f32; 3]; 24] = [
        // Front face (+Z)
        [-1.0, -1.0, 1.0], [1.0, -1.0, 1.0], [1.0, 1.0, 1.0], [-1.0, 1.0, 1.0],
        // Back face (-Z)
        [1.0, -1.0, -1.0], [-1.0, -1.0, -1.0], [-1.0, 1.0, -1.0], [1.0, 1.0, -1.0],
        // Top face (+Y)
        [-1.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, -1.0], [-1.0, 1.0, -1.0],
        // Bottom face (-Y)
        [-1.0, -1.0, -1.0], [1.0, -1.0, -1.0], [1.0, -1.0, 1.0], [-1.0, -1.0, 1.0],
        // Right face (+X)
        [1.0, -1.0, 1.0], [1.0, -1.0, -1.0], [1.0, 1.0, -1.0], [1.0, 1.0, 1.0],
        // Left face (-X)
        [-1.0, -1.0, -1.0], [-1.0, -1.0, 1.0], [-1.0, 1.0, 1.0], [-1.0, 1.0, -1.0],
    ];

    let normals: [[f32; 3]; 6] = [
        [0.0, 0.0, 1.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [-1.0, 0.0, 0.0],
    ];

    let uvs: [[f32; 2]; 4] = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];

    let mut vertices = Vec::new();
    for face in 0..6 {
        let base = face * 4;
        let n = normals[face];
        for corner in 0..4 {
            vertices.push(PreviewVertex {
                position: positions[base + corner],
                normal: n,
                uv: uvs[corner],
            });
        }
    }

    let indices: Vec<u32> = (0..6).flat_map(|f| {
        let b = f * 4;
        vec![b, b + 1, b + 2, b, b + 2, b + 3]
    }).collect();

    let index_count = indices.len() as u32;
    PreviewMeshData { vertices, indices, index_count }
}

pub fn generate_cinderblock() -> PreviewMeshData {
    let mut vs = Vec::new();

    // Outer box: 1.5 x 1.0 x 1.0 centered at origin
    let ox = 1.5;
    let oy = 1.0;
    let oz = 1.0;

    // Inner cavity (recess on front face): 0.6 x 0.4 x 0.3 inset from front
    let cx = 0.6;
    let cy = 0.4;
    let cz = 0.3;

    // Front face of outer box (has a recess)
    // We create vertices for the ring around the recess
    let outer_verts: [[f32; 3]; 4] = [
        [-ox, -oy, oz], [ox, -oy, oz], [ox, oy, oz], [-ox, oy, oz],
    ];
    let inner_verts: [[f32; 3]; 4] = [
        [-cx, -cy, oz], [cx, -cy, oz], [cx, cy, oz], [-cx, cy, oz],
    ];
    let recess_verts: [[f32; 3]; 4] = [
        [-cx, -cy, oz - cz], [cx, -cy, oz - cz], [cx, cy, oz - cz], [-cx, cy, oz - cz],
    ];

    // Front face ring (outer rect with inner hole - use 2 triangles per segment)
    // Top segment
    vs.push(PreviewVertex { position: outer_verts[0], normal: [0.0, 0.0, 1.0], uv: [0.0, 1.0] });
    vs.push(PreviewVertex { position: outer_verts[1], normal: [0.0, 0.0, 1.0], uv: [1.0, 1.0] });
    vs.push(PreviewVertex { position: inner_verts[1], normal: [0.0, 0.0, 1.0], uv: [0.7, 0.7] });
    vs.push(PreviewVertex { position: inner_verts[0], normal: [0.0, 0.0, 1.0], uv: [0.3, 0.7] });

    // Right segment
    vs.push(PreviewVertex { position: outer_verts[1], normal: [0.0, 0.0, 1.0], uv: [1.0, 1.0] });
    vs.push(PreviewVertex { position: outer_verts[2], normal: [0.0, 0.0, 1.0], uv: [1.0, 0.0] });
    vs.push(PreviewVertex { position: inner_verts[2], normal: [0.0, 0.0, 1.0], uv: [0.7, 0.3] });
    vs.push(PreviewVertex { position: inner_verts[1], normal: [0.0, 0.0, 1.0], uv: [0.7, 0.7] });

    // Bottom segment
    vs.push(PreviewVertex { position: outer_verts[2], normal: [0.0, 0.0, 1.0], uv: [1.0, 0.0] });
    vs.push(PreviewVertex { position: outer_verts[3], normal: [0.0, 0.0, 1.0], uv: [0.0, 0.0] });
    vs.push(PreviewVertex { position: inner_verts[3], normal: [0.0, 0.0, 1.0], uv: [0.3, 0.3] });
    vs.push(PreviewVertex { position: inner_verts[2], normal: [0.0, 0.0, 1.0], uv: [0.7, 0.3] });

    // Left segment
    vs.push(PreviewVertex { position: outer_verts[3], normal: [0.0, 0.0, 1.0], uv: [0.0, 0.0] });
    vs.push(PreviewVertex { position: outer_verts[0], normal: [0.0, 0.0, 1.0], uv: [0.0, 1.0] });
    vs.push(PreviewVertex { position: inner_verts[0], normal: [0.0, 0.0, 1.0], uv: [0.3, 0.7] });
    vs.push(PreviewVertex { position: inner_verts[3], normal: [0.0, 0.0, 1.0], uv: [0.3, 0.3] });

    // Recess inner faces (back wall of cavity)
    let recess_norm = [0.0, 0.0, -1.0];
    vs.push(PreviewVertex { position: recess_verts[0], normal: recess_norm, uv: [0.0, 1.0] });
    vs.push(PreviewVertex { position: recess_verts[1], normal: recess_norm, uv: [1.0, 1.0] });
    vs.push(PreviewVertex { position: recess_verts[2], normal: recess_norm, uv: [1.0, 0.0] });
    vs.push(PreviewVertex { position: recess_verts[3], normal: recess_norm, uv: [0.0, 0.0] });

    // Recess side walls (inset edges)
    // Top wall of recess
    vs.push(PreviewVertex { position: inner_verts[0], normal: [0.0, -1.0, 0.0], uv: [0.0, 0.0] });
    vs.push(PreviewVertex { position: inner_verts[1], normal: [0.0, -1.0, 0.0], uv: [1.0, 0.0] });
    vs.push(PreviewVertex { position: recess_verts[1], normal: [0.0, -1.0, 0.0], uv: [1.0, 1.0] });
    vs.push(PreviewVertex { position: recess_verts[0], normal: [0.0, -1.0, 0.0], uv: [0.0, 1.0] });

    // Bottom wall of recess
    vs.push(PreviewVertex { position: inner_verts[3], normal: [0.0, 1.0, 0.0], uv: [0.0, 0.0] });
    vs.push(PreviewVertex { position: inner_verts[2], normal: [0.0, 1.0, 0.0], uv: [1.0, 0.0] });
    vs.push(PreviewVertex { position: recess_verts[2], normal: [0.0, 1.0, 0.0], uv: [1.0, 1.0] });
    vs.push(PreviewVertex { position: recess_verts[3], normal: [0.0, 1.0, 0.0], uv: [0.0, 1.0] });

    // Right wall of recess
    vs.push(PreviewVertex { position: inner_verts[1], normal: [1.0, 0.0, 0.0], uv: [0.0, 0.0] });
    vs.push(PreviewVertex { position: inner_verts[2], normal: [1.0, 0.0, 0.0], uv: [1.0, 0.0] });
    vs.push(PreviewVertex { position: recess_verts[2], normal: [1.0, 0.0, 0.0], uv: [1.0, 1.0] });
    vs.push(PreviewVertex { position: recess_verts[1], normal: [1.0, 0.0, 0.0], uv: [0.0, 1.0] });

    // Left wall of recess
    vs.push(PreviewVertex { position: inner_verts[0], normal: [-1.0, 0.0, 0.0], uv: [0.0, 0.0] });
    vs.push(PreviewVertex { position: inner_verts[3], normal: [-1.0, 0.0, 0.0], uv: [1.0, 0.0] });
    vs.push(PreviewVertex { position: recess_verts[3], normal: [-1.0, 0.0, 0.0], uv: [1.0, 1.0] });
    vs.push(PreviewVertex { position: recess_verts[0], normal: [-1.0, 0.0, 0.0], uv: [0.0, 1.0] });

    // Remaining 5 faces of outer box (simple quad faces)
    let back_face = [[1.0, -1.0, -1.0], [-1.0, -1.0, -1.0], [-1.0, 1.0, -1.0], [1.0, 1.0, -1.0]];
    for corner in 0..4 {
        vs.push(PreviewVertex { position: back_face[corner], normal: [0.0, 0.0, -1.0], uv: [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]][corner] });
    }

    let top_face = [[-1.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, -1.0], [-1.0, 1.0, -1.0]];
    for corner in 0..4 {
        vs.push(PreviewVertex { position: top_face[corner], normal: [0.0, 1.0, 0.0], uv: [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]][corner] });
    }

    let bottom_face = [[-1.0, -1.0, -1.0], [1.0, -1.0, -1.0], [1.0, -1.0, 1.0], [-1.0, -1.0, 1.0]];
    for corner in 0..4 {
        vs.push(PreviewVertex { position: bottom_face[corner], normal: [0.0, -1.0, 0.0], uv: [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]][corner] });
    }

    let right_face = [[1.0, -1.0, 1.0], [1.0, -1.0, -1.0], [1.0, 1.0, -1.0], [1.0, 1.0, 1.0]];
    for corner in 0..4 {
        vs.push(PreviewVertex { position: right_face[corner], normal: [1.0, 0.0, 0.0], uv: [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]][corner] });
    }

    let left_face = [[-1.0, -1.0, -1.0], [-1.0, -1.0, 1.0], [-1.0, 1.0, 1.0], [-1.0, 1.0, -1.0]];
    for corner in 0..4 {
        vs.push(PreviewVertex { position: left_face[corner], normal: [-1.0, 0.0, 0.0], uv: [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]][corner] });
    }

    // Build indices (2 triangles per 4-vertex quad)
    let mut indices = Vec::new();
    for quad in 0..(vs.len() / 4) {
        let b = quad as u32 * 4;
        indices.push(b);
        indices.push(b + 1);
        indices.push(b + 2);
        indices.push(b);
        indices.push(b + 2);
        indices.push(b + 3);
    }

    let index_count = indices.len() as u32;
    PreviewMeshData { vertices: vs, indices, index_count }
}
