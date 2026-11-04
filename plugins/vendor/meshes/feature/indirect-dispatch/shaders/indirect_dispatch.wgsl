// GPU frustum culling + indirect draw command generation.
// O(1) CPU cost: one dispatch, all culling on GPU.

struct Camera {
    view:          mat4x4<f32>,
    proj:          mat4x4<f32>,
    view_proj:     mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    position_near: vec4<f32>,
    forward_far:   vec4<f32>,
    jitter_frame:  vec4<f32>,
    prev_view_proj: mat4x4<f32>,
}

struct CullUniforms {
    frustum_planes: array<vec4<f32>, 6>,
    // Rows `draw_calls` holds. The indirect dispatch covers the live groups.
    draw_capacity: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

struct GpuInstance {
    model_0:      vec4<f32>,  //   0
    model_1:      vec4<f32>,  //  16
    model_2:      vec4<f32>,  //  32
    model_3:      vec4<f32>,  //  48
    normal_0:     vec4<f32>,  //  64
    normal_1:     vec4<f32>,  //  80
    normal_2:     vec4<f32>,  //  96
    bounds:       vec4<f32>,  // 112 — xyz = world-space center, w = world-space radius
    prev_model_0: vec4<f32>,  // 128
    prev_model_1: vec4<f32>,  // 144
    prev_model_2: vec4<f32>,  // 160
    prev_model_3: vec4<f32>,  // 176
    mesh_id:      u32,        // 192
    material_id:  u32,        // 196
    flags:        u32,        // 200
    _pad:         u32,        // 204
    skin_vertex_offset: u32,
    bone_palette_offset: u32,
    bone_palette_count: u32,
    _skin_pad: u32,
}

struct GpuDrawCall {
    index_count:    u32,
    first_index:    u32,
    vertex_offset:  i32,
    first_instance: u32,  // base index into instances[] for this batch
    instance_count: u32,  // number of consecutive instances
}

struct GpuAabb {
    min:   vec3<f32>,
    _pad0: f32,
    max:   vec3<f32>,
    _pad1: f32,
}

struct DrawIndexedIndirect {
    index_count:    u32,
    instance_count: u32,
    first_index:    u32,
    base_vertex:    i32,
    first_instance: u32,
}

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<uniform>            cull:       CullUniforms;
@group(0) @binding(2) var<storage, read>      instances:  array<GpuInstance>;
@group(0) @binding(3) var<storage, read>      draw_calls: array<GpuDrawCall>;
@group(0) @binding(4) var<storage, read>      aabbs:     array<GpuAabb>;
@group(0) @binding(5) var<storage, read_write> indirect:  array<DrawIndexedIndirect>;
@group(0) @binding(6) var<storage, read_write> stats:   array<atomic<u32>>;
// Per-group compacted list of original instance slots that survive frustum
// culling, packed starting at each group's `first_instance` offset. Consumers
// that draw through `indirect` must index `instances` through this buffer.
@group(0) @binding(7) var<storage, read_write> compacted_indices: array<u32>;
// Coordinate-space transforms (current frame). Slot 0 = identity. A
// sublevel/portal member's `bounds` sphere center is authored in its *local*
// (pre-space) transform, same as `model` — it must be mapped through this
// before any frustum/subpixel test against the world-space camera, exactly
// like the vertex shader maps `model` (see gbuffer.wgsl).
@group(0) @binding(8) var<storage, read> coordinate_spaces: array<mat4x4<f32>>;

// One workgroup handles one draw-call group. Its 64 lanes cooperatively test
// every instance in the group and compact survivors via workgroup-shared
// atomics, so a group's final `instance_count` reflects only what actually
// passed the frustum test instead of an all-or-nothing pass/fail per batch.
var<workgroup> wg_counter:        atomic<u32>;
var<workgroup> wg_nonsubpixel:    atomic<u32>;
var<workgroup> wg_shadow_caster:  atomic<u32>;

// Stats layout (shared with OcclusionCullPass):
// 0: total_draws
// 1: frustum_culled
// 2: subpixel_culled
// 3: frustum_visible
// 4: occlusion_culled     ← written by occlusion pass only
// 5: shadow_total
// 6: shadow_frustum_visible
// 7: shadow_occlusion_culled ← written by occlusion pass only

fn sphere_in_frustum(center: vec3<f32>, radius: f32) -> bool {
    for (var i = 0u; i < 6u; i++) {
        let plane = cull.frustum_planes[i];
        let dist = dot(plane.xyz, center) + plane.w;
        if dist + radius < 0.0 { return false; }
    }
    return true;
}

fn aabb_in_frustum(min: vec3<f32>, max: vec3<f32>) -> bool {
    for (var i = 0u; i < 6u; i++) {
        let plane = cull.frustum_planes[i];
        let px = select(min.x, max.x, plane.x >= 0.0);
        let py = select(min.y, max.y, plane.y >= 0.0);
        let pz = select(min.z, max.z, plane.z >= 0.0);
        let dist = dot(plane.xyz, vec3<f32>(px, py, pz)) + plane.w;
        if dist < 0.0 { return false; }
    }
    return true;
}

/// Mirrors `libhelio::INSTANCE_FLAG_ALWAYS_VISIBLE`.
const INSTANCE_FLAG_ALWAYS_VISIBLE: u32 = 4u;
/// Mirrors `helio_pass_object_batch::INSTANCE_FLAG_CLASSIC_SUPPRESSED`: another
/// renderer (virtual geometry) draws this instance in the main view.
const INSTANCE_FLAG_CLASSIC_SUPPRESSED: u32 = 16u;

struct TransformedAabb {
    lo: vec3<f32>,
    hi: vec3<f32>,
}

/// Re-derives a world-space AABB after an additional rigid transform (a
/// coordinate space may rotate, unlike the pure-translation common case), by
/// transforming all 8 corners of the input box and taking the new min/max.
/// Only reached for instances tagged with a non-identity coordinate space.
fn transform_aabb(lo: vec3<f32>, hi: vec3<f32>, m: mat4x4<f32>) -> TransformedAabb {
    var new_lo = vec3<f32>(3.4e38);
    var new_hi = vec3<f32>(-3.4e38);
    for (var i = 0u; i < 8u; i++) {
        let corner = vec3<f32>(
            select(lo.x, hi.x, (i & 1u) != 0u),
            select(lo.y, hi.y, (i & 2u) != 0u),
            select(lo.z, hi.z, (i & 4u) != 0u),
        );
        let world = (m * vec4<f32>(corner, 1.0)).xyz;
        new_lo = min(new_lo, world);
        new_hi = max(new_hi, world);
    }
    return TransformedAabb(new_lo, new_hi);
}

struct InstanceTestResult {
    visible:      bool,
    world_center: vec3<f32>,  // bounds.xyz mapped through the instance's coordinate space
}

/// `inst.bounds`/`aabb` are authored in the instance's *local* (pre-space) frame,
/// same as `inst.transform` — see `GpuInstanceData`. Slot 0 (world space,
/// identity) is the overwhelming common case and takes the exact same path
/// this test always has; only an instance tagged with a real sublevel/portal
/// coordinate space (see `libhelio::coordinate_space`) pays the extra mapping.
fn test_instance(inst: GpuInstance, aabb: GpuAabb) -> InstanceTestResult {
    let space_id = (inst.flags >> 8u) & 0xFFu;
    let aabb_degenerate = all(aabb.min == aabb.max);

    if space_id == 0u {
        // Common case: byte-identical to the pre-coordinate-space behavior.
        if (inst.flags & INSTANCE_FLAG_ALWAYS_VISIBLE) != 0u {
            return InstanceTestResult(true, inst.bounds.xyz);
        }
        let aabb_visible = aabb_in_frustum(aabb.min, aabb.max);
        let sphere_visible = sphere_in_frustum(inst.bounds.xyz, inst.bounds.w);
        let visible = select(sphere_visible, aabb_visible, !aabb_degenerate);
        return InstanceTestResult(visible, inst.bounds.xyz);
    }

    let space = coordinate_spaces[space_id];
    let world_center = (space * vec4<f32>(inst.bounds.xyz, 1.0)).xyz;
    if (inst.flags & INSTANCE_FLAG_ALWAYS_VISIBLE) != 0u {
        return InstanceTestResult(true, world_center);
    }
    if aabb_degenerate {
        return InstanceTestResult(sphere_in_frustum(world_center, inst.bounds.w), world_center);
    }
    let world_aabb = transform_aabb(aabb.min, aabb.max, space);
    let visible = aabb_in_frustum(world_aabb.lo, world_aabb.hi);
    return InstanceTestResult(visible, world_center);
}

@compute @workgroup_size(64)
fn main(
    @builtin(workgroup_id) wg_id: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
) {
    let idx = wg_id.x;
    if idx >= cull.draw_capacity { return; }

    let dc = draw_calls[idx];

    // Cooperatively test every instance in the group across the workgroup's 64
    // lanes (grid-stride loop), compacting survivors into `compacted_indices`
    // via a workgroup-shared atomic counter. `wg_counter`/`wg_nonsubpixel`/
    // `wg_shadow_caster` are zero-initialized per workgroup dispatch by WGSL.
    for (var i = lid.x; i < dc.instance_count; i += 64u) {
        let slot_idx = dc.first_instance + i;
        let inst = instances[slot_idx];
        if (inst.flags & INSTANCE_FLAG_CLASSIC_SUPPRESSED) != 0u {
            continue;
        }
        let aabb = aabbs[slot_idx];
        let result = test_instance(inst, aabb);
        if result.visible {
            let slot = atomicAdd(&wg_counter, 1u);
            compacted_indices[dc.first_instance + slot] = slot_idx;

            // Sub-pixel test: check if this instance projects to ≥ 1 pixel.
            //
            // This is a third rejection path, independent of the frustum and Hi-Z tests:
            // a batch where no instance is marked non-subpixel is dropped wholesale
            // below, so an instance that passes `test_instance` still disappears if it
            // fails to set this flag.
            //
            // It is evaluated at the bounding sphere's *centre* (mapped through the
            // instance's coordinate space, `result.world_center` — see `test_instance`),
            // which is why a large object needs the opt-out: the ground plane's centre is
            // the world origin, so as soon as the camera looks away from the origin
            // `clip_pos.w <= 0.0`, the test is skipped, nothing sets `wg_nonsubpixel`, and
            // the whole ground is culled as "subpixel only" — while covering the screen.
            // Direction-dependent disappearance of large geometry is the signature of this
            // path, not of frustum culling.
            if (inst.flags & INSTANCE_FLAG_ALWAYS_VISIBLE) != 0u {
                atomicStore(&wg_nonsubpixel, 1u);
            } else {
                let clip_pos = cameras[0].view_proj * vec4<f32>(result.world_center, 1.0);
                // A frustum-visible sphere whose centre is behind the eye can
                // still cover a substantial part of the screen (this is
                // common for the cathedral's large origin-centred meshes
                // while the camera flies inside them).  Treat it as
                // non-subpixel instead of letting the batch-level gate drop
                // the entire draw when the centre projection is undefined.
                if clip_pos.w <= 0.0 {
                    atomicStore(&wg_nonsubpixel, 1u);
                } else {
                    let r_ndc = abs(inst.bounds.w * cameras[0].proj[1][1] / clip_pos.w);
                    if r_ndc >= 0.001 {
                        atomicStore(&wg_nonsubpixel, 1u);
                    }
                }
            }
            if (inst.flags & 1u) != 0u {
                atomicStore(&wg_shadow_caster, 1u);
            }
        }
    }

    workgroupBarrier();

    if lid.x != 0u { return; }

    let visible_count = atomicLoad(&wg_counter);
    let any_visible = visible_count > 0u;
    let subpixel_only = any_visible && atomicLoad(&wg_nonsubpixel) == 0u;
    let batch_has_shadow_caster = atomicLoad(&wg_shadow_caster) != 0u;

    if batch_has_shadow_caster {
        atomicAdd(&stats[5u], 1u);
    }

    if any_visible && !subpixel_only {
        indirect[idx] = DrawIndexedIndirect(
            dc.index_count,
            visible_count,
            dc.first_index,
            dc.vertex_offset,
            dc.first_instance,
        );
        atomicAdd(&stats[3u], 1u);
        if batch_has_shadow_caster {
            atomicAdd(&stats[6u], 1u);
        }
    } else {
        indirect[idx] = DrawIndexedIndirect(
            dc.index_count,
            0u,
            dc.first_index,
            dc.vertex_offset,
            dc.first_instance,
        );
        if !any_visible {
            atomicAdd(&stats[1u], 1u);
        } else {
            atomicAdd(&stats[2u], 1u);
        }
    }
    atomicAdd(&stats[0u], 1u);
}
