//! Frustum-culled indirect draw args + compacted instance indices --
//! this pass's own output, read ONLY by `helio-pass-occlusion-cull` (the
//! next culling stage). Not for drawing passes -- see `helio_pass_occlusion_
//! cull::CulledBatchFrameData` for the buffers a pass actually issuing draw
//! calls should read.
#[derive(Clone, Copy)]
pub struct IndirectDispatchFrameData<'a> {
    /// Per-group indirect draw args, `instance_count` replaced with each
    /// group's frustum-surviving count.
    pub indirect: &'a wgpu::Buffer,
    /// Frustum-culling survivors, packed per draw-call group starting at
    /// that group's `first_instance` offset -- index `instances` (from
    /// `helio_pass_gbuffer::ObjectBatchFrameData`) through this, not
    /// directly.
    pub compacted_indices: &'a wgpu::Buffer,
}
