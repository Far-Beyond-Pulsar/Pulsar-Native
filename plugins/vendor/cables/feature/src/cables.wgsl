enable wgpu_binding_array;

struct CableUniforms {
    connector_count: u32,
    definition_count: u32,
    frame: u32,
    _pad: u32,
};
struct Camera {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> u: CableUniforms;
@group(0) @binding(1) var<storage, read> connector_words: array<u32>;
@group(0) @binding(2) var<storage, read> definition_words: array<u32>;
@group(0) @binding(3) var<uniform> camera: Camera;
@group(0) @binding(4) var<storage, read> instances: array<u32>;
@group(1) @binding(0) var scene_sampler: sampler;
@group(1) @binding(1) var scene_textures: binding_array<texture_2d<f32>, 256>;

// @common

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) texture_index: u32,
};

fn sample_cable_texture(texture_index: u32, uv: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(scene_textures[texture_index], scene_sampler, uv, 0.0);
}

fn dead_vertex() -> VertexOut {
    var out: VertexOut;
    out.clip = vec4<f32>(2.0, 2.0, 2.0, 1.0);
    out.color = vec4<f32>(0.0);
    out.uv = vec2<f32>(0.0);
    out.texture_index = 0xffffffffu;
    return out;
}

// `list` 0 is the camera-facing ribbon list, 1 the tube list; `face` is the
// tube side (always 0 for a ribbon).
fn emit(segment: u32, face: u32, corner: u32, instance: u32, list: u32) -> VertexOut {
    if (instance >= INSTANCE_CAPACITY) { return dead_vertex(); }
    let record = (list * INSTANCE_CAPACITY + instance) * 4u;
    let ao = instances[record] * CONNECTOR_WORDS;
    let bo = instances[record + 1u] * CONNECTOR_WORDS;
    let definition = instances[record + 2u];
    let lane = instances[record + 3u];
    let tube = list != 0u;
    let bundle_count = select(1u, clamp(definition_words[definition + 3u], 1u, BUNDLE_LANES), tube);

    let p0 = vec3<f32>(fword(connector_words[ao + 12u]), fword(connector_words[ao + 13u]), fword(connector_words[ao + 14u]));
    let p1 = vec3<f32>(fword(connector_words[bo + 12u]), fword(connector_words[bo + 13u]), fword(connector_words[bo + 14u]));
    let t0 = f32(segment) / f32(SEGMENTS);
    let next = min(t0 + 1.0 / f32(SEGMENTS), 1.0);
    let t = select(t0, next, corner == 1u || corner == 2u || corner == 4u);
    let side = select(-1.0, 1.0, corner == 2u || corner == 4u || corner == 5u);
    let delta = p1 - p0;
    let span = length(delta);
    let tangent = normalize(delta + vec3<f32>(0.00001, 0.0, 0.0));
    let view_right = normalize(vec3<f32>(camera.view[0][0], camera.view[1][0], camera.view[2][0]));
    // Camera-facing ribbon: the width runs perpendicular to both the cable and
    // the view direction. Edge-on (cable along the view axis) the ribbon has no
    // visible width, so fall back to the camera's right axis, never a NaN.
    let view_back = vec3<f32>(camera.view[0][2], camera.view[1][2], camera.view[2][2]);
    let facing = cross(tangent, view_back);
    var width_axis = view_right;
    if (dot(facing, facing) > 1e-8) { width_axis = normalize(facing); }
    let spacing = fword(definition_words[definition + 5u]);
    let lanes_per_axis = u32(ceil(sqrt(f32(bundle_count))));
    let bundle_x = lane % lanes_per_axis;
    let bundle_y = lane / lanes_per_axis;
    let offset_x = (f32(bundle_x) - (f32(lanes_per_axis) - 1.0) * 0.5) * spacing;
    let offset_y = (f32(bundle_y) - (f32(lanes_per_axis) - 1.0) * 0.5) * spacing;
    let droop = clamp(fword(definition_words[definition + 6u]), 0.0, 1.0) * span;
    let strength = max(fword(definition_words[definition + 7u]), 0.0);
    let wind = vec3<f32>(fword(definition_words[definition + 8u]), fword(definition_words[definition + 9u]), fword(definition_words[definition + 10u]));
    let speed = max(fword(definition_words[definition + 11u]), 0.0);
    let revision = f32(definition_words[definition + 12u]);
    let phase = f32(u.frame) * 0.016 * speed + t * 7.0 + revision * 0.37;
    let curve = mix(p0, p1, t)
        - vec3<f32>(0.0, 1.0, 0.0) * (droop * 4.0 * t * (1.0 - t))
        + wind * (sin(phase) * strength * t * (1.0 - t));
    let radius = max(fword(definition_words[definition + 4u]), 0.001);
    let next_face = (face + 1u) % TUBE_SIDES;
    let angle0 = TAU * f32(face) / f32(TUBE_SIDES);
    let angle1 = TAU * f32(next_face) / f32(TUBE_SIDES);
    let angle = select(angle0, angle1, side > 0.0);
    let reference = select(vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(1.0, 0.0, 0.0), abs(tangent.y) > 0.9);
    let tube_normal = normalize(cross(tangent, reference));
    let tube_binormal = normalize(cross(tangent, tube_normal));
    let circular = tube_normal * cos(angle) + tube_binormal * sin(angle);
    let bundle_offset = tube_normal * offset_x + tube_binormal * offset_y;
    let final_width_axis = select(width_axis, circular, tube);
    let width = select(side * radius, radius, tube);
    let pos = curve + bundle_offset + final_width_axis * width;
    var out: VertexOut;
    out.clip = camera.view_proj * vec4<f32>(pos, 1.0);
    out.color = unpack4x8unorm(definition_words[definition + 14u]);
    out.uv = vec2<f32>(t * span, side * 0.5 + 0.5);
    out.texture_index = definition_words[definition + 13u];
    return out;
}

@vertex
fn vs_ribbon(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> VertexOut {
    return emit(vertex / 6u, 0u, vertex % 6u, instance, 0u);
}

@vertex
fn vs_tube(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> VertexOut {
    return emit(vertex / (TUBE_SIDES * 6u), (vertex / 6u) % TUBE_SIDES, vertex % 6u, instance, 1u);
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    var texel = vec4<f32>(1.0);
    if (in.texture_index != 0xffffffffu) {
        texel = sample_cable_texture(in.texture_index, in.uv);
    }
    return texel * in.color;
}
