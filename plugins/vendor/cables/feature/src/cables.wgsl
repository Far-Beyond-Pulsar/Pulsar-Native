enable wgpu_binding_array;

struct Camera {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
};
@group(0) @binding(0) var<uniform> u: CableUniforms;
@group(0) @binding(1) var<storage, read> definition_words: array<u32>;
@group(0) @binding(2) var<uniform> camera: Camera;
@group(0) @binding(3) var<storage, read> ribbon_words: array<u32>;
@group(0) @binding(4) var<storage, read> tube_words: array<u32>;
@group(1) @binding(0) var scene_sampler: sampler;
@group(1) @binding(1) var scene_textures: binding_array<texture_2d<f32>, 256>;

// @common

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) @interpolate(flat) texture_index: u32,
};

// One cable strand: the two ends in the frame's space, which strand of a
// bundle it is, and the cable ID that names its definition row.
struct Instance {
    start: vec3<f32>,
    lane: u32,
    end: vec3<f32>,
    id: u32,
};

fn ribbon_instance(i: u32) -> Instance {
    let o = i * INSTANCE_WORDS;
    var s: Instance;
    s.start = vec3<f32>(fword(ribbon_words[o]), fword(ribbon_words[o + 1u]), fword(ribbon_words[o + 2u]));
    s.lane = ribbon_words[o + 3u];
    s.end = vec3<f32>(fword(ribbon_words[o + 4u]), fword(ribbon_words[o + 5u]), fword(ribbon_words[o + 6u]));
    s.id = ribbon_words[o + 7u];
    return s;
}

fn tube_instance(i: u32) -> Instance {
    let o = i * INSTANCE_WORDS;
    var s: Instance;
    s.start = vec3<f32>(fword(tube_words[o]), fword(tube_words[o + 1u]), fword(tube_words[o + 2u]));
    s.lane = tube_words[o + 3u];
    s.end = vec3<f32>(fword(tube_words[o + 4u]), fword(tube_words[o + 5u]), fword(tube_words[o + 6u]));
    s.id = tube_words[o + 7u];
    return s;
}

