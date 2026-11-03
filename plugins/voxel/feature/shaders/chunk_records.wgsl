// Chunk records (`ChunkWorld::flush_records`): the frames (`ChunkStatic`,
// 9 vec4s) and keys of the chunks generated, restored or read back this
// frame, uploaded at once and scattered into their slots. records[0].x is
// their count; record n is records[1 + 11n] (x: its slot), its frame, its
// key. A thread a vec4 written.

@group(0) @binding(0) var<storage, read> records: array<vec4<i32>>;
@group(0) @binding(1) var<storage, read_write> statics: array<vec4<i32>>;
@group(0) @binding(2) var<storage, read_write> chunk_keys: array<vec4<i32>>;

@compute @workgroup_size(64)
fn records_update(@builtin(global_invocation_id) id: vec3<u32>) {
    let n = id.x / 10u;
    let part = id.x % 10u;
    if n >= u32(records[0].x) { return; }
    let at = 1u + n * 11u;
    let slot = u32(records[at].x);
    if part < 9u {
        statics[slot * 9u + part] = records[at + 1u + part];
    } else {
        chunk_keys[slot] = records[at + 10u];
    }
}
