// Decal rows' placement (`helio_scene_join::Placement`): a box whose transform
// maps world space into the owner-placed box.

// A decal needs a size on every axis: a flat box covers nothing.
fn placement_placed(row: u32) -> bool {
    return !any(source_size(row) == vec3<f32>(0.0));
}

// The source starts with the local size `vec4`, which the output row does not
// carry.
fn placement_source_header() -> u32 {
    return 4u;
}

// A decal's world-to-decal transform (`mat4x4`, column-major, words 0..15 of the
// output row): world space into the owner-placed box, whose full local extent is
// the source size, so the box spans -1..1 on each axis. Row `i` of the matrix is
// the box's world axis `i` over its world half extent; the translation takes the
// owner's position to the origin.
fn placement_write(row: u32, output: u32) {
    let t = transforms[owners[row].owner_index];
    let r = object_rotation(t);
    let half = source_size(row) * object_scale(t) * 0.5;
    let center = placed_position(t);
    for (var i = 0u; i < 3u; i++) {
        let axis = r[i] / half[i];
        for (var j = 0u; j < 3u; j++) {
            rows_out[output + j * 4u + i] = bitcast<u32>(axis[j]);
        }
        rows_out[output + 12u + i] = bitcast<u32>(-dot(axis, center));
        rows_out[output + i * 4u + 3u] = 0u;
    }
    rows_out[output + 15u] = bitcast<u32>(1.0);
}
