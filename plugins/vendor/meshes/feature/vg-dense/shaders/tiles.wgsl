// Dense-tile compaction shared by 3-NonRT and 3-RT.
//
// `tile_mask` is T7's `tile_classify` output. PLACEHOLDER contract: one u32
// per 16x16 tile (row-major, tiles_x * tiles_y entries), nonzero = dense. T7
// may publish a bitset or list instead; replace `tile_is_dense` only.
@group(0) @binding(0) var<uniform> params: DenseParams;
@group(0) @binding(1) var<storage, read> tile_mask: array<u32>;
@group(0) @binding(2) var<storage, read_write> tile_slot: array<u32>;
@group(0) @binding(3) var<storage, read_write> state: array<atomic<u32>>;
@group(0) @binding(4) var<storage, read_write> slot_lists: array<atomic<u32>>;

fn tile_is_dense(t: u32) -> bool {
    return tile_mask[t] != 0u;
}

fn claim_slot(t: u32) -> u32 {
    if !tile_is_dense(t) {
        tile_slot[t] = SLOT_INVALID;
        return SLOT_INVALID;
    }
    let slot = atomicAdd(&state[STATE_DENSE_COUNT], 1u);
    if slot >= params.limits.y {
        atomicAdd(&state[STAT_SLOT_OVERFLOW], 1u);
        tile_slot[t] = SLOT_INVALID;
        return SLOT_INVALID;
    }
    tile_slot[t] = slot;
    atomicStore(&state[STATE_HEADER + slot], t);
    return slot;
}

// 3-NonRT: also resets the slot's cluster list.
@compute @workgroup_size(64)
fn compact(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.limits.x { return; }
    let slot = claim_slot(gid.x);
    if slot != SLOT_INVALID {
        for (var h = 0u; h < LIST_HDR; h++) {
            atomicStore(&slot_lists[slot * LIST_HDR + h], 0u);
        }
    }
}

// 3-RT: no per-slot lists.
@compute @workgroup_size(64)
fn compact_tiles_only(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= params.limits.x { return; }
    claim_slot(gid.x);
}
