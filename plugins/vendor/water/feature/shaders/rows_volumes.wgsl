// Water volume rows' placement (`helio_scene_join::Placement`): what a water
// volume needs beyond the join's owner-oriented world box.
//
// `helio_pass_water_sim::GpuWaterVolume`: `sun_direction` and `world_phase`.
const WATER_SUN_WORD: u32 = 44u;
const WATER_PHASE_WORD: u32 = 60u;

// Written after the join copied the source row: the box's `bounds_max.w` is
// the surface height (the owner's Y plus the source size's `w`, scaled like the
// box), the sun the lights' directional light (the direction toward it, `w` 1;
// straight up with `w` 0 when the scene has none), and the frame's phase in the
// water's world tiling (the row's own z/w, the bounded flag, stay as its source
// wrote them).
fn placement_write(row: u32, output: u32) {
    let t = transforms[owners[row].owner_index];
    let scale = object_scale(t);
    let center = placed_position(t);
    let surface = center.y + bitcast<f32>(sources[source_base(row) + 3u]) * scale.y;
    rows_out[output + 7u] = bitcast<u32>(surface);

    var sun = vec4<f32>(0.0, 1.0, 0.0, 0.0);
    if arrayLength(&directional_light) > 0u && directional_light[0].w > 0.0 {
        sun = directional_light[0];
    }
    for (var i = 0u; i < 4u; i++) {
        rows_out[output + WATER_SUN_WORD + i] = bitcast<u32>(sun[i]);
    }

    rows_out[output + WATER_PHASE_WORD] = bitcast<u32>(u.phase.x);
    rows_out[output + WATER_PHASE_WORD + 1u] = bitcast<u32>(u.phase.y);
}
