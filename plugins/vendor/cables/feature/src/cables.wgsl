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
@group(1) @binding(0) var scene_sampler: sampler;
@group(1) @binding(1) var scene_textures: binding_array<texture_2d<f32>, 256>;

const CONNECTOR_WORDS: u32 = 20u;
const DEFINITION_WORDS: u32 = 16u;
const BUNDLE_LANES: u32 = 64u;
const SEGMENTS: u32 = 32u;
const TUBE_SIDES: u32 = 12u;
const TAU: f32 = 6.283185307179586;

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) texture_index: u32,
};

fn fword(word: u32) -> f32 {
    return bitcast<f32>(word);
}

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

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> VertexOut {
    if (u.connector_count < 2u || u.definition_count == 0u) { return dead_vertex(); }
    let candidate = instance / BUNDLE_LANES;
    let lane = instance % BUNDLE_LANES;
    let a = candidate / u.connector_count;
    let b = candidate % u.connector_count;
    if (a >= u.connector_count || b <= a) { return dead_vertex(); }
    let ao = a * CONNECTOR_WORDS;
    let bo = b * CONNECTOR_WORDS;
    if (connector_words[ao + 17u] == 0u || connector_words[bo + 17u] == 0u) { return dead_vertex(); }
    let id = connector_words[ao + 16u];
    if (id == 0u || connector_words[bo + 16u] != id) { return dead_vertex(); }
    var definition = 0xffffffffu;
    for (var k = 0u; k < u.definition_count; k++) {
        let d = k * DEFINITION_WORDS;
        if (definition_words[d] == id && definition_words[d + 1u] != 0u) {
            definition = d;
            break;
        }
    }
    if (definition == 0xffffffffu) { return dead_vertex(); }
    let bundle_count = clamp(definition_words[definition + 3u], 1u, BUNDLE_LANES);
    let mode = definition_words[definition + 2u];
    if (lane >= bundle_count || (mode == 0u && lane != 0u)) { return dead_vertex(); }

    let p0 = vec3<f32>(fword(connector_words[ao + 12u]), fword(connector_words[ao + 13u]), fword(connector_words[ao + 14u]));
    let p1 = vec3<f32>(fword(connector_words[bo + 12u]), fword(connector_words[bo + 13u]), fword(connector_words[bo + 14u]));
    let segment = vertex / (TUBE_SIDES * 6u);
    let face = (vertex / 6u) % TUBE_SIDES;
    if (mode == 0u && face != 0u) { return dead_vertex(); }
    let t0 = f32(segment) / f32(SEGMENTS);
    let next = min(t0 + 1.0 / f32(SEGMENTS), 1.0);
    let corner = vertex % 6u;
    let t = select(t0, next, corner == 1u || corner == 2u || corner == 4u);
    let side = select(-1.0, 1.0, corner == 2u || corner == 4u || corner == 5u);
    let delta = p1 - p0;
    let span = length(delta);
    let tangent = normalize(delta + vec3<f32>(0.00001, 0.0, 0.0));
    let view_right = normalize(vec3<f32>(camera.view[0][0], camera.view[1][0], camera.view[2][0]));
    var width_axis = normalize(cross(tangent, normalize(cross(view_right, tangent))));
    if (length(width_axis) < 0.1) { width_axis = view_right; }
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
    let final_width_axis = select(width_axis, circular, mode != 0u);
    let width = select(side * radius, radius, mode != 0u);
    let pos = curve + bundle_offset + final_width_axis * width;
    var out: VertexOut;
    out.clip = camera.view_proj * vec4<f32>(pos, 1.0);
    out.color = unpack4x8unorm(definition_words[definition + 14u]);
    out.uv = vec2<f32>(t * span, side * 0.5 + 0.5);
    out.texture_index = definition_words[definition + 13u];
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    var texel = vec4<f32>(1.0);
    if (in.texture_index != 0xffffffffu) {
        texel = sample_cable_texture(in.texture_index, in.uv);
    }
    return texel * in.color;
}
