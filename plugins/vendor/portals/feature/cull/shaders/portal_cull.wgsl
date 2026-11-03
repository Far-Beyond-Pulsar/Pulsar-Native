// Per-portal-*chain* GPU frustum culling — selects which instances get a
// duplicate draw through each active portal chain (a sequence of up to
// a runtime-sized portal sequence, e.g. "through P, then through P again,
// then again" — chains, not single portals, make recursive views automatic.
//
// Two passes, `select` then `finalize`, mirroring why the main scene's own
// GPU cull does the same split:
//
// `select`: one workgroup per (draw-call group, chain) pair, its 64 lanes
// cooperatively test that group's instances — mapped through the chain's
// *composed* transform, built here from `coordinate_spaces` — against the
// main camera frustum. Survivors are appended into that draw group's own
// reserved region of a *shared* compacted-index buffer (capacity is per
// draw-*group*, not per chain — chains vastly outnumber draw groups, and
// realistically only a few chains ever have any given group's content in
// frustum at once, so reserving capacity per chain would be enormous waste
// for essentially no headroom benefit). Each survivor is tagged with which
// chain selected it, since a single draw group can (and typically does) be
// selected by several different chains at once, each wanting its own
// separately-transformed duplicate.
//
// `finalize`: one thread per draw group, turns that group's final survivor
// count into a normal `DrawIndexedIndirect` — split out into its own pass
// because the count isn't known until every chain's `select` workgroup for
// that group has finished (they run concurrently, all bumping the same
// atomic counter).
//
// No occlusion (Hi-Z) test here — frustum-only. Portal-visible content is
// already bounded by the frustum test; occlusion is a pure fill-rate
// optimization this v1 skips (see docs/).

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
@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;

struct CullUniforms {
    frustum_planes:     array<vec4<f32>, 6>,
    // Draw groups dispatched; the live count is `live_draw_count()`.
    draw_capacity:      u32,
    chain_count:        u32,
    // How many (instance, chain) survivor slots are reserved per draw
    // group — NOT per chain. See module doc.
    group_capacity:     u32,
    _pad:               u32,
}
@group(0) @binding(1) var<uniform> cull: CullUniforms;

// Must match GpuInstanceData in libhelio exactly (224 bytes).
struct GpuInstance {
    model_0:      vec4<f32>,
    model_1:      vec4<f32>,
    model_2:      vec4<f32>,
    model_3:      vec4<f32>,
    normal_0:     vec4<f32>,
    normal_1:     vec4<f32>,
    normal_2:     vec4<f32>,
    bounds:       vec4<f32>,
    prev_model_0: vec4<f32>,
    prev_model_1: vec4<f32>,
    prev_model_2: vec4<f32>,
    prev_model_3: vec4<f32>,
    mesh_id:      u32,
    material_id:  u32,
    flags:        u32,
    _pad:         u32,
    skin_vertex_offset: u32,
    bone_palette_offset: u32,
    bone_palette_count: u32,
    _skin_pad: u32,
}
@group(0) @binding(2) var<storage, read> instances: array<GpuInstance>;

struct GpuDrawCall {
    index_count:    u32,
    first_index:    u32,
    vertex_offset:  i32,
    first_instance: u32,
    instance_count: u32,
}
@group(0) @binding(3) var<storage, read> draw_calls: array<GpuDrawCall>;

// Coordinate-space transforms (current frame). Slot 0 = identity — see
// `libhelio::{coordinate_space, set_coordinate_space}`.
@group(0) @binding(4) var<storage, read> coordinate_spaces: array<mat4x4<f32>>;

// One active portal's render data. Must match libhelio::GpuPortalView (144 bytes).
struct GpuPortalView {
    transform:         mat4x4<f32>,
    inverse_transform: mat4x4<f32>,
    half_extent:       vec2<f32>,
    coordinate_space:  u32,
    _pad:              u32,
}
@group(0) @binding(5) var<storage, read> portal_views: array<GpuPortalView>;

// SceneDB's variable-length chain field: one handle row per chain and one
// contiguous payload pool. `count` is the runtime recursion depth.
struct GpuPortalChainHandle {
    offset: u32,
    count:  u32,
}
@group(0) @binding(9) var<storage, read> portal_chain_handles: array<GpuPortalChainHandle>;
@group(0) @binding(11) var<storage, read> portal_chain_portals: array<u32>;

fn chain_depth(chain_idx: u32) -> u32 {
    return portal_chain_handles[chain_idx].count;
}

fn chain_portal(chain_idx: u32, stage: u32) -> u32 {
    let handle = portal_chain_handles[chain_idx];
    return portal_chain_portals[handle.offset + stage];
}

struct DrawIndexedIndirect {
    index_count:    u32,
    instance_count: u32,
    first_index:    u32,
    base_vertex:    i32,
    first_instance: u32,
}
// One entry per draw group (NOT per chain) — same shape as the main scene's
// own indirect buffer.
@group(0) @binding(6) var<storage, read_write> portal_indirect: array<DrawIndexedIndirect>;
// Shared compacted buffer: draw group `g`'s reserved region is
// `[g * cull.group_capacity, (g+1) * cull.group_capacity)`, filled by
// whichever chains' `select` workgroups actually found survivors there.
@group(0) @binding(7) var<storage, read_write> portal_compacted_indices: array<u32>;
// Parallel to `portal_compacted_indices` — which chain each compacted entry
// was selected under (the instance pass needs this to know which composed
// transform, and which portal's screen-space mask, applies to it).
@group(0) @binding(10) var<storage, read_write> portal_compacted_chains: array<u32>;
// Per-draw-group survivor counts across all chains combined — a *global*
// (cross-workgroup) atomic, since every chain that touches group `g`
// contributes to the same counter concurrently. Zeroed by the CPU every
// frame before dispatch; `finalize` reads the settled value once `select`
// has fully completed.
@group(0) @binding(8) var<storage, read_write> portal_stats: array<atomic<u32>>;
// Object Batch's GPU counts, written this frame; word 0 is the live draw
// group count (`ObjectBatchFrameData::draw_counts_gpu`). The dispatch covers
// every group the batch can produce, up to `draw_capacity`.
@group(0) @binding(12) var<storage, read> batch_counts: array<u32>;

