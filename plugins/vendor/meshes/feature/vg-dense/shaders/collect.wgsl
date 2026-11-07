// Tier-3-RT candidate collection: which VG instances have visible clusters
// overlapping a dense tile this frame, with the rows the CPU needs to build a
// TLAS (transform, asset slot, error scale). One thread per visible cluster
// (same dispatch as `bin`). GPU-side dedup through a per-instance bitmap, so a
// readback carries O(dense instances) records, never O(all instances).
//
//   inst_bits  one bit per `vg_instances` row, cleared every frame
//   cand       [0] distinct dense instances seen (may exceed capacity)
//              [1] of those, not in `covered` (the TLAS' current instance set)
//              [2] alpha-masked clusters overlapping a dense tile (diagnostic;
//                  proxies trace their geometry as opaque surfaces)
//              [3] spare; records of 16 words from word 4:
//              instance, asset_slot, bitcast(lod_error_scale), 0,
//              then the row-major 3x4 object-to-world transform (12 f32)
//   covered    one bit per instance: instances already in the TLAS
@group(0) @binding(0) var<uniform> params: DenseParams;
@group(0) @binding(6) var<storage, read> tile_slot: array<u32>;
@group(0) @binding(8) var<storage, read_write> state: array<atomic<u32>>;
@group(0) @binding(11) var<storage, read> bin_args: array<u32>;
@group(0) @binding(12) var<storage, read_write> inst_bits: array<atomic<u32>>;
@group(0) @binding(13) var<storage, read_write> cand: array<atomic<u32>>;
@group(0) @binding(14) var<storage, read> covered: array<u32>;

const CAND_HEADER: u32 = 4u;
const CAND_STRIDE: u32 = 16u;

@compute @workgroup_size(64)
fn collect(@builtin(workgroup_id) wid: vec3<u32>, @builtin(local_invocation_id) l: vec3<u32>) {
    let i = (wid.y * BIN_ROW_GROUPS + wid.x) * 64u + l.x;
    let total = bin_args[ARGS_PREFIX + CULL_RANGE_COUNT];
    if i >= total { return; }
    if atomicLoad(&state[STATE_DENSE_COUNT]) == 0u { return; }
    var rg = 0u;
    for (var k = 1u; k < CULL_RANGE_COUNT; k++) {
        if bin_args[ARGS_PREFIX + k] <= i && bin_args[ARGS_PREFIX + k + 1u] > i { rg = k; }
    }
    let u = bin_args[8u + rg] + (i - bin_args[ARGS_PREFIX + rg]);
    let v = visible[u];
    let inst = v.instance;
    let word = inst >> 5u;
    if word >= arrayLength(&inst_bits) { return; }
    let bit = 1u << (inst & 31u);
    let masked = (rg % (CULL_RANGE_COUNT / 2u)) >= CULL_FIRST_MASKED;
    // limits.w bit 1 optionally restricts collection to the dense and SW bins.
    // The RT replacement path leaves this off so HW/SW/dense tiers all seed
    // the TLAS when their cluster bounds touch a selected tile.
    let bin_of = rg % (CULL_RANGE_COUNT / 2u);
    if (params.limits.w & 2u) != 0u && bin_of != BIN_DENSE_BIN && bin_of != BIN_SW_BIN { return; }
    // Count masked geometry for diagnostics, but still collect its instance:
    // RT replacement traces every proxy triangle. Proxy alpha-texture support
    // is not available yet, so those surfaces are geometrically opaque.
    if masked { atomicAdd(&cand[2], 1u); }
    if (atomicLoad(&inst_bits[word]) & bit) != 0u { return; }
    let c = vgd_load_cluster(v.cluster_slot);
    let rect = cluster_tile_rect(v, c);
    if !rect.ok { return; }
    let tiles_x = params.viewport.z;
    var hit = false;
    for (var ty = rect.ty0; ty <= rect.ty1 && !hit; ty++) {
        for (var tx = rect.tx0; tx <= rect.tx1; tx++) {
            if tile_slot[ty * tiles_x + tx] != SLOT_INVALID {
                hit = true;
                break;
            }
        }
    }
    if !hit { return; }
    let old = atomicOr(&inst_bits[word], bit);
    if (old & bit) != 0u { return; }
    let idx = atomicAdd(&cand[0], 1u);
    var is_covered = false;
    if word < arrayLength(&covered) {
        is_covered = (covered[word] & bit) != 0u;
    }
    if !is_covered {
        atomicAdd(&cand[1], 1u);
    }
    let cap = (arrayLength(&cand) - CAND_HEADER) / CAND_STRIDE;
    if idx >= cap { return; }
    let m = dense_xform(v);
    let base = CAND_HEADER + idx * CAND_STRIDE;
    let vi = vg_instances[inst];
    atomicStore(&cand[base], inst);
    atomicStore(&cand[base + 1u], vi.asset_slot);
    atomicStore(&cand[base + 2u], bitcast<u32>(vi.lod_error_scale));
    atomicStore(&cand[base + 3u], 0u);
    for (var r = 0u; r < 3u; r++) {
        atomicStore(&cand[base + 4u + r * 4u], bitcast<u32>(m[0][r]));
        atomicStore(&cand[base + 5u + r * 4u], bitcast<u32>(m[1][r]));
        atomicStore(&cand[base + 6u + r * 4u], bitcast<u32>(m[2][r]));
        atomicStore(&cand[base + 7u + r * 4u], bitcast<u32>(m[3][r]));
    }
}
