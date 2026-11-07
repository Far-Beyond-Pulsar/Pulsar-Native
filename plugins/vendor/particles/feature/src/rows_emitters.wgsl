// Particle emitter rows' placement (`helio_scene_join::Placement`): the owner's
// transform, and the next range of the particle pool.
//
// A pooled row (the particles plugin's `GpuCoronaEmitter`): `particle_offset`,
// `particle_count` (the requested range in the source row) and `spawn_cursor`,
// which the pass reads as the emitter's identity.
const EMITTER_OFFSET_WORD: u32 = 45u;
const EMITTER_COUNT_WORD: u32 = 46u;
const EMITTER_IDENTITY_WORD: u32 = 47u;
// The pool's range alignment (`CORONA_RANGE_ALIGNMENT`).
const EMITTER_RANGE_ALIGNMENT: u32 = 256u;

// A particle emitter's transform (`mat4x4`, column-major, words 0..15 of the
// output row): its owner's, so particles spawn at the owner.
fn placement_write(row: u32, output: u32) {
    let t = transforms[owners[row].owner_index];
    let r = object_rotation(t);
    let scale = object_scale(t);
    let center = placed_position(t);
    for (var i = 0u; i < 3u; i++) {
        let axis = r[i] * scale[i];
        for (var j = 0u; j < 3u; j++) {
            rows_out[output + i * 4u + j] = bitcast<u32>(axis[j]);
        }
        rows_out[output + i * 4u + 3u] = 0u;
        rows_out[output + 12u + i] = bitcast<u32>(center[i]);
    }
    rows_out[output + 15u] = bitcast<u32>(1.0);
}

// The pool particles source `row` takes: its request, rounded up to the range
// alignment so the next range starts on it.
fn placement_request(row: u32) -> u32 {
    let requested = sources[row * u.source_words + EMITTER_COUNT_WORD];
    return (requested + EMITTER_RANGE_ALIGNMENT - 1u) / EMITTER_RANGE_ALIGNMENT * EMITTER_RANGE_ALIGNMENT;
}

// Gives output row `slot` (from source `row`) the pool range starting at
// `offset`: its request, clamped to what is left of the pool (none past its end),
// and the emitter's identity (its source row + 1): when another emitter takes the
// slot, the pass restarts the slot's particles.
fn placement_range(row: u32, slot: u32, offset: u32) {
    let output = slot * u.output_words;
    if output + u.output_words > arrayLength(&rows_out) {
        return;
    }
    let requested = sources[row * u.source_words + EMITTER_COUNT_WORD];
    var count = 0u;
    if offset < u.pool {
        count = min(requested, u.pool - offset);
    }
    rows_out[output + EMITTER_OFFSET_WORD] = min(offset, u.pool);
    rows_out[output + EMITTER_COUNT_WORD] = count;
    rows_out[output + EMITTER_IDENTITY_WORD] = row + 1u;
}
