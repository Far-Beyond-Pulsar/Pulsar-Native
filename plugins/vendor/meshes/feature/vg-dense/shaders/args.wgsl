// Indirect dispatch arguments from the compaction count and the cull's ranges.
//   args[0..3]    peel/trace: one workgroup per dense tile
//   args[3..6]    bin: one thread per visible cluster; 2D split with rows of
//                 BIN_ROW_GROUPS (32768) workgroups of 64 threads, thread
//                 index (y * BIN_ROW_GROUPS + x) * 64 + lane, guarded < total
//   args[8..8+R]    `first` of each of the R cull ranges (2 phases x CULL_BIN_COUNT bins)
//   args[8+R..9+2R] exclusive prefix of the range counts; the last = total clusters
//                 (`ARGS_PREFIX` = 8 + R)
@group(0) @binding(0) var<uniform> params: DenseParams;
@group(0) @binding(1) var<storage, read_write> state: array<atomic<u32>>;
@group(0) @binding(2) var<storage, read> cull_state: array<u32>;
@group(0) @binding(3) var<storage, read_write> args: array<u32>;


@compute @workgroup_size(1)
fn make_args() {
    let n = min(atomicLoad(&state[STATE_DENSE_COUNT]), params.limits.y);
    args[0] = n;
    args[1] = 1u;
    args[2] = 1u;
    var total = 0u;
    for (var r = 0u; r < CULL_RANGE_COUNT; r++) {
        let first = cull_state[CULL_RANGES + r * 2u];
        // Never read past the visible list the caller sized.
        let count = min(cull_state[CULL_RANGES + r * 2u + 1u], select(0u, params.limits.z - min(first, params.limits.z), params.limits.z > 0u));
        args[8u + r] = first;
        args[ARGS_PREFIX + r] = total;
        total += count;
    }
    args[ARGS_PREFIX + CULL_RANGE_COUNT] = total;
    let groups = (total + 63u) / 64u;
    args[3] = min(groups, BIN_ROW_GROUPS);
    args[4] = max((groups + BIN_ROW_GROUPS - 1u) / BIN_ROW_GROUPS, 1u);
    args[5] = 1u;
    // Nothing dense: the binning and candidate kernels have nothing to do.
    if n == 0u { args[3] = 0u; }
}

// RT variant: no visible list.
@compute @workgroup_size(1)
fn make_args_rt() {
    let n = min(atomicLoad(&state[STATE_DENSE_COUNT]), params.limits.y);
    args[0] = n;
    args[1] = 1u;
    args[2] = 1u;
}
