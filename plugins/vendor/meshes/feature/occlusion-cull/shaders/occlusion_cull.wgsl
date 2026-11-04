//! Hi-Z occlusion culling — fully GPU-driven, O(1) CPU.
//!
//! Each thread evaluates one DRAW CALL slot by testing the bounding sphere
//! of that draw call's first (representative) instance against the Hi-Z pyramid.
//! Occluded draws get instance_count=0 in the indirect buffer.
//!
//! IMPORTANT: this pass runs AFTER IndirectDispatchPass (frustum cull). It does
//! NOT re-do frustum culling — only tests occlusion. The indirect buffer is
//! shared: frustum cull writes initial instance_count, then we may zero it.
//!
//! Uses TEMPORAL Hi-Z: the pyramid was built from the PREVIOUS frame's depth,
//! so the OcclusionCullPass runs BEFORE DepthPrepass each frame.
//! Frame 0 is skipped entirely (no pyramid yet).
//!use helio_prelude

// ──────────────────────────────────────────────────────────────────────────────
// Bind group 0
// ──────────────────────────────────────────────────────────────────────────────

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;

struct CullParams {
    screen_width:       u32,
    screen_height:      u32,
    // Rows `draw_calls` holds. The indirect dispatch covers the live groups.
    draw_capacity:      u32,
    hiz_mip_count:      u32,
    // Baked PVS grid (helio-bake, published as `baked_pvs`); 0 = none.
    pvs_available:      u32,
    pvs_grid_x:         u32,
    pvs_grid_y:         u32,
    pvs_grid_z:         u32,
    pvs_min_x:          f32,
    pvs_min_y:          f32,
    pvs_min_z:          f32,
    pvs_cell_size:      f32,
    /// u32 words per source cell (the baker's u64 words × 2).
    pvs_words_per_cell: u32,
    _pad0:              u32,
    _pad1:              u32,
    _pad2:              u32,
}
@group(0) @binding(1) var<uniform> params: CullParams;

// GpuInstanceData: 224 bytes, must match libhelio/src/instance.rs exactly.
struct GpuInstanceData {
    model_col0:      vec4<f32>,  //   0 – 15
    model_col1:      vec4<f32>,  //  16 – 31
    model_col2:      vec4<f32>,  //  32 – 47
    model_col3:      vec4<f32>,  //  48 – 63
    normal_col0:     vec4<f32>,  //  64 – 79   (w = padding)
    normal_col1:     vec4<f32>,  //  80 – 95
    normal_col2:     vec4<f32>,  //  96 – 111
    bounds:          vec4<f32>,  // 112 – 127  (xyz = world-space sphere center, w = radius)
    prev_model_col0: vec4<f32>,  // 128 – 143
    prev_model_col1: vec4<f32>,  // 144 – 159
    prev_model_col2: vec4<f32>,  // 160 – 175
    prev_model_col3: vec4<f32>,  // 176 – 191
    mesh_id:         u32,        // 192
    material_id:     u32,        // 196
    flags:           u32,        // 200
    _pad:            u32,        // 204
    skin_vertex_offset: u32,
    bone_palette_offset: u32,
    bone_palette_count: u32,
    _skin_pad: u32,
}
@group(0) @binding(2) var<storage, read> instances: array<GpuInstanceData>;

/// GpuDrawCall: 20 bytes, matches DrawCall in indirect_dispatch.wgsl.
struct GpuDrawCall {
    index_count:    u32,
    first_index:    u32,
    vertex_offset:  i32,
    first_instance: u32,  // base index into instances[] for this batch
    instance_count: u32,  // number of consecutive instances in this draw
}
@group(0) @binding(3) var<storage, read> draw_calls: array<GpuDrawCall>;

@group(0) @binding(4) var hiz_tex:  texture_2d<f32>;
@group(0) @binding(5) var hiz_samp: sampler;

