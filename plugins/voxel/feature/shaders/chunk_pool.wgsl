// The leaf pool grown (`ChunkWorld::grow_leaves`): its new pages are on the
// free stack already, under the old ones; the stack's count rises by them
// after whatever the GPU took or gave back before.

@group(0) @binding(0) var<storage, read_write> leaf_alloc: array<atomic<i32>>;
@group(0) @binding(1) var<storage, read> added: array<i32>;

@compute @workgroup_size(1)
fn pool_grow() {
    atomicAdd(&leaf_alloc[0], added[0]);
}
