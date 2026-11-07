// The query stage shares the exact authored surface functions with the water
// renderer. Keeping only the entry point here prevents a second implementation
// of FFT cascade/shore damping/interaction sampling from drifting over time.
struct WaterQueryInput {
    position_cell: vec4f,
    identity_volume: vec4u,
}

struct WaterQueryOutput {
    // xyz = displaced frame-relative surface position; w = valid.
    surface_position_valid: vec4f,
    // xyz = surface normal; w reserved.
    normal_valid: vec4f,
    // stable body key low/high, point index, valid marker.
    identity: vec4u,
}

@group(0) @binding(21) var<storage, read> water_query_inputs: array<WaterQueryInput>;
@group(0) @binding(22) var<storage, read_write> water_query_outputs: array<WaterQueryOutput>;

@compute @workgroup_size(64)
fn cs_water_query(@builtin(global_invocation_id) gid: vec3u) {
    let query_index = gid.x;
    if query_index >= arrayLength(&water_query_inputs) {
        return;
    }
    let input = water_query_inputs[query_index];
    let volume_index = input.identity_volume.w;
    if volume_index >= arrayLength(&volumes) {
        water_query_outputs[query_index] = WaterQueryOutput(
            vec4f(0.0), vec4f(0.0), vec4u(input.identity_volume.xyz, 0u));
        return;
    }

    let xz = input.position_cell.xz;
    let cell_size = max(input.position_cell.w, 1.0 / 16.0);
    let volume = volumes[volume_index];
    let extent = volume.bounds_max.xz - volume.bounds_min.xz;
    if any(extent <= vec2f(1e-4)) {
        water_query_outputs[query_index] = WaterQueryOutput(
            vec4f(0.0), vec4f(0.0), vec4u(input.identity_volume.xyz, 0u));
        return;
    }
    let displacement = water_cascade_displacement(xz, cell_size);
    let height = volume.bounds_max.w
        + water_cascade_height_sum(xz, volume_index, cell_size);
    let normal = water_total_normal(xz, volume, volume_index, cell_size);
    water_query_outputs[query_index] = WaterQueryOutput(
        vec4f(xz.x + displacement.x, height, xz.y + displacement.z, 1.0),
        vec4f(normal, 0.0),
        vec4u(input.identity_volume.xyz, 1u));
}