// The packed definition row of `id` (the join keeps enabled definitions in the
// leading rows), or 0xffffffff.
fn find_definition(id: u32) -> u32 {
    for (var k = 0u; k < u.definition_count; k++) {
        let d = k * DEFINITION_WORDS;
        if (definition_words[d] == id && definition_words[d + 1u] != 0u) {
            return d;
        }
    }
    return 0xffffffffu;
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

// `tube` selects the round cross-section; `face` is the tube side (0 for a
// ribbon).
fn emit(segment: u32, face: u32, corner: u32, strand: Instance, tube: bool) -> VertexOut {
    let definition = find_definition(strand.id);
    if (definition == 0xffffffffu) { return dead_vertex(); }
    let bundle_count = select(1u, clamp(definition_words[definition + 3u], 1u, BUNDLE_LANES), tube);
    let lane = strand.lane;
    if (lane >= bundle_count) { return dead_vertex(); }

    let p0 = strand.start;
    let p1 = strand.end;
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
    return emit(vertex / 6u, 0u, vertex % 6u, ribbon_instance(instance), false);
}

@vertex
fn vs_tube(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> VertexOut {
    return emit(vertex / (TUBE_SIDES * 6u), (vertex / 6u) % TUBE_SIDES, vertex % 6u, tube_instance(instance), true);
}

// ── Debug views ─────────────────────────────────────────────────────────────
// "Cable Solid" (flag FLAG_SOLID) paints the generated geometry flat magenta,
// separating geometry problems from surface/texture problems. "Cable
// Diagnostics" draws a HUD of what each stage found, a cross at every cable
// end and straight lines along the first cables.

struct Projected {
    px: vec2<f32>,
    visible: bool,
};

fn project(world: vec3<f32>) -> Projected {
    let clip = camera.view_proj * vec4<f32>(world, 1.0);
    var out: Projected;
    out.visible = clip.w > 0.0001;
    let ndc = clip.xy / max(clip.w, 0.0001);
    out.px = vec2<f32>((ndc.x * 0.5 + 0.5) * u.viewport.x, (0.5 - ndc.y * 0.5) * u.viewport.y);
    return out;
}

fn segment_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let len2 = max(dot(ab, ab), 1e-4);
    let t = clamp(dot(p - a, ab) / len2, 0.0, 1.0);
    return length(p - (a + ab * t));
}

@vertex
fn vs_debug(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

fn bar(px: vec2<f32>, row: u32, value: u32, color: vec3<f32>) -> vec4<f32> {
    let y0 = 16.0 + f32(row) * 24.0;
    if (px.y < y0 || px.y >= y0 + 18.0) { return vec4<f32>(0.0); }
    let x = px.x - 16.0;
    if (x < 0.0) { return vec4<f32>(0.0); }
    if (x < f32(min(value, 128u)) * 4.0) { return vec4<f32>(color, 1.0); }
    // A faint tick every 8 units so counts can be read off the bar.
    if (u32(x) % 32u == 0u) { return vec4<f32>(1.0, 1.0, 1.0, 0.35); }
    return vec4<f32>(0.0);
}

fn cross_hit(px: vec2<f32>, at: Projected) -> bool {
    if (!at.visible) { return false; }
    let d = abs(px - at.px);
    return (d.x < 12.0 && d.y < 2.0) || (d.y < 12.0 && d.x < 2.0);
}

@fragment
fn fs_debug(@builtin(position) frag: vec4<f32>) -> @location(0) vec4<f32> {
    let px = frag.xy;
    var color = vec4<f32>(0.0);

    // HUD: one bar per stage; length is the count (4 px per unit, 128 max).
    if (px.x < 16.0 + 128.0 * 4.0 + 8.0 && px.y < 16.0 + 13.0 * 24.0) {
        color = vec4<f32>(0.0, 0.0, 0.0, 0.45);
        var enabled = 0u;
        for (var d = 0u; d < u.definition_count; d++) {
            if (definition_words[d * DEFINITION_WORDS + 1u] != 0u) { enabled++; }
        }
        // Raw definition buffer: how many rows it holds, how many carry any
        // data (scanning up to 1024 rows), and the first such row's words.
        let total_rows = arrayLength(&definition_words) / DEFINITION_WORDS;
        var nonzero_rows = 0u;
        var first_row = 0xffffffffu;
        for (var d = 0u; d < min(total_rows, 1024u); d++) {
            var any_data = false;
            for (var w = 0u; w < DEFINITION_WORDS; w++) {
                if (definition_words[d * DEFINITION_WORDS + w] != 0u) { any_data = true; }
            }
            if (any_data) {
                nonzero_rows++;
                if (first_row == 0xffffffffu) { first_row = d; }
            }
        }
        let first_base = select(0u, first_row * DEFINITION_WORDS, first_row != 0xffffffffu);
        let rows_live = u.flags & FLAG_ROWS_LIVE;
        let textures_ok = (u.flags & FLAG_TEXTURES_OK) >> 1u;
        var layers: array<vec4<f32>, 13>;
        layers[0] = bar(px, 0u, rows_live * 128u, vec3<f32>(0.2, 0.9, 0.2));         // definition sources hold rows
        layers[1] = bar(px, 1u, textures_ok * 128u, vec3<f32>(0.9, 0.9, 0.9));       // material texture table present
        layers[2] = bar(px, 2u, u.ribbon_count, vec3<f32>(1.0, 0.6, 0.1));           // ribbon strands published
        layers[3] = bar(px, 3u, u.tube_count, vec3<f32>(1.0, 0.2, 0.9));             // tube strands published
        layers[4] = bar(px, 4u, enabled, vec3<f32>(0.4, 0.9, 1.0));                  // enabled definitions packed
        layers[5] = bar(px, 5u, u.definition_count, vec3<f32>(0.5, 0.5, 0.9));       // definition rows considered
        layers[6] = bar(px, 6u, total_rows, vec3<f32>(0.8, 0.8, 0.8));               // rows in the definition buffer
        layers[7] = bar(px, 7u, nonzero_rows, vec3<f32>(1.0, 0.5, 0.5));             // ... holding any data
        layers[8] = bar(px, 8u, select(0u, first_row, first_row != 0xffffffffu), vec3<f32>(0.6, 1.0, 0.6)); // first such row's index
        layers[9] = bar(px, 9u, definition_words[first_base], vec3<f32>(1.0, 1.0, 1.0));       // word 0: id (expect 1)
        layers[10] = bar(px, 10u, definition_words[first_base + 1u], vec3<f32>(1.0, 1.0, 1.0)); // word 1: enabled (expect 1)
        layers[11] = bar(px, 11u, definition_words[first_base + 2u], vec3<f32>(1.0, 1.0, 1.0)); // word 2: mode (0 = 2D, 1 = 3D)
        layers[12] = bar(px, 12u, definition_words[first_base + 3u], vec3<f32>(1.0, 1.0, 1.0)); // word 3: bundle count (expect 1)
        for (var k = 0u; k < 13u; k++) {
            if (layers[k].a > 0.0) { color = vec4<f32>(mix(color.rgb, layers[k].rgb, layers[k].a), max(color.a, 0.9)); }
        }
    }

    // The first strands of each list: a cross at both ends and a straight
    // screen-space line (ribbon yellow, tube magenta), whatever the geometry does.
    for (var i = 0u; i < min(u.ribbon_count, 16u); i++) {
        let s = ribbon_instance(i);
        let a = project(s.start);
        let b = project(s.end);
        if (cross_hit(px, a) || cross_hit(px, b)) { color = vec4<f32>(1.0, 0.6, 0.1, 1.0); }
        if (a.visible && b.visible && segment_distance(px, a.px, b.px) < 1.5) {
            color = vec4<f32>(1.0, 1.0, 0.0, 1.0);
        }
    }
    for (var i = 0u; i < min(u.tube_count, 16u); i++) {
        let s = tube_instance(i);
        let a = project(s.start);
        let b = project(s.end);
        if (cross_hit(px, a) || cross_hit(px, b)) { color = vec4<f32>(1.0, 0.2, 0.9, 1.0); }
        if (a.visible && b.visible && segment_distance(px, a.px, b.px) < 1.5) {
            color = vec4<f32>(1.0, 0.0, 1.0, 1.0);
        }
    }
    return color;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    if ((u.flags & FLAG_SOLID) != 0u) { return vec4<f32>(1.0, 0.0, 1.0, 1.0); }
    var texel = vec4<f32>(1.0);
    if (in.texture_index != 0xffffffffu) {
        texel = sample_cable_texture(in.texture_index, in.uv);
    }
    return texel * in.color;
}
