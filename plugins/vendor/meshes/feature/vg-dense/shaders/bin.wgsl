// Tier-3-NonRT stage 1: bin visible clusters into the cluster lists of the
// dense tiles their bounding sphere can touch. One thread per visible cluster.
// The tile rectangle is `cluster_tile_rect` (bounds.wgsl).
//
// Three kernels, so every tile's list is exactly as long as it needs to be
// (a fixed per-tile capacity dropped geometry on crowds):
//   bin_count   per (cluster, tile): count[slot] += 1
//   bin_scan    one thread: start[slot] = running sum; counts are clamped to
//               what is left of the shared pool (the excess is counted in
//               STAT_BIN_OVERFLOW: the only way geometry is dropped)
//   bin_fill    per (cluster, tile): pool[start + cursor++] = entry
// `slot_lists` layout: LIST_HDR words per slot [count, start, cursor], then the
// pool at LIST_POOL_BASE.
@group(0) @binding(0) var<uniform> params: DenseParams;
@group(0) @binding(6) var<storage, read> tile_slot: array<u32>;
@group(0) @binding(7) var<storage, read_write> slot_lists: array<atomic<u32>>;
@group(0) @binding(8) var<storage, read_write> state: array<atomic<u32>>;
@group(0) @binding(11) var<storage, read> bin_args: array<u32>;

fn bin_visit(i: u32, fill: bool) {
    // Locate range r with prefix[r] <= i < prefix[r + 1] (args[20..33] hold the
    // prefix, args[8..20] the `first` of each cull range, see args.wgsl).
    let total = bin_args[ARGS_PREFIX + CULL_RANGE_COUNT];
    if i >= total { return; }
    var rg = 0u;
    for (var k = 1u; k < CULL_RANGE_COUNT; k++) {
        if bin_args[ARGS_PREFIX + k] <= i && bin_args[ARGS_PREFIX + k + 1u] > i { rg = k; }
    }
    let u = bin_args[8u + rg] + (i - bin_args[ARGS_PREFIX + rg]);
    // Range `rg` is `phase * BIN_COUNT + bin`: bins from CULL_FIRST_MASKED hold
    // alpha-tested clusters.
    let masked = (rg % (CULL_RANGE_COUNT / 2u)) >= CULL_FIRST_MASKED;
    // limits.w bit 0: only the alpha-masked class (the ray-traced backend
    // resolves the opaque class itself).
    if (params.limits.w & 1u) != 0u && !masked { return; }
    // limits.w bit 1: only the clusters the cull diverted to the dense bin.
    if (params.limits.w & 2u) != 0u && (rg % (CULL_RANGE_COUNT / 2u)) != BIN_DENSE_BIN { return; }
    let v = visible[u];
    let c = vgd_load_cluster(v.cluster_slot);
    let rect = cluster_tile_rect(v, c);
    if !rect.ok { return; }
    let tiles_x = params.viewport.z;
    let cls = select(0u, 1u, masked);
    for (var ty = rect.ty0; ty <= rect.ty1; ty++) {
        for (var tx = rect.tx0; tx <= rect.tx1; tx++) {
            let s = tile_slot[ty * tiles_x + tx];
            if s == SLOT_INVALID { continue; }
            let h = s * LIST_HDR;
            if fill {
                let k = atomicAdd(&slot_lists[h + 3u + cls], 1u);
                if k < atomicLoad(&slot_lists[h + cls]) {
                    var start = atomicLoad(&slot_lists[h + 2u]);
                    if masked { start += atomicLoad(&slot_lists[h]); }
                    atomicStore(&slot_lists[LIST_POOL_BASE + start + k], u);
                }
            } else {
                atomicAdd(&slot_lists[h + cls], 1u);
            }
        }
    }
}

@compute @workgroup_size(64)
fn bin(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) l: vec3<u32>) {
    bin_visit((wid.y * BIN_ROW_GROUPS + wid.x) * 64u + l.x, false);
}

@compute @workgroup_size(64)
fn bin_fill(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) l: vec3<u32>) {
    bin_visit((wid.y * BIN_ROW_GROUPS + wid.x) * 64u + l.x, true);
}

// Serial prefix over the dense slots (at most `max_dense_slots`, typically tens).
@compute @workgroup_size(1)
fn bin_scan() {
    let n = min(atomicLoad(&state[STATE_DENSE_COUNT]), params.limits.y);
    var acc = 0u;
    for (var s = 0u; s < n; s++) {
        let h = s * LIST_HDR;
        let c_op = atomicLoad(&slot_lists[h]);
        let c_m = atomicLoad(&slot_lists[h + 1u]);
        atomicAdd(&state[STAT_LIST_TOTAL], c_op + c_m);
        atomicMax(&state[STAT_LIST_MAX], c_op + c_m);
        let start = acc;
        let e_op = min(c_op, LIST_POOL_CAP - acc);
        acc += e_op;
        let e_m = min(c_m, LIST_POOL_CAP - acc);
        acc += e_m;
        atomicStore(&slot_lists[h], e_op);
        atomicStore(&slot_lists[h + 1u], e_m);
        atomicStore(&slot_lists[h + 2u], start);
        atomicStore(&slot_lists[h + 3u], 0u);
        atomicStore(&slot_lists[h + 4u], 0u);
        let lost = (c_op - e_op) + (c_m - e_m);
        if lost > 0u { atomicAdd(&state[STAT_BIN_OVERFLOW], lost); }
    }
}
