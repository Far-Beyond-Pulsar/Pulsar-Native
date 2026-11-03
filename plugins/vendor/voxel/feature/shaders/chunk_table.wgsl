// Chunk table updates (docs/chunk-octree.md, `chunks/drawn.rs`): the
// entries that changed, uploaded at once and scattered into the table.
// changes[0].x is their count; change n is changes[1 + 3n] (x: the entry's
// index), its key, its value.

@group(0) @binding(0) var<storage, read> changes: array<vec4<i32>>;
@group(0) @binding(1) var<storage, read_write> chunk_table: array<vec4<i32>>;

@compute @workgroup_size(64)
fn table_update(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= u32(changes[0].x) { return; }
    let at = 1u + id.x * 3u;
    let entry = u32(changes[at].x);
    chunk_table[entry * 2u] = changes[at + 1u];
    chunk_table[entry * 2u + 1u] = changes[at + 2u];
}
