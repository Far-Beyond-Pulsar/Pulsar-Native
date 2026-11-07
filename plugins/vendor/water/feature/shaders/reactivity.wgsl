// Water's coverage for the TSR resolve (`transparency_reactivity`).
//
// The sea is animated shading with no history of its own: its waves move, its
// glints change every frame, and `gbuffer_velocity` there is the velocity of
// whatever opaque thing lies under it (a static sea floor). The resolve would
// reproject last frame's waves along that velocity and blend them in, which
// smears the surface flat and drags halos off whatever stands in the water.
//
// Where the water surface draw wrote depth, this writes a coverage the
// resolve reads as "drop this much history here". It runs after the
// transparent phase and takes the larger of what is already there (a
// transparent mesh's own coverage) and the water's.

@group(0) @binding(0) var water_surface_depth: texture_depth_2d;

// History confidence dropped per frame under water. 1.0 would be the current
// frame only (aliased wave detail); lower keeps a little temporal smoothing.
const WATER_REACTIVITY: f32 = 0.8;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(p[i], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) clip_pos: vec4<f32>) -> @location(0) vec4<f32> {
    let pix = vec2<i32>(i32(clip_pos.x), i32(clip_pos.y));
    // Reverse-Z: the clear is 0.0 (no surface); any water fragment is nearer.
    if textureLoad(water_surface_depth, pix, 0) <= 0.0 {
        discard;
    }
    return vec4<f32>(WATER_REACTIVITY, 0.0, 0.0, 0.0);
}
