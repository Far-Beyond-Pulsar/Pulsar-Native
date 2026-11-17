// Cable connector placement: preserve the authored ID/enabled words and put
// the owner's camera-relative transform in the row consumed by Cables.
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
    let input = row * u.source_words;
    rows_out[output + 16u] = sources[input + 16u];
    rows_out[output + 17u] = sources[input + 17u];
    rows_out[output + 18u] = 0u;
    rows_out[output + 19u] = 0u;
}
