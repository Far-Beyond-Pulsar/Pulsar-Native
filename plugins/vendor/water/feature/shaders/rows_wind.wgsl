// The global wind's placement, over the water volume rows (`RowTable::over`).
//
// `helio_pass_water_sim::GpuWaterVolume`: `wind_params` (`w`: the volume opts
// out of the global wind).
const WATER_WIND_WORD: u32 = 56u;
const WATER_OWN_WIND_WORD: u32 = 59u;
// `scene_rows.rs`'s `WATER_WIND_STRENGTH_PER_SPEED`: a 2 m/s breeze makes
// gentle ripples (1), 10 m/s a choppy surface (5).
const WATER_WIND_PER_SPEED: f32 = 0.5;

// The global wind (source `row`, a foliage wind row: normalised direction and
// speed) as the wind of every placed water row that does not opt out of it:
// its direction's XZ, and a strength proportional to its speed. The first
// placed global wind (a level has one) writes; this writes no row of its own.
fn placement_write_row(row: u32, slot: u32, output: u32) -> bool {
    if slot != 0u {
        return true;
    }
    let source = row * u.source_words;
    let direction = vec2<f32>(bitcast<f32>(sources[source]), bitcast<f32>(sources[source + 2u]));
    let speed = max(bitcast<f32>(sources[source + 3u]), 0.0);
    let slots = min(u.capacity, arrayLength(&rows_out) / u.output_words);
    for (var volume = 0u; volume < slots; volume++) {
        let target_row = volume * u.output_words;
        // An unplaced row is zero; a placed one has a non-empty box.
        var placed_volume = false;
        for (var i = 0u; i < 3u; i++) {
            placed_volume = placed_volume || rows_out[target_row + i] != rows_out[target_row + 4u + i];
        }
        if !placed_volume || bitcast<f32>(rows_out[target_row + WATER_OWN_WIND_WORD]) != 0.0 {
            continue;
        }
        rows_out[target_row + WATER_WIND_WORD] = bitcast<u32>(direction.x);
        rows_out[target_row + WATER_WIND_WORD + 1u] = bitcast<u32>(direction.y);
        rows_out[target_row + WATER_WIND_WORD + 2u] = bitcast<u32>(speed * WATER_WIND_PER_SPEED);
    }
    return true;
}
