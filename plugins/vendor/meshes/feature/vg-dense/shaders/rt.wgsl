// Tier-3-RT: primary rays over dense tiles against a TLAS of VG instance
// proxies (BLASes built from a streamed proxy LOD, see `src/rt.rs`).
// The caller prepends `enable wgpu_ray_query;` (it must be the first token).
//
// Writes the same `dense_key` format as the NonRT peel (depth bits, vis id) so
// T9 treats both Tier-3 backends identically, plus the hardware barycentrics
// packed like `bary_pack` (u | v << 16, v0 = 1 - u - v) into `dense_bary`.
//
// Vis ids: proxy BLAS primitives map (via `prim_map`) to a proxy-cluster-local
// ordinal and the triangle index inside that real cluster, so the id is a
// genuine (cluster, triangle) pair of the asset. The cluster field is
// `visible_base + ordinal` and `visible_base >= DENSE_RT_VISIBLE_BASE`: it
// indexes the RT visible table (`GpuVisibleCluster` rows), not T5's list.
@group(0) @binding(0) var<uniform> params: DenseParams;
@group(0) @binding(1) var acc_struct: acceleration_structure;
// One row per TLAS instance, in TLAS instance order.
struct RtInstance {
    visible_base: u32,
    prim_map_offset: u32,
    instance_id: u32,
    flags: u32,
}
@group(0) @binding(2) var<storage, read> rt_instances: array<RtInstance>;
// Per proxy primitive: (proxy cluster ordinal << 7) | triangle in cluster.
@group(0) @binding(3) var<storage, read> prim_map: array<u32>;
@group(0) @binding(8) var<storage, read_write> state: array<atomic<u32>>;
@group(0) @binding(9) var<storage, read_write> dense_key: array<vec2<u32>>;
@group(0) @binding(10) var<storage, read_write> dense_bary: array<u32>;

fn pack_bary(u: f32, v: f32) -> u32 {
    let qu = u32(round(clamp(u, 0.0, 1.0) * 65535.0));
    let qv = u32(round(clamp(v, 0.0, 1.0) * 65535.0));
    return qu | (qv << 16u);
}

@compute @workgroup_size(TILE, TILE)
fn trace(
    @builtin(local_invocation_id) l3: vec3<u32>,
    @builtin(workgroup_id) wid: vec3<u32>,
) {
    let tile = atomicLoad(&state[STATE_HEADER + wid.x]);
    if l3.x == 0u && l3.y == 0u {
        atomicAdd(&state[STAT_TILES], 1u);
    }
    let tiles_x = params.viewport.z;
    let pix = vec2<u32>(tile % tiles_x, tile / tiles_x) * TILE + l3.xy;
    if pix.x >= params.viewport.x || pix.y >= params.viewport.y { return; }
    let idx = pix.y * params.viewport.x + pix.x;

    let fw = f32(params.viewport.x);
    let fh = f32(params.viewport.y);
    let ndc = vec2<f32>((f32(pix.x) + 0.5) / fw * 2.0 - 1.0, 1.0 - (f32(pix.y) + 0.5) / fh * 2.0);
    // Reversed-Z: the near plane is z = 1.
    let nearp = params.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let origin = params.camera_pos.xyz;
    let dir = normalize(nearp.xyz / nearp.w - origin);

    var q: ray_query;
    rayQueryInitialize(&q, acc_struct, RayDesc(0x01u, 0xffu, params.misc.x, params.misc.y, origin, dir));
    while rayQueryProceed(&q) {}
    let hit = rayQueryGetCommittedIntersection(&q);
    if hit.kind == RAY_QUERY_INTERSECTION_NONE {
        dense_key[idx] = vec2<u32>(0u, 0u);
        dense_bary[idx] = 0u;
        return;
    }
    let inst = rt_instances[hit.instance_index];
    let mapped = prim_map[inst.prim_map_offset + hit.primitive_index];
    let id = vg_vis_pack(inst.visible_base + (mapped >> VG_VIS_TRI_BITS), mapped & VG_VIS_TRI_MASK);
    let clip = params.view_proj * vec4<f32>(origin + dir * hit.t, 1.0);
    let z = clip.z / clip.w;
    dense_key[idx] = vg_raster_key(z, id);
    dense_bary[idx] = pack_bary(hit.barycentrics.x, hit.barycentrics.y);
}
