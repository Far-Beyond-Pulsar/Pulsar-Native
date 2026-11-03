//! GPU-facing per-portal render data.
//!
//! Authored as SceneDB rows and mirrored into the `portal_views` and
//! SceneDB variable-length portal-chain buffers. Consumed by
//! `helio-pass-portal-cull` (frustum test
//! to select which instances get a duplicate draw) and
//! `helio-pass-portal-instances` (the duplicate draw itself, clipped to the
//! portal's opening).

use bytemuck::{Pod, Zeroable};

/// Legacy dense-row reserve used by existing demo setup code. It is not a
/// projection or recursion cap: the resolver/bridge accept runtime-sized
/// chain vectors, SceneDB grows the handle/payload buffers, and callers that
/// need more rows must reserve a larger runtime entity range.
///
/// It is retained only so older demos can reserve a conservative dense row
/// range without changing their setup code. The GPU cull output is governed
/// by its own runtime work/capacity policy and does not use this symbol.
pub const MAX_PORTAL_CHAINS: usize = 300;

/// The explicit `#[gpu(buffer = ...)]` pool key for portal-chain IDs.
pub const PORTAL_CHAIN_PORTAL_POOL_BUFFER: &str = "PortalChainComponent::portals";

/// SceneDB's generated per-row handle-table key for [`PORTAL_CHAIN_PORTAL_POOL_BUFFER`].
pub const PORTAL_CHAIN_HANDLE_BUFFER: &str = "PortalChainComponent::portals::handles";

/// One active portal's render data. 144 bytes.
///
/// # WGSL equivalent
/// ```wgsl
/// struct GpuPortalView {
///     transform:         mat4x4<f32>,  // 64 bytes
///     inverse_transform: mat4x4<f32>,  // 64 bytes
///     half_extent:       vec2<f32>,    // 8 bytes
///     coordinate_space:  u32,          // 4 bytes
///     _pad:               u32,          // 4 bytes
/// }
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct GpuPortalView {
    /// Portal-local → world (this portal surface's own pose, `pair.a.transform`).
    /// Used by `helio-pass-portal-mask` to place the opening quad it stamps
    /// into the screen-space visibility mask — see that crate's docs.
    pub transform: [f32; 16],

    /// World → portal-local (this portal surface's own inverse transform).
    /// Used by the fragment-shader clip test: a duplicated fragment is kept
    /// only when its world position maps within `half_extent` of local X/Y
    /// and beyond the source surface on the target side (local Z >= 0).
    pub inverse_transform: [f32; 16],

    /// Half-extent of the portal opening, in its own local X/Y.
    pub half_extent: [f32; 2],

    /// Index into `coordinate_spaces[]` (see `crate::coordinate_space`) —
    /// holds the portal-view transform that places target-level content
    /// beyond the source surface where it should appear when seen through
    /// this side. This is distinct from the teleport/pair map.
    pub coordinate_space: u32,

    pub _pad: u32,
}

// A chain row is SceneDB's variable-length `Vec<u32>` field, not a fixed Rust
// struct. The GPU ABI is the pair of buffers named above: a
// `VarLenHandle { offset, count }` per row and a contiguous `u32` payload.

/// The live view and chain counts of the frame's portal projection: how
/// many leading rows of `portal_views` and of the chain tables are this
/// frame's (the SceneDB-backed buffers are capacity-sized). The host that
/// projects the portals publishes it every frame under [`PORTAL_COUNTS`];
/// without it the portal passes draw nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PortalCounts {
    pub views: u32,
    pub chains: u32,
}

/// The payload the portal passes read their live counts from.
pub const PORTAL_COUNTS: helio_core::PayloadContract<PortalCounts> =
    helio_core::PayloadContract::new("portal_counts");

/// The counts published in `scene` this frame (zero without a projection).
pub fn portal_counts(scene: &helio_core::SceneBufferProjection) -> PortalCounts {
    scene
        .payload(&PORTAL_COUNTS)
        .map(|counts| *counts)
        .unwrap_or_default()
}
