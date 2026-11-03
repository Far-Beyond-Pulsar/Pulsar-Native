//! Screen-space portal-opening mask — two tiny sub-passes run back to back
//! (see `helio-pass-portal-instances::mask::PortalMaskPass::execute`):
//!
//! 1. **Stamp** (`vs_stamp`/`fs_stamp`): draws each active portal's real
//!    opening quad (`portal.transform` * `half_extent`) through the main
//!    camera, depth-tested (read-only) against the G-buffer's already-written
//!    real depth. A quad fragment survives only where nothing real already
//!    occludes the portal from this viewpoint, and writes `portal_index + 1`
//!    into `portal_mask` there. This *is* the fix for portal content leaking
//!    outside the opening's on-screen silhouette — see
//!    `helio-pass-portal-instances/shaders/gbuffer_portal.wgsl`'s module doc
//!    for the full story.
//!
//! 2. **Reset** (`vs_reset`/`fs_reset`): a full-screen triangle that samples
//!    the mask just stamped and, wherever it's non-zero, writes the depth
//!    clear value (infinitely far) into the real depth buffer. Without this, the portal-duplicate
//!    pass's own depth test would compare its (legitimately distant) content
//!    against whatever real geometry happens to sit behind the opening —
//!    which is exactly the bug that made the near portal render solid black
//!    (see that crate's changelog / investigation notes). Resetting depth
//!    only where the mask says "portal visible here" means: duplicate
//!    content correctly self-occludes (nearer copies win) inside the
//!    opening, while depth stays untouched (and the mask stays 0) anywhere
//!    the portal itself is blocked from view — so the duplicate pass's own
//!    depth+mask test still correctly rejects content there too.

struct Camera {
    view:           mat4x4<f32>,
    proj:           mat4x4<f32>,
    view_proj:      mat4x4<f32>,
    inv_view_proj:  mat4x4<f32>,
    position_near:  vec4<f32>,
    forward_far:    vec4<f32>,
    jitter_frame:   vec4<f32>,
    prev_view_proj: mat4x4<f32>,
}

/// Must match libhelio::GpuPortalView (144 bytes) — see gbuffer_portal.wgsl.
struct GpuPortalView {
    transform:         mat4x4<f32>,
    inverse_transform: mat4x4<f32>,
    half_extent:       vec2<f32>,
    coordinate_space:  u32,
    _pad:              u32,
}

// ── Stamp ────────────────────────────────────────────────────────────────────

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<storage, read> portal_views: array<GpuPortalView>;

// Two triangles covering [-1,1]^2 in the portal's own local X/Y, scaled by
// half_extent in the vertex shader — the portal's real opening quad.
const LOCAL_CORNERS: array<vec2<f32>, 6> = array<vec2<f32>, 6>(
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
    vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
);

struct StampOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) @interpolate(flat) portal_index: u32,
    @location(1) @interpolate(flat) portal_flags: u32,
}

@vertex
fn vs_stamp(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> StampOutput {
    let portal = portal_views[instance_index];
    let local = LOCAL_CORNERS[vertex_index] * portal.half_extent;
    let world_pos = portal.transform * vec4<f32>(local, 0.0, 1.0);

    var out: StampOutput;
    out.clip_position = cameras[0].view_proj * world_pos;
    out.portal_index = instance_index;
    out.portal_flags = portal._pad;
    return out;
}

@fragment
fn fs_stamp(input: StampOutput) -> @location(0) u32 {
    // Nested projection rows are required by PortalChainComponent for
    // clipping, but they do not represent additional physical openings in
    // the current world. Legacy rows use zero and remain visible.
    if (input.portal_flags & 1u) != 0u {
        discard;
    }
    return input.portal_index + 1u;
}

// ── Reset ────────────────────────────────────────────────────────────────────

@group(0) @binding(0) var portal_mask: texture_2d<u32>;

// Depth written wherever the mask is non-zero. Deliberately the *exact* clear
// value (helio_core::depth::CLEAR, which GBufferPass clears to) —
// not just "very far" — because deferred lighting distinguishes real geometry
// from empty background by comparing against that same clear value. Content
// inside the portal opening that isn't covered by any duplicated surface
// (e.g. the open interior of a hollow duplicated corridor) must read back as
// ordinary background there, exactly as if there were no portal, rather than
// as a very-distant-but-technically-real surface with stale G-buffer data —
// the latter previously showed up as a faint lit-looking rectangle over the
// whole opening. Any real duplicate content still wins the instance pass's
// GreaterEqual test against this: reversed depth is 0 only at infinity, so
// legitimate scene depth is always strictly greater.
const RESET_DEPTH: f32 = 0.0;

@vertex
fn vs_reset(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    // Standard full-screen-triangle trick: 3 vertices covering the whole
    // clip-space square and then some, no vertex/index buffer needed.
    let x = f32((vertex_index << 1u) & 2u) * 2.0 - 1.0;
    let y = f32(vertex_index & 2u) * 2.0 - 1.0;
    return vec4<f32>(x, -y, RESET_DEPTH, 1.0);
}

@fragment
fn fs_reset(@builtin(position) pos: vec4<f32>) {
    let mask_value = textureLoad(portal_mask, vec2<i32>(pos.xy), 0).r;
    if mask_value == 0u {
        discard;
    }
    // Depth write happens via the pipeline's normal depth-write path using
    // this fragment's interpolated position.z (== RESET_DEPTH, constant
    // across the triangle) — no @builtin(frag_depth) needed.
}
