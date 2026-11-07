//! Virtual-geometry offline builder (plan: `.agents/plans/virtual-geometry.md`,
//! section 2.1 / task T2).
//!
//! Pure CPU: an indexed triangle mesh goes in, a `.vgeo` byte blob comes out
//! (cluster DAG + always-resident hierarchy + 128 KiB pages). No GPU
//! dependencies; the output is a deterministic function of the input and the
//! [`BuildOptions`].
//!
//! Error metric: every group's error is the *measured* object-space distance
//! from original-surface reference points (carried up the DAG) to the group's
//! simplified surface, times a small margin; parent error is
//! `max(children, measured)`, never a sum (module `deviation`). The simplifier's
//! own estimate is not used, and input vertices it leaves further than a
//! per-group budget from the result are locked and the group re-simplified.
//!
//! Pipeline: [`mesh::weld`] -> per-material [`cluster`] -> repeat
//! { [`partition`] 4..=8 clusters, [`simplify`] with locked group boundary,
//! split } -> [`pack::write_vgeo`].

mod cluster;
mod dag;
mod deviation;
mod file;
mod math;
mod mesh;
mod pack;
mod partition;
mod reader;
mod simplify;

pub use dag::{build_dag, ClusterNode, Dag, Group, NONE};
pub use file::{
    DecodedCluster, PageDirEntry, Projection, SelectedCluster, VgeoMeta, LATTICE_BITS, LATTICE_MAX,
    POS_BLOCK_HEADER_WORDS,
};
pub use math::sphere_contains;
pub use mesh::{BuildError, MeshInput, VertexStore};
pub use pack::{write_vgeo, BuildOptions, MAX_ROOT_PAGES};
pub use reader::{PageView, ParseError, VgeoReader};

/// Builder revision stored in the upper 16 bits of asset flags. The binary
/// format stays readable, but derived caches from older algorithms must cook
/// again. Revision 2 fixed accumulated QEM error and premature hierarchy stop;
/// revision 3 measures each group's error against the original surface
/// (`deviation`), reduces 40% per level and stops stalled levels. Revision 5 builds a real DAG for disconnected/island geometry (floating pieces, proximity merging, appearance error) and bounds the pinned root set to `MAX_ROOT_PAGES`.
/// Revision 7 restores revision 5 foliage simplification and coverage error.
/// Revision 6 produced distorted foliage and excessive refinement; recook it.
pub const COOK_REVISION: u32 = 11;

pub fn is_current_cook(header: &helio_vg_format::VgeoHeader) -> bool {
    header.asset.flags >> 16 == COOK_REVISION
}

/// Build a `.vgeo` from a mesh. Same input and options give the same bytes
/// regardless of thread count.
pub fn build_vgeo(mesh: &MeshInput, opts: &BuildOptions) -> Result<Vec<u8>, BuildError> {
    let dag = build_dag(mesh)?;
    write_vgeo(&dag, mesh.attributes, mesh.attribute_stride, opts)
}
