// Foliage layer rows' placement (`helio_scene_join::Placement`): a layer is a
// world-aligned square of half extent `source[0]` (scaled by the owner's X and
// Z scale) centred on the owner, spanning the authored world altitudes
// `source[2]..source[3]` in Y; `source[1]` is the infinite-extent flag, which
// the placement pass reads from `bounds_max.w`. Written in the frame's space,
// where placement tests its points, the altitudes included.
fn placement_write_row(row: u32, slot: u32, output: u32) -> bool {
    let source = source_base(row);
    let t = transforms[owners[row].owner_index];
    let center = placed_position(t);
    let scale = abs(object_scale(t));
    let half = bitcast<f32>(sources[source]);
    rows_out[output] = bitcast<u32>(center.x - half * scale.x);
    rows_out[output + 1u] = bitcast<u32>(placed_altitude(bitcast<f32>(sources[source + 2u])));
    rows_out[output + 2u] = bitcast<u32>(center.z - half * scale.z);
    rows_out[output + 3u] = 0u;
    rows_out[output + 4u] = bitcast<u32>(center.x + half * scale.x);
    rows_out[output + 5u] = bitcast<u32>(placed_altitude(bitcast<f32>(sources[source + 3u])));
    rows_out[output + 6u] = bitcast<u32>(center.z + half * scale.z);
    rows_out[output + 7u] = sources[source + 1u];
    return true;
}
