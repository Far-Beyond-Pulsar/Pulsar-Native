// Tier-3 footprint: every tile that a cluster the cull routed to the dense bin
// can touch. A dense-bin cluster is not drawn by Tier 1/2 (the cull diverts it
// because its neighbourhood was dense last frame), but its bounding sphere can
// cover tiles that are not themselves dense; those pixels would stay empty.
// Tier 3 therefore resolves the footprint too (`fp_mask` starts as the union of
// the dense masks, see `mask_union.wgsl`, and is only ever set here).
//
//   footprint_args  one thread: total dense-bin clusters and the indirect size
//   footprint       one thread per dense-bin cluster (2D split like `bin`)
//
// Needs `FP_DENSE_BIN` (the cull's dense bin index), prepended by the host.
@group(0) @binding(0) var<uniform> params: DenseParams;
@group(0) @binding(12) var<storage, read_write> fp_mask: array<atomic<u32>>;
@group(0) @binding(13) var<storage, read_write> fp_args: array<u32>;
// Total dense-bin clusters. Kept out of `fp_args` so the `footprint` dispatch,
// which uses `fp_args` as its indirect source, does not also bind it as storage.
@group(0) @binding(14) var<storage, read_write> fp_total: array<u32>;

const FP_BINS: u32 = CULL_RANGE_COUNT / 2u;

fn fp_first(r: u32) -> u32 {
    return cull_state[CULL_RANGES + r * 2u];
}

// Count of range `r`, never reading past the visible list the caller sized
// (the same clamp as `make_args`).
fn fp_count(r: u32) -> u32 {
    let first = fp_first(r);
    let cap = select(0u, params.limits.z - min(first, params.limits.z), params.limits.z > 0u);
    return min(cull_state[CULL_RANGES + r * 2u + 1u], cap);
}

@compute @workgroup_size(1)
fn footprint_args() {
    var total = 0u;
    for (var r = 0u; r < CULL_RANGE_COUNT; r++) {
        if r % FP_BINS == FP_DENSE_BIN { total += fp_count(r); }
    }
    let groups = (total + 63u) / 64u;
    fp_args[0] = min(groups, BIN_ROW_GROUPS);
    fp_args[1] = max((groups + BIN_ROW_GROUPS - 1u) / BIN_ROW_GROUPS, 1u);
    fp_args[2] = 1u;
    fp_total[0] = total;
}

@compute @workgroup_size(64)
fn footprint(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) l: vec3<u32>) {
    let i = (wid.y * BIN_ROW_GROUPS + wid.x) * 64u + l.x;
    if i >= fp_total[0] { return; }
    var acc = 0u;
    var u = 0u;
    var found = false;
    for (var r = 0u; r < CULL_RANGE_COUNT; r++) {
        if r % FP_BINS != FP_DENSE_BIN { continue; }
        let c = fp_count(r);
        if i < acc + c {
            u = fp_first(r) + (i - acc);
            found = true;
            break;
        }
        acc += c;
    }
    if !found { return; }
    let v = visible[u];
    let c = vgd_load_cluster(v.cluster_slot);
    let rect = cluster_tile_rect(v, c);
    if !rect.ok { return; }
    let tiles_x = params.viewport.z;
    for (var ty = rect.ty0; ty <= rect.ty1; ty++) {
        for (var tx = rect.tx0; tx <= rect.tx1; tx++) {
            atomicStore(&fp_mask[ty * tiles_x + tx], 1u);
        }
    }
}
