// Scene-side inputs of the Tier-3-NonRT kernels (reconciled in T10a).
//
//   cull_state       helio-pass-vg-cull `state` words: the per-phase, per-bin
//                    {first, count} ranges of the unified visible list live at
//                    word CULL_RANGES + phase * 12 + bin * 2.
//   visible          the cull's `visible_clusters` {instance, cluster_slot},
//                    indexed by the UNIFIED index (`bin * bin_capacity + k`),
//                    which is also the `vis_pack` cluster index of the ids.
//   vg_page_pool     helio-vg-stream page pool        (binding 3, decode_bindings_wgsl)
//   vg_asset_quant   helio-vg-stream lattice per asset (binding 4)
//   vg_instances     GpuVgInstance rows (asset_slot, object_row)
//   vg_objects       scene-join `static_objects`, 252-byte rows (transform = words 4..20)
//
// All tiers' clusters are binned, not just the dense bin: a dense tile
// REPLACES whatever Tier 1/2 drew there, so it must see every overlapping
// cluster of both cull phases.
@group(0) @binding(1) var<storage, read> cull_state: array<u32>;
@group(0) @binding(2) var<storage, read> visible: array<VgVisibleCluster>;
@group(0) @binding(5) var<storage, read> vg_instances: array<VgInstance>;

struct DenseObject {
    head:      array<u32, 4>,
    transform: array<array<f32, 4>, 4>,
    rest:      array<u32, 43>,
}
@group(0) @binding(10) var<storage, read> vg_objects: array<DenseObject>;

fn dense_xform(v: VgVisibleCluster) -> mat4x4<f32> {
    let t = vg_objects[vg_instances[v.instance].object_row].transform;
    return mat4x4<f32>(
        vec4<f32>(t[0][0], t[0][1], t[0][2], t[0][3]),
        vec4<f32>(t[1][0], t[1][1], t[1][2], t[1][3]),
        vec4<f32>(t[2][0], t[2][1], t[2][2], t[2][3]),
        vec4<f32>(t[3][0], t[3][1], t[3][2], t[3][3]),
    );
}

fn dense_asset(v: VgVisibleCluster) -> u32 {
    return vg_instances[v.instance].asset_slot;
}

// Cluster-list entries (`slot_lists`) are unified visible indices; bit 31 tags
// an entry that came from a masked (alpha-tested) cull bin, so the opaque and
// the masked peel pipelines each take their own class.
const LIST_MASKED_BIT: u32 = 0x80000000u;
const LIST_INDEX_MASK: u32 = 0x7fffffffu;
