//! SceneDB-owned reflection capture records.
//!
//! `ReflectionCaptureRow` is field-for-field identical to
//! `crate::GpuReflectionCapture`'s ABI: one placed capture volume (sphere
//! or oriented box), its parallax transform, and which cube-array layer its
//! baked (or not-yet-baked) cubemap lives in.
//!
//! # Blend-order caveat
//!
//! `deferred_lighting.wgsl`'s `sample_reflection_environment` accumulates
//! captures front-to-back and saturates, so overlapping captures are meant
//! to be fed smallest-influence-first (a small capture gets first claim on
//! a pixel, matching how a level designer expects a small "override" probe
//! nested inside a large one to win). Iterating this SceneDB buffer in raw
//! entity-index order does not guarantee that ordering -- unlike `lights`/
//! `decals`/etc, whose zero-row default is inert regardless of iteration
//! order, `ReflectionCaptureRow` rows are semantically order-sensitive
//! when captures overlap. A single capture, or non-overlapping captures,
//! blend identically either way; only deliberately-nested capture volumes
//! may pick a different (still plausible, not incorrect-looking) winner
//! than before. Sorting this buffer by influence volume is the same class
//! of cross-entity GPU-sort work already specified (not yet implemented)
//! for batched object instances -- tracked, not silently dropped.

#[derive(bytemuck::Pod, bytemuck::Zeroable, Clone, Copy, Debug, PartialEq)]
#[repr(C)]
pub struct ReflectionCaptureRow {
    pub position_radius: [f32; 4],
    pub extents_transition: [f32; 4],
    pub world_to_local: [[f32; 4]; 4],
    pub cubemap_index: i32,
    pub shape: u32,
    pub mobility: u32,
    pub brightness: f32,
}

// SAFETY: a `#[repr(C)]` bytemuck `Pod` row: no padding bytes, any bit
// pattern valid. Lets an engine's SceneDB component embed it.
unsafe impl pulsar_scenedb::Pod for ReflectionCaptureRow {}

/// The buffer the renderer reads [`ReflectionCaptureRow`]s from. Engines produce it
/// (a SceneDB component stored under this key, or a scene derivation).
pub const REFLECTION_CAPTURES: helio_core::BufferContract<ReflectionCaptureRow> =
    helio_core::BufferContract::new("reflection_captures");

impl From<crate::GpuReflectionCapture> for ReflectionCaptureRow {
    fn from(v: crate::GpuReflectionCapture) -> Self {
        bytemuck::cast(v)
    }
}
impl From<ReflectionCaptureRow> for crate::GpuReflectionCapture {
    fn from(v: ReflectionCaptureRow) -> Self {
        bytemuck::cast(v)
    }
}

/// Fixed capacity for the `"reflection_captures"` SceneDB buffer --
/// `deferred_lighting.wgsl` already clamped its old CPU-driven count to this
/// exact number (`min(globals.reflection_capture_count, 64u)`), so keeping
/// it at 64 changes nothing about the shader's own worst-case cost, only
/// where the row data and the iteration bound come from.
pub const MAX_REFLECTION_CAPTURES: u32 = 64;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_matches_gpu_abi() {
        assert_eq!(
            std::mem::size_of::<ReflectionCaptureRow>(),
            std::mem::size_of::<crate::GpuReflectionCapture>()
        );
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::ReflectionCaptureRow;
}