/// Live draw groups this frame, within the dispatched range.
fn live_draw_count() -> u32 {
    return min(batch_counts[0], cull.draw_capacity);
}

/// Mirrors `libhelio::INSTANCE_FLAG_ALWAYS_VISIBLE`.
const INSTANCE_FLAG_ALWAYS_VISIBLE: u32 = 4u;

fn sphere_in_frustum(center: vec3<f32>, radius: f32) -> bool {
    for (var i = 0u; i < 6u; i++) {
        let plane = cull.frustum_planes[i];
        if dot(plane.xyz, center) + plane.w + radius < 0.0 { return false; }
    }
    return true;
}

@compute @workgroup_size(64)
fn select(
    @builtin(workgroup_id) wg_id: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let draw_idx = wg_id.x;
    let chain_idx = wg_id.y;
    let is_active = draw_idx < live_draw_count() && chain_idx < cull.chain_count;

    // Every invocation must reach the barrier below regardless of
    // `is_active` — see indirect_dispatch.wgsl for why (FXC
    // barrier-uniformity rejection). No workgroup-local state to init this
    // time (the survivor counter is the global `portal_stats`), so the
    // barrier is only here to keep that same uniform-control-flow shape.
    workgroupBarrier();

    if is_active {
        let chain = portal_chain_handles[chain_idx];
        let dc = draw_calls[draw_idx];
        let group_base = draw_idx * cull.group_capacity;

        for (var i = lid.x; i < dc.instance_count; i += 64u) {
            let slot_idx = dc.first_instance + i;
            let inst = instances[slot_idx];
            let own_space_id = (inst.flags >> 8u) & 0xFFu;

            // Skip an instance already living in one of THIS chain's own
            // portal coordinate spaces — it would duplicate onto itself.
            // Generalizes the single-portal recursion guard the pre-chain
            // cull used.
            var self_referential = false;
            for (var k = 0u; k < chain.count; k++) {
                if own_space_id == portal_views[chain_portal(chain_idx, k)].coordinate_space {
                    self_referential = true;
                }
            }
            if self_referential {
                continue;
            }

            let own_space = coordinate_spaces[own_space_id];
            let radius = inst.bounds.w;

            // Apply the chain deepest-portal-first, same order as the draw
            // pass. The fragment shader owns the exact portal-window clip.
            // Do not reject against an inner portal's rectangle here: the
            // instance bound is only a sphere, and a large wall can have its
            // center outside that rectangle while a real part of the mesh
            // still intersects the opening. That false negative is exactly
            // what makes room walls disappear while small center props and
            // portal frames remain visible. Frustum culling below remains
            // conservative; the fragment stage is the correctness gate.
            //
            // The outermost stage (`s == 1`, portal `chain.portals[0]`)
            // only checks "behind the surface". The screen-space mask and
            // fragment shader enforce the actual aperture.
            var pos = (own_space * vec4<f32>(inst.bounds.xyz, 1.0)).xyz;
            var rejected = false;
            for (var s = chain.count; s > 0u; s--) {
                let p = portal_views[chain_portal(chain_idx, s - 1u)];
                pos = (coordinate_spaces[p.coordinate_space] * vec4<f32>(pos, 1.0)).xyz;
                let local = (p.inverse_transform * vec4<f32>(pos, 1.0)).xyz;
                // Keep the complete far-side half-space. Reject only bounds
                // that are entirely on the camera-facing side. In Helio's
                // inward-facing portal convention the camera side is
                // negative local Z and mapped target content is positive.
                if local.z < -radius {
                    rejected = true;
                    break;
                }
            }
            if rejected {
                continue;
            }
            let world_center = pos;

            let visible = (inst.flags & INSTANCE_FLAG_ALWAYS_VISIBLE) != 0u
                || sphere_in_frustum(world_center, radius);
            if visible {
                // Global (cross-workgroup) claim — other chains' workgroups
                // for this same draw group are appending concurrently.
                let slot = atomicAdd(&portal_stats[draw_idx], 1u);
                if slot < cull.group_capacity {
                    portal_compacted_indices[group_base + slot] = slot_idx;
                    portal_compacted_chains[group_base + slot] = chain_idx;
                }
                // Else: this draw group's reserved region is full for this
                // frame — drop the excess rather than corrupt the next
                // group's region, same accepted degradation shape
                // helio-pass-shadow-cull's per-face draw cap uses.
            }
        }
    }
}

@compute @workgroup_size(64)
fn finalize(@builtin(global_invocation_id) gid: vec3<u32>) {
    let draw_idx = gid.x;
    if draw_idx >= cull.draw_capacity {
        return;
    }
    // A group past this frame's live count (one just despawned) draws
    // nothing, rather than its last frame's survivors.
    if draw_idx >= live_draw_count() {
        portal_indirect[draw_idx] = DrawIndexedIndirect(0u, 0u, 0u, 0, 0u);
        return;
    }
    let dc = draw_calls[draw_idx];
    let survivor_count = min(atomicLoad(&portal_stats[draw_idx]), cull.group_capacity);
    portal_indirect[draw_idx] = DrawIndexedIndirect(
        dc.index_count,
        survivor_count,
        dc.first_index,
        dc.vertex_offset,
        draw_idx * cull.group_capacity,
    );
}