// Baked potentially-visible-set bitfield, the baker's u64 words as u32 pairs:
// bit `to` of source cell `from` is word `from * pvs_words_per_cell + to / 32`,
// bit `to % 32`. Set means "cell `to` may be visible from cell `from`".
@group(0) @binding(7) var<storage, read> pvs_bits: array<u32>;

// Indirect draw buffer as raw u32 array.
// DrawIndexedIndirect stride = 20 bytes = 5 × u32:
//   [i*5 + 0] index_count
//   [i*5 + 1] instance_count  ← we write 0 (occluded) or keep original value
//   [i*5 + 2] first_index
//   [i*5 + 3] base_vertex     (i32 reinterpreted as u32 for array access)
//   [i*5 + 4] first_instance
@group(0) @binding(6) var<storage, read_write> indirect: array<u32>;
@group(0) @binding(9) var<storage, read_write> stats:   array<atomic<u32>>;

// Per-group compacted original instance slots surviving frustum culling
// (written by IndirectDispatchPass) and, after this pass, surviving BOTH
// frustum AND Hi-Z occlusion. Draw-consuming passes must read
// `compacted_indices_2`, not `compacted_indices` (frustum-only intermediate).
@group(0) @binding(10) var<storage, read>       compacted_indices:   array<u32>;
@group(0) @binding(11) var<storage, read_write> compacted_indices_2: array<u32>;

// Coordinate-space transforms (current frame). Slot 0 = identity. An
// instance's `bounds` center is authored pre-space (same frame as `model`),
// so it must be mapped through this before any occlusion test — mirrors the
// frustum-stage handling in indirect_dispatch.wgsl.
@group(0) @binding(12) var<storage, read> coordinate_spaces: array<mat4x4<f32>>;

// One workgroup handles one draw-call group, cooperatively Hi-Z-testing only
// the instances that already survived frustum culling and re-compacting the
// survivors — mirrors IndirectDispatchPass's per-instance compaction so a
// group's final `instance_count` reflects real per-instance occlusion instead
// of an all-or-nothing "is the whole batch occluded" test.
var<workgroup> wg_counter: atomic<u32>;

// Stats layout (shared with IndirectDispatchPass):
// 4: occlusion_culled  (we only write to slot 4)
// 7: shadow_occlusion_culled

// ──────────────────────────────────────────────────────────────────────────────
// Helpers
// ──────────────────────────────────────────────────────────────────────────────

// Interval projection contains every point of the sphere, including near
// and off-axis silhouettes. Dividing by center depth underestimates them.
fn ratio_interval(lo: f32, hi: f32, wlo: f32, whi: f32) -> vec2<f32> {
    return vec2<f32>(min(lo / wlo, lo / whi), max(hi / wlo, hi / whi));
}

