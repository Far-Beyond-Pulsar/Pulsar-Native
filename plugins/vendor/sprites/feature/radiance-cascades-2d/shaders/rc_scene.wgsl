// Builds the "scene" texture radiance cascades reads everything else from:
// RGB = emissive radiance (light sources), A = occluder mask (1 = opaque,
// blocks/absorbs light; 0 = empty space light passes through). Mirrors the
// reference's `drawPassTexture` (straight-alpha scene canvas), except this
// one is synthesized from a caller-owned occupancy grid + emitter list
// instead of being painted by hand.

struct SceneUniforms {
    view_center: vec2<f32>,
    view_half_extent: vec2<f32>,
    occupancy_dims: vec2<u32>,
    occupancy_cell_size: f32,
    emitter_count: u32,
    occupancy_origin: vec2<f32>,
    scene_size: vec2<f32>,
}

struct Emitter {
    pos: vec2<f32>,
    radius: f32,
    r: f32,
    g: f32,
    b: f32,
    _pad: f32,
}

@group(0) @binding(0) var<uniform> u: SceneUniforms;
@group(0) @binding(1) var<storage, read> occupancy: array<u32>;
@group(0) @binding(2) var<storage, read> emitters: array<Emitter>;
@group(0) @binding(3) var scene_tex: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8)
fn cs_build_scene(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (f32(gid.x) >= u.scene_size.x || f32(gid.y) >= u.scene_size.y) {
        return;
    }

    let uv = (vec2<f32>(gid.xy) + 0.5) / u.scene_size;
    // Y-up world space, V-down texture space.
    let world = u.view_center + (uv * 2.0 - 1.0) * u.view_half_extent * vec2<f32>(1.0, -1.0);

    var occluded = 0.0;
    let cellf = floor((world - u.occupancy_origin) / u.occupancy_cell_size);
    if (cellf.x >= 0.0 && cellf.y >= 0.0) {
        let cell = vec2<u32>(cellf);
        if (cell.x < u.occupancy_dims.x && cell.y < u.occupancy_dims.y) {
            let idx = cell.y * u.occupancy_dims.x + cell.x;
            let word = occupancy[idx / 32u];
            occluded = f32((word >> (idx % 32u)) & 1u);
        }
    }

    var rgb = vec3<f32>(0.0);
    for (var i = 0u; i < u.emitter_count; i = i + 1u) {
        let e = emitters[i];
        let d = distance(world, e.pos);
        let falloff = clamp(1.0 - d / max(e.radius, 1.0), 0.0, 1.0);
        rgb += vec3<f32>(e.r, e.g, e.b) * falloff * falloff;
    }

    textureStore(scene_tex, vec2<i32>(gid.xy), vec4<f32>(rgb, occluded));
}
