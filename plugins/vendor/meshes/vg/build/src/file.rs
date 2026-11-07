//! `.vgeo` container pieces that the frozen `helio-vg-format` header leaves to
//! the builder: quantization metadata, the page directory, and the per-cluster
//! stream layout.
//!
//! File layout (all offsets from the start of the file, little endian):
//!
//! ```text
//! 0      VgeoHeader                       (72 B, helio-vg-format)
//! 72     VgeoMeta                         (32 B)
//! hierarchy_offset  [GpuHierarchyNode]    (16-aligned, root node = index 0)
//! group_refs_offset [GpuGroupRef]
//! page_dir_offset   [PageDirEntry]
//! pages_offset      page 0 .. page N-1    (4096-aligned, PAGE_BYTES each)
//! ```
//!
//! Page layout (offsets page-relative):
//!
//! ```text
//! 0                       PageHeader (32 B)
//! clusters_offset         [GpuCluster; cluster_count]
//! position_stream_offset  per-cluster position blocks
//! attribute_stream_offset per-cluster attribute blocks
//! index_stream_offset     per-cluster u8 triangle indices (3 per tri, padded to 4)
//! ```
//!
//! A cluster's **position block** (at `GpuCluster::vertex_offset`) is:
//!
//! ```text
//! u32 anchor_x, anchor_y, anchor_z      min lattice coordinate over the cluster
//! u32 bits: bx | by << 8 | bz << 16     bits per axis (0..=LATTICE_BITS)
//! u32 attribute_offset                  page-relative start of its attribute block
//! bit stream, LSB first, per vertex: (x - anchor_x) in bx bits, then y, then z
//! ```
//!
//! Positions are quantized on an **asset-global lattice**
//! (`origin + q * step`), so a vertex shared by two clusters decodes
//! bit-identically whichever cluster (or page, or LOD level) it is read from.
//! A per-cluster or per-page grid would open cracks along cluster seams.

use bytemuck::{Pod, Zeroable};
use helio_vg_format::GpuCluster;

/// Bits per axis of the asset-global position lattice.
pub use helio_vg_format::{LATTICE_BITS, LATTICE_MAX, POS_BLOCK_HEADER_WORDS};

/// 32 bytes, directly after `VgeoHeader`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct VgeoMeta {
    /// Lattice origin (asset space); decode `p = origin + q * step` in f32.
    pub origin: [f32; 3],
    pub step: f32,
    /// Bytes of opaque attributes per vertex.
    pub attribute_stride: u32,
    pub lattice_bits: u32,
    /// Number of DAG levels (leaf = level 0, so roots are at `level_count - 1`).
    pub level_count: u32,
    /// Byte offset from the file start of the page-dependency table (0 = none).
    /// CSR: `page_count + 1` u32 offsets, then the flat list of dependency page ids.
    pub page_deps_offset: u32,
}

/// 16 bytes per page.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Pod, Zeroable)]
pub struct PageDirEntry {
    /// Absolute file offset of the page.
    pub file_offset: u32,
    pub cluster_count: u32,
    /// Range of DAG levels of the clusters stored in the page.
    pub min_level: u32,
    pub max_level: u32,
}

/// Perspective-ish projection for the CPU cut: error `e` at a sphere projects
/// to `e * scale / distance_to_nearest_point_of_sphere` pixels.
#[derive(Clone, Copy, Debug)]
pub struct Projection {
    pub eye: [f32; 3],
    /// `viewport_height / (2 * tan(fov_y / 2))`.
    pub scale: f32,
}

impl Projection {
    pub fn project(&self, error: f32, sphere: [f32; 4]) -> f32 {
        if error.is_infinite() {
            return f32::INFINITY;
        }
        let dx = sphere[0] - self.eye[0];
        let dy = sphere[1] - self.eye[1];
        let dz = sphere[2] - self.eye[2];
        let d = ((dx * dx + dy * dy + dz * dz).sqrt() - sphere[3]).max(1e-6);
        error * self.scale / d
    }
}

/// One cluster chosen by [`crate::VgeoReader::select_clusters`].
#[derive(Clone, Copy, Debug)]
pub struct SelectedCluster {
    pub page_id: u32,
    pub cluster_index: u32,
    pub cluster: GpuCluster,
}

/// A cluster decoded back to CPU arrays.
#[derive(Clone, Debug)]
pub struct DecodedCluster<'a> {
    /// Lattice coordinates per local vertex.
    pub lattice: Vec<[u32; 3]>,
    /// Dequantized positions (`origin + q * step`).
    pub positions: Vec<[f32; 3]>,
    /// Local vertex indices per triangle.
    pub tris: Vec<[u8; 3]>,
    /// `vert_count * attribute_stride` opaque attribute bytes.
    pub attributes: &'a [u8],
}