fn instance_hiz_occluded(inst: GpuInstanceData, center: vec3<f32>) -> bool {
    let radius = inst.bounds.w;
    if !(radius > 0.0) { return false; }
    let m = cameras[0].view_proj;
    let c = m * vec4<f32>(center, 1.0);
    let extent = radius * vec4<f32>(
        length(vec3<f32>(m[0].x, m[1].x, m[2].x)),
        length(vec3<f32>(m[0].y, m[1].y, m[2].y)),
        length(vec3<f32>(m[0].z, m[1].z, m[2].z)),
        length(vec3<f32>(m[0].w, m[1].w, m[2].w)));
    let wlo = c.w - extent.w;
    let whi = c.w + extent.w;
    if !(wlo > 1e-5) { return false; }
    let x = ratio_interval(c.x - extent.x, c.x + extent.x, wlo, whi);
    let y = ratio_interval(c.y - extent.y, c.y + extent.y, wlo, whi);
    let z = ratio_interval(c.z - extent.z, c.z + extent.z, wlo, whi);
    if x.y < -1.0 || x.x > 1.0 || y.y < -1.0 || y.x > 1.0 { return false; }
    // Reversed Z: maximum projected depth is the nearest surface point.
    let near_z = clamp(z.y, 0.0, 1.0);
    let size = vec2<f32>(textureDimensions(hiz_tex, 0));
    // Cover subpixel jitter and rounding at rectangle boundaries.
    let guard = vec2<f32>(2.0) / size;
    let uv_lo = clamp(vec2<f32>(x.x * 0.5 + 0.5, 0.5 - y.y * 0.5) - guard, vec2<f32>(0.0), vec2<f32>(1.0));
    let uv_hi = clamp(vec2<f32>(x.y * 0.5 + 0.5, 0.5 - y.x * 0.5) + guard, vec2<f32>(0.0), vec2<f32>(1.0));
    let pixels = max((uv_hi - uv_lo) * size, vec2<f32>(1.0));
    var mip = min(u32(ceil(log2(max(pixels.x, pixels.y)))), params.hiz_mip_count - 1u);
    var lo = vec2<i32>(0);
    var hi = vec2<i32>(0);
    // NPOT mip dimensions need an explicit coverage check: the four taps
    // must cover EVERY intersected texel, not just four chosen corners.
    loop {
        let dim = vec2<i32>(textureDimensions(hiz_tex, i32(mip)));
        lo = clamp(vec2<i32>(floor(uv_lo * vec2<f32>(dim))), vec2<i32>(0), dim - vec2<i32>(1));
        hi = clamp(vec2<i32>(floor(uv_hi * vec2<f32>(dim))), vec2<i32>(0), dim - vec2<i32>(1));
        if all(hi - lo <= vec2<i32>(1)) || mip + 1u >= params.hiz_mip_count { break; }
        mip += 1u;
    }
    let d00 = textureLoad(hiz_tex, lo, i32(mip)).r;
    let d01 = textureLoad(hiz_tex, vec2<i32>(hi.x, lo.y), i32(mip)).r;
    let d10 = textureLoad(hiz_tex, vec2<i32>(lo.x, hi.y), i32(mip)).r;
    let d11 = textureLoad(hiz_tex, hi, i32(mip)).r;
    return helio_hiz_occludes(min(min(d00, d01), min(d10, d11)), near_z);
}

/// Largest cell block an instance's bounds may cover before the PVS test is
/// skipped (the instance is then kept, never culled on a guess).
const PVS_MAX_CELLS: i32 = 64;

/// Cell coordinates of `p` in the PVS grid (may be outside it).
fn pvs_cell_coords(p: vec3<f32>) -> vec3<i32> {
    let origin = vec3<f32>(params.pvs_min_x, params.pvs_min_y, params.pvs_min_z);
    return vec3<i32>(floor((p - origin) / params.pvs_cell_size));
}

fn pvs_grid() -> vec3<i32> {
    return vec3<i32>(i32(params.pvs_grid_x), i32(params.pvs_grid_y), i32(params.pvs_grid_z));
}

fn pvs_inside(c: vec3<i32>) -> bool {
    return all(c >= vec3<i32>(0)) && all(c < pvs_grid());
}

fn pvs_cell_index(c: vec3<i32>) -> u32 {
    let g = pvs_grid();
    return u32(c.x + c.y * g.x + c.z * g.x * g.y);
}

/// Whether cell `dst_cell` may be visible from cell `src_cell`. Out-of-range words
/// count as visible, matching `BakedPvsRef::is_visible`.
fn pvs_visible(src_cell: u32, dst_cell: u32) -> bool {
    let word = src_cell * params.pvs_words_per_cell + dst_cell / 32u;
    if word >= arrayLength(&pvs_bits) {
        return true;
    }
    return ((pvs_bits[word] >> (dst_cell % 32u)) & 1u) != 0u;
}

