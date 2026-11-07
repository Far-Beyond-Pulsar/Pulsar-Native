//! The final, fully-culled (frustum + Hi-Z occlusion), per-range-compacted
//! indirect draw args and compacted instance indices -- what every pass that actually issues
//! `multi_draw_indexed_indirect` calls (`helio-pass-gbuffer`, `helio-pass-
//! shadow` and its `-cull`/`-dirty` siblings, `helio-pass-transparent`,
//! `helio-pass-forward-lit`, `helio-pass-portal-cull`/`-instances`) should
//! read. Produced by `helio-pass-
//! occlusion-cull`'s `OcclusionCullPass` from `helio-pass-indirect-
//! dispatch`'s `IndirectDispatchFrameData`.
//!
//! The meshes' contract, as [`crate::ObjectBatchFrameData`].
#[derive(Clone, Copy)]
pub struct CulledBatchFrameData<'a> {
    /// Per-range packed indirect args for groups with at least one surviving
    /// instance. Unused tail slots have zero `instance_count` for non-count
    /// multi-draw fallbacks.
    pub indirect: &'a wgpu::Buffer,
    /// Final surviving instance slots, packed per draw-call group -- index
    /// `instances` (from [`crate::ObjectBatchFrameData`]) through this.
    pub compacted_indices: &'a wgpu::Buffer,
    /// The same surviving draws, packed per material key at fixed offsets:
    /// what passes drawing one pipeline per material use.
    pub segments: crate::DrawSegments<'a>,
}
