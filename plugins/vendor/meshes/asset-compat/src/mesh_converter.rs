//! Vertex format conversion from SolidRS to Helio
//!
//! Converts SolidRS's rich vertex format (up to 8 UVs, 4 colors, optional tangents)
//! to Helio's compact PackedVertex format (32 bytes).

use crate::Result;
use helio_mesh_api::PackedVertex;
use solid_rs::geometry::{Topology, Vertex};
use solid_rs::scene::Mesh;
use std::sync::Once;

static WARNED_MISSING_NORMAL: Once = Once::new();
static WARNED_EXTRA_UVS: Once = Once::new();
static WARNED_VERTEX_COLORS: Once = Once::new();

/// Convert a SolidRS vertex to Helio's PackedVertex format
pub fn convert_vertex(v: &Vertex, flip_uv_y: bool) -> PackedVertex {
    // Extract position (mandatory)
    let position = [v.position.x, v.position.y, v.position.z];

    // Extract normal (auto-generate if missing)
    let normal = if let Some(n) = v.normal {
        [n.x, n.y, n.z]
    } else {
        WARNED_MISSING_NORMAL.call_once(|| log::warn!("Vertex missing normal, using +Y"));
        [0.0, 1.0, 0.0]
    };

    // Extract primary UV channel (default to 0,0 if missing)
    let tex_coords = if let Some(uv) = v.uvs[0] {
        if flip_uv_y {
            [uv.x, 1.0 - uv.y] // Flip for DirectX → OpenGL
        } else {
            [uv.x, uv.y] // Use as-is
        }
    } else {
        [0.0, 0.0]
    };

    // Extract tangent XYZ and bitangent handedness sign from the Vec4.
    // SolidRS stores tangent as Vec4 where w = bitangent sign (+1 or -1),
    // following the MikkTSpace / FBX / glTF convention.
    // The SolidRS FBX loader already flips V (1.0 - v) to convert from the
    // DirectX top-left UV origin to the OpenGL bottom-left convention.
    // Flipping V inverts the bitangent direction, so we negate w here to
    // keep the TBN frame consistent with the flipped UVs.
    let (tangent, bitangent_sign) = if let Some(t) = v.tangent {
        ([t.x, t.y, t.z], -t.w)
    } else {
        (normal_to_tangent(normal), 1.0)
    };

    // Warn if we're dropping data
    let tex_coords1 = if let Some(uv) = v.uvs[1] {
        if flip_uv_y {
            [uv.x, 1.0 - uv.y]
        } else {
            [uv.x, uv.y]
        }
    } else {
        [0.0, 0.0]
    };

    if v.uvs.iter().skip(2).any(|uv| uv.is_some()) {
        WARNED_EXTRA_UVS.call_once(|| log::warn!("Vertex has more than two UV channels - only UV0/UV1 are supported, higher channels will be discarded"));
    }
    if v.colors.iter().any(|c| c.is_some()) {
        WARNED_VERTEX_COLORS.call_once(|| {
            log::warn!("Mesh has vertex colors - not yet supported, will be discarded")
        });
    }

    let mut packed =
        PackedVertex::from_components(position, normal, tex_coords, tangent, bitangent_sign);
    packed.tex_coords1 = tex_coords1;
    packed
}

/// Convert a single SolidRS primitive (submesh) to Helio vertex/index buffers
///
/// This is the correct approach: each primitive has its own material and should be
/// rendered as a separate draw call.
pub fn convert_primitive(
    mesh: &Mesh,
    primitive: &solid_rs::geometry::Primitive,
    config: &crate::LoadConfig,
) -> Result<(Vec<PackedVertex>, Vec<u32>)> {
    use solid_rs::geometry::Topology;

    // Only support triangle lists for now
    if primitive.topology != Topology::TriangleList {
        return Err(crate::AssetError::UnsupportedFormat(format!(
            "Primitive topology {:?} not supported, only TriangleList",
            primitive.topology
        )));
    }

    // Check UV coverage for debugging
    let has_uvs = mesh.vertices.iter().any(|v| v.uvs[0].is_some());
    if !has_uvs {
        log::warn!(
            "Mesh '{}' has NO UV coordinates - textures will not display correctly",
            mesh.name
        );
    } else {
        // Calculate UV bounds to detect issues
        let mut min_u = f32::MAX;
        let mut max_u = f32::MIN;
        let mut min_v = f32::MAX;
        let mut max_v = f32::MIN;

        for v in &mesh.vertices {
            if let Some(uv) = v.uvs[0] {
                min_u = min_u.min(uv.x);
                max_u = max_u.max(uv.x);
                min_v = min_v.min(uv.y);
                max_v = max_v.max(uv.y);
            }
        }

        log::debug!(
            "Mesh '{}' UV range: U=[{:.3}, {:.3}], V=[{:.3}, {:.3}]",
            mesh.name,
            min_u,
            max_u,
            min_v,
            max_v
        );

        if min_u < -0.1 || max_u > 1.1 || min_v < -0.1 || max_v > 1.1 {
            log::warn!("Mesh '{}' has UVs outside [0,1] range", mesh.name);
        }
    }

    let vertices: Vec<PackedVertex> = mesh
        .vertices
        .iter()
        .map(|v| convert_vertex(v, config.flip_uv_y))
        .collect();

    let indices = primitive.indices.clone();

    let max_index = indices.iter().max().copied().unwrap_or(0);
    let vertices_len = vertices.len();
    if max_index >= vertices_len as u32 {
        log::error!(
            "Mesh '{}': index out of bounds — max_index={}, vertices={}; first 10: {:?}",
            mesh.name,
            max_index,
            vertices_len,
            &indices[0..indices.len().min(10)]
        );
    }

    Ok((vertices, indices))
}

