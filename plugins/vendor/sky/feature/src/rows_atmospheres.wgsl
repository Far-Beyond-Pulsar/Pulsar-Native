// Atmosphere rows' placement (`helio_scene_join::Placement`): a planet placed
// at its owner is centred on the owner's position.
const PLACEMENT_CENTER: u32 = 1u;

// Planet centres are world anchors (an atmosphere row's `center`), which the
// pass rebases: the owner's world position is added to the row's own centre
// (words 0..2) when word 3 says so.
fn placement_write(row: u32, output: u32) {
    let source = source_base(row);
    if sources[source + 3u] == PLACEMENT_CENTER {
        let center = object_position(transforms[owners[row].owner_index]);
        rows_out[output] = bitcast<u32>(bitcast<f32>(sources[source]) + center.x);
        rows_out[output + 1u] = bitcast<u32>(bitcast<f32>(sources[source + 1u]) + center.y);
        rows_out[output + 2u] = bitcast<u32>(bitcast<f32>(sources[source + 2u]) + center.z);
    }
}
