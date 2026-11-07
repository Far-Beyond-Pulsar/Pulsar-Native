// Sprite rows' placement (`helio_scene_join::Placement`): a 2D sprite (the
// sprites plugin's `SpriteComponent`) at its owner's X and Y, turned by its roll
// (rotation about Z) and its size scaled by its X and Y scale. The owner's Z and
// other rotations do not apply.
fn placement_write(row: u32, output: u32) {
    let t = transforms[owners[row].owner_index];
    let scale = abs(object_scale(t));
    let roll = t.rotation[2] * 0.017453292519943295;
    let position = object_position(t);
    rows_out[output] = bitcast<u32>(position.x);
    rows_out[output + 1u] = bitcast<u32>(position.y);
    rows_out[output + 2u] = bitcast<u32>(bitcast<f32>(rows_out[output + 2u]) * scale.x);
    rows_out[output + 3u] = bitcast<u32>(bitcast<f32>(rows_out[output + 3u]) * scale.y);
    rows_out[output + 4u] = bitcast<u32>(bitcast<f32>(rows_out[output + 4u]) + roll);
}