/// Convert a SolidRS mesh to Helio vertex/index buffers (deprecated - merges all primitives)
///
/// DEPRECATED: This merges all primitives together and loses per-primitive material info.
/// Use convert_primitive instead.
#[allow(dead_code)]
pub fn convert_mesh(mesh: &Mesh) -> Result<(Vec<PackedVertex>, Vec<u32>)> {
    // Check UV coverage for debugging
    let has_uvs = mesh.vertices.iter().any(|v| v.uvs[0].is_some());
    if !has_uvs {
        log::warn!(
            "Mesh '{}' has NO UV coordinates - textures will not display correctly",
            mesh.name
        );
    } else {
        // Calculate UV bounds to detect issues
        let mut min_u = f32::MAX;
        let mut max_u = f32::MIN;
        let mut min_v = f32::MAX;
        let mut max_v = f32::MIN;

        for v in &mesh.vertices {
            if let Some(uv) = v.uvs[0] {
                min_u = min_u.min(uv.x);
                max_u = max_u.max(uv.x);
                min_v = min_v.min(uv.y);
                max_v = max_v.max(uv.y);
            }
        }

        log::debug!(
            "Mesh '{}' UV range: U=[{:.3}, {:.3}], V=[{:.3}, {:.3}]",
            mesh.name,
            min_u,
            max_u,
            min_v,
            max_v
        );

        if min_u < -0.1 || max_u > 1.1 || min_v < -0.1 || max_v > 1.1 {
            log::warn!("Mesh '{}' has UVs outside [0,1] range", mesh.name);
        }
    }

    // Convert all vertices (deprecated - uses no UV flip)
    let vertices: Vec<PackedVertex> = mesh
        .vertices
        .iter()
        .map(|v| convert_vertex(v, false))
        .collect();

    // Collect all indices from all primitives
    let mut indices = Vec::new();
    for primitive in &mesh.primitives {
        // Only support triangle lists for now
        if primitive.topology != Topology::TriangleList {
            log::warn!(
                "Primitive has topology {:?}, only TriangleList is supported - skipping",
                primitive.topology
            );
            continue;
        }

        indices.extend_from_slice(&primitive.indices);
    }

    Ok((vertices, indices))
}

/// Compute a tangent perpendicular to the normal (used when no tangent is provided)
fn normal_to_tangent(n: [f32; 3]) -> [f32; 3] {
    // Choose the axis least aligned with n to avoid degeneracy
    let up = if n[1].abs() < 0.9 {
        [0.0f32, 1.0, 0.0]
    } else {
        [1.0f32, 0.0, 0.0]
    };
    // cross(up, n) gives a vector perpendicular to n
    let t = [
        up[1] * n[2] - up[2] * n[1],
        up[2] * n[0] - up[0] * n[2],
        up[0] * n[1] - up[1] * n[0],
    ];
    let len = (t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt().max(1e-8);
    [t[0] / len, t[1] / len, t[2] / len]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_convert_minimal_vertex() {
        use solid_rs::glam::Vec3;

        let v = Vertex {
            position: Vec3::new(1.0, 2.0, 3.0),
            normal: Some(Vec3::new(0.0, 1.0, 0.0)),
            tangent: None,
            colors: [None; 4],
            uvs: [None; 8],
            skin_weights: None,
        };

        let packed = convert_vertex(&v, false);
        assert_eq!(packed.position, [1.0, 2.0, 3.0]);
        assert_eq!(packed.tex_coords0, [0.0, 0.0]);
        assert_eq!(packed.tex_coords1, [0.0, 0.0]);
    }
}