/// Test a single instance against the baked PVS (Helio#256). `center` is the
/// bounds center already mapped through the instance's coordinate space.
///
/// Occluded only when no cell the bounding sphere touches is potentially
/// visible from the camera's cell. Anything the grid cannot answer exactly
/// (camera or bounds outside it, bounds spanning too many cells) is visible.
fn instance_pvs_occluded(inst: GpuInstanceData, center: vec3<f32>, cam_pos: vec3<f32>) -> bool {
    let cam_cell = pvs_cell_coords(cam_pos);
    if !pvs_inside(cam_cell) {
        return false;
    }
    let src_cell = pvs_cell_index(cam_cell);
    let radius = max(inst.bounds.w, 0.0);
    let lo = pvs_cell_coords(center - vec3<f32>(radius));
    let hi = pvs_cell_coords(center + vec3<f32>(radius));
    if !pvs_inside(lo) || !pvs_inside(hi) {
        return false;
    }
    let span = hi - lo + vec3<i32>(1);
    if span.x * span.y * span.z > PVS_MAX_CELLS {
        return false;
    }
    for (var z = lo.z; z <= hi.z; z++) {
        for (var y = lo.y; y <= hi.y; y++) {
            for (var x = lo.x; x <= hi.x; x++) {
                if pvs_visible(src_cell, pvs_cell_index(vec3<i32>(x, y, z))) {
                    return false;
                }
            }
        }
    }
    return true;
}

/// Returns true when an instance is occluded by the Hi-Z pyramid or, when a
/// bake provided one, the static PVS.
/// Mirrors `libhelio::INSTANCE_FLAG_ALWAYS_VISIBLE`.
const INSTANCE_FLAG_ALWAYS_VISIBLE: u32 = 4u;

fn instance_is_occluded(inst: GpuInstanceData, center: vec3<f32>, cam_pos: vec3<f32>) -> bool {
    // Per-object cull opt-out — must be honoured here as well as in the frustum stage,
    // or an object marked always-visible still vanishes behind the Hi-Z test.
    if (inst.flags & INSTANCE_FLAG_ALWAYS_VISIBLE) != 0u {
        return false;
    }
    if instance_hiz_occluded(inst, center) {
        return true;
    }
    if params.pvs_available != 0u {
        if instance_pvs_occluded(inst, center, cam_pos) {
            return true;
        }
    }
    return false;
}

@compute @workgroup_size(64, 1, 1)
fn main(
    @builtin(workgroup_id) wg_id: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let idx = wg_id.x;
    let active_draw = idx < params.draw_capacity;
    var visible_count = 0u;

    // Cooperatively Hi-Z-test only the instances that already survived
    // frustum culling (`visible_count` of them, packed in `compacted_indices`
    // starting at `dc.first_instance`), compacting survivors into
    // `compacted_indices_2` via a workgroup-shared atomic counter.
    //
    // Every invocation must reach the barrier below. Although `idx` and the
    // indirect count are uniform for a workgroup, FXC cannot prove uniformity
    // through storage-buffer reads and rejects a barrier reached after the old
    // early returns. Keep all draw-dependent work inside the branch instead.
    if active_draw {
        visible_count = indirect[idx * 5u + 1u];
        let dc = draw_calls[idx];
        let cam_pos = cameras[0].position_near.xyz;
        for (var i = lid.x; i < visible_count; i += 64u) {
            let original_idx = compacted_indices[dc.first_instance + i];
            let inst = instances[original_idx];
            // Common case (space 0, identity) stays exactly as before; only a
            // sublevel/portal-tagged instance pays the extra matrix multiply.
            let space_id = (inst.flags >> 8u) & 0xFFu;
            let center = select(
                inst.bounds.xyz,
                (coordinate_spaces[space_id] * vec4<f32>(inst.bounds.xyz, 1.0)).xyz,
                space_id != 0u,
            );
            if !instance_is_occluded(inst, center, cam_pos) {
                let slot = atomicAdd(&wg_counter, 1u);
                compacted_indices_2[dc.first_instance + slot] = original_idx;
            }
        }
    }

    workgroupBarrier();

    if lid.x == 0u && active_draw && visible_count > 0u {
        let final_count = atomicLoad(&wg_counter);
        indirect[idx * 5u + 1u] = final_count;
        if final_count == 0u {
            atomicAdd(&stats[4u], 1u);
        }
    }
}
