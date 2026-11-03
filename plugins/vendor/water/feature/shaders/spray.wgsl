//!use helio_prelude

// Camera-facing breaker spray and impact droplets. Shore emitters are a
// deterministic packed set over the authored map; each vertex evaluates the
// same travel-time phase and breaking envelope as surface.wgsl. No per-frame
// CPU emitter simulation is required.

struct WaterVolume {
    bounds_min: vec4f, bounds_max: vec4f, wave_params: vec4f,
    wave_direction: vec4f, water_color: vec4f, extinction: vec4f,
    reflection_refraction: vec4f, caustics_params: vec4f, fog_params: vec4f,
    sim_params: vec4f, shadow_params: vec4f, sun_direction: vec4f,
    ssr_params: vec4f, sim_dynamics: vec4f, wind_params: vec4f,
    world_phase: vec4f,
}
struct ShoreParams { center_extent: vec4f, state: vec4f }
struct OceanPhase { patches: array<vec4f, 4> }
struct SprayParams { amount: f32, size: f32, max_distance: f32, emitter_count: u32 }
struct Impact { position_age: vec4f, radius_strength: vec4f }
struct SprayEmitter { position_seed: vec4f }
struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    @location(1) alpha: f32,
    @location(2) seed: f32,
    @location(3) view_z: f32,
}

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<storage, read> spray_emitters: array<SprayEmitter>;
@group(0) @binding(2) var shore_fields: texture_2d_array<f32>;
@group(0) @binding(3) var<uniform> shore_params: ShoreParams;
@group(0) @binding(4) var<storage, read> impacts: array<Impact>;
@group(0) @binding(5) var<storage, read> volumes: array<WaterVolume>;
@group(0) @binding(6) var ocean_displacement: texture_2d_array<f32>;
@group(0) @binding(7) var ocean_sampler: sampler;
@group(0) @binding(8) var<uniform> ocean_phase: OceanPhase;
@group(0) @binding(9) var<uniform> spray_params: SprayParams;
@group(0) @binding(10) var scene_depth: texture_depth_2d;
@group(0) @binding(11) var water_interaction: texture_2d_array<f32>;

const PI: f32 = 3.141592653589793;
const PATCHES: array<f32, 3> = array<f32, 3>(600.0, 85.0, 12.0);

fn hash21(p: vec2f) -> f32 {
    var h = fract(p * vec2f(0.1031, 0.1030));
    h += dot(h, h.yx + 33.33);
    return fract((h.x + h.y) * h.x);
}

fn value_noise(p: vec2f) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(hash21(i), hash21(i + vec2f(1.0, 0.0)), u.x),
               mix(hash21(i + vec2f(0.0, 1.0)), hash21(i + vec2f(1.0, 1.0)), u.x), u.y);
}

fn shore_sample(world_xz: vec2f, layer: i32) -> vec4f {
    let extent = max(shore_params.center_extent.zw, vec2f(1e-3));
    let uv = (world_xz - shore_params.center_extent.xy) / extent + vec2f(0.5);
    if any(uv < vec2f(0.0)) || any(uv > vec2f(1.0)) {
        return select(vec4f(0.0, 1e6, 0.0, 0.0), vec4f(0.0, 0.0, 0.0, 1.0), layer == 1);
    }
    let size = vec2f(textureDimensions(shore_fields, 0));
    let p = uv * size - vec2f(0.5);
    let base = vec2i(floor(p));
    let f = fract(p);
    let hi = vec2i(size) - vec2i(1);
    let p00 = clamp(base, vec2i(0), hi);
    let p10 = clamp(base + vec2i(1, 0), vec2i(0), hi);
    let p01 = clamp(base + vec2i(0, 1), vec2i(0), hi);
    let p11 = clamp(base + vec2i(1), vec2i(0), hi);
    return mix(mix(textureLoad(shore_fields, p00, layer, 0), textureLoad(shore_fields, p10, layer, 0), f.x),
               mix(textureLoad(shore_fields, p01, layer, 0), textureLoad(shore_fields, p11, layer, 0), f.x), f.y);
}

fn emitter_vertex(vertex: u32, instance: u32) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4f(2.0, 2.0, 2.0, 1.0);
    out.uv = vec2f(0.0);
    out.alpha = 0.0;
    out.seed = 0.0;
    out.view_z = 0.0;
    if shore_params.state.x < 0.5 || arrayLength(&volumes) == 0u || spray_params.amount <= 0.0
        || instance >= spray_params.emitter_count || instance >= arrayLength(&spray_emitters) { return out; }
    let emitter = spray_emitters[instance].position_seed;
    let seed = f32(instance) + 11.0;
    let world_xz = shore_params.center_extent.xy + emitter.xy;
    let to_eye = world_xz - cameras[0].position_near.xz;
    let dist = length(to_eye);
    if dist > spray_params.max_distance { return out; }
    let shore = shore_sample(world_xz, 0);
    let travel = shore_sample(world_xz, 1);
    let depth = max(-shore.x, 0.0);
    if depth < 0.3 || depth > 3.2 || shore.y > 150.0 { return out; }
    let env = (1.0 - smoothstep(70.0, 180.0, shore.y))
            * smoothstep(0.35, 0.8, length(shore.zw))
            * smoothstep(0.0, 0.5, depth);
    let has_front = dot(travel.yz, travel.yz) > 0.25;
    if env < 1e-4 || !has_front { return out; }
    let dir = normalize(travel.yz);
    let shoal = max(travel.w, 0.05);
    let celerity = sqrt(9.81 * clamp(depth, 0.3, 14.0));
    let phase_noise = value_noise(world_xz * 0.0042) * 9.0 + value_noise(world_xz * 0.011 + vec2f(7.0)) * 2.5;
    let group = 0.45 + 0.55 * value_noise(world_xz * 0.004
        + vec2f(shore_params.state.y * 0.021, -shore_params.state.y * 0.017));
    let swell_dir = vec2f(-0.9396926, -0.3420201);
    let exposure = mix(0.25, 1.0, smoothstep(-0.3, 0.7, dot(dir, swell_dir)));

    var height = volumes[0].bounds_max.w;
    let cell_size = max(max(shore_params.center_extent.z, shore_params.center_extent.w) / 512.0, 1e-4);
    for (var band = 0u; band < 3u; band++) {
        let damp = select(1.0, smoothstep(2.0, 26.0, depth), band == 0u)
                 * select(1.0, smoothstep(-0.4, 2.0, depth) * mix(0.55, 1.0, smoothstep(0.0, 10.0, depth)), band > 0u);
        let spectrum_uv = fract((world_xz + ocean_phase.patches[band].xy) / PATCHES[band]);
        let lod = max(log2(max(cell_size, 1e-4) * 256.0 / PATCHES[band]) + 1.0, 0.0);
        height += textureSampleLevel(ocean_displacement, ocean_sampler, spectrum_uv, i32(band), lod).y * damp;
    }
    // Localized impact waves remain part of the visible interface inside the
    // first volume's bounds; match the surface's normalized-height scaling.
    let vol = volumes[0];
    let interaction_extent = max(vol.bounds_max.xz - vol.bounds_min.xz, vec2f(1e-4));
    let interaction_uv = (world_xz - vol.bounds_min.xz) / interaction_extent;
    if all(interaction_uv >= vec2f(0.0)) && all(interaction_uv <= vec2f(1.0)) {
        let headroom = min(vol.bounds_max.w - vol.bounds_min.y, vol.bounds_max.y - vol.bounds_max.w);
        let wave_amplitude = clamp(vol.wave_params.x, 0.0, max(headroom, 0.0));
        height += textureSampleLevel(water_interaction, ocean_sampler, interaction_uv, 0, 0.0).r * wave_amplitude;
    }
    var strength = 0.0;
    var main_amplitude = 0.0;
    var main_omega = 0.0;
    var main_k = 0.0;
    var main_phase = 0.0;
    var shore_height = 0.0;
    var shore_horizontal = vec2f(0.0);
    var compression = 0.82;
    for (var train = 0u; train < 2u; train++) {
        let lambda = 30.0 * select(1.0, 0.61, train == 1u);
        let omega = sqrt(9.81 * 2.0 * PI / lambda);
        let k = omega / celerity;
        let amp_scale = select(1.0, 0.45, train == 1u);
        let cap = (0.42 * depth + 0.14) * select(1.0, 0.5, train == 1u);
        let amplitude = min(0.55 * amp_scale * env * exposure * shoal * group, cap);
        let phase = omega * (travel.x + shore_params.state.y) + phase_noise * select(1.0, 1.37, train == 1u) + f32(train) * 2.1;
        let c = 0.5 + 0.5 * cos(phase);
        let s = sin(phase);
        let cp = pow(max(c, 1e-4), 0.6);
        let relative_height = amplitude / max(depth, 0.05);
        let cycle = floor((phase + PI) / (2.0 * PI));
        let early = value_noise(world_xz * 0.03 + vec2f(cycle * 7.3, cycle * 3.1 + f32(train) * 31.0));
        let breaking = smoothstep(0.12 + 0.14 * early, 0.3 + 0.2 * early, relative_height);
        strength = max(strength, breaking * smoothstep(0.08, 0.35, amplitude));
        var pitch = 0.0;
        if train == 0u {
            pitch = 2.0 * amplitude * smoothstep(0.16, 0.28, relative_height)
                * (1.0 - smoothstep(0.4, 0.6, relative_height));
        }
        pitch = min(pitch, 0.35 / (1.08 * max(k, 1e-4)));
        compression -= 1.08 * pitch * k;
        let qak = min(compression, amplitude * k * 8.0);
        compression -= qak;
        shore_height += amplitude * (2.0 * cp * c - 0.78);
        let c5 = c * c * c * c * c;
        let horizontal = (qak / max(k, 1e-4)) * s + pitch * c5 * c;
        shore_horizontal += dir * horizontal;
        if train == 0u {
            main_amplitude = amplitude;
            main_omega = omega;
            main_k = k;
            main_phase = phase;
        }
    }
    height += shore_height;
    if strength <= 0.01 { return out; }
    let random_a = emitter.z;
    let random_b = emitter.w;
    let psi = atan2(sin(main_phase + PI), cos(main_phase + PI));
    let age = psi / max(main_omega, 1e-3) - random_a * 0.3;
    let life = 0.9 + 1.5 * random_b;
    if age < 0.0 || age > life { return out; }
    let drag = 1.7;
    let gravity = 9.81;
    let ex = (1.0 - exp(-drag * age)) / drag;
    let celerity_wave = main_omega / max(main_k, 1e-4);
    let side = vec2f(-dir.y, dir.x) * (fract(random_a * 31.7) - 0.5) * 1.6;
    let xz = world_xz + side + dir * celerity_wave * (0.25 + 0.45 * fract(random_b * 17.3)) * ex;
    let vy = (1.2 + 3.2 * fract(random_a * 7.9)) * sqrt(max(main_amplitude, 0.0));
    let y = height + 1.1 * main_amplitude + (vy + gravity / drag) * ex - gravity * age / drag;
    if y < height - 0.3 { return out; }

    let quad = array<vec2f, 6>(vec2f(-1.0,-1.0), vec2f(1.0,-1.0), vec2f(1.0,1.0), vec2f(-1.0,-1.0), vec2f(1.0,1.0), vec2f(-1.0,1.0));
    let q = quad[vertex % 6u];
    let size = (0.1 + 0.6 * age) * (0.6 + 0.8 * fract(random_b * 5.3)) * spray_params.size * sqrt(max(main_amplitude / 0.5, 0.0));
    let world_pos = vec3f(xz.x + shore_horizontal.x, y, xz.y + shore_horizontal.y);
    let view_pos = cameras[0].view * vec4f(world_pos, 1.0);
    let billboard = view_pos + vec4f(q * size, 0.0, 0.0);
    out.position = cameras[0].proj * billboard;
    out.uv = q;
    out.seed = random_a;
    out.view_z = -view_pos.z;
    out.alpha = strength * spray_params.amount * smoothstep(0.0, 0.12, age) * pow(1.0 - age / life, 1.5)
        * smoothstep(-0.3, 0.2, y - height)
        * (1.0 - smoothstep(spray_params.max_distance * 0.7, spray_params.max_distance, dist))
        * smoothstep(2.5, 10.0, out.view_z);
    return out;
}

@vertex
fn vs_breaker(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> VertexOutput {
    return emitter_vertex(vertex, instance);
}

@vertex
fn vs_impact(@builtin(vertex_index) vertex: u32, @builtin(instance_index) instance: u32) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4f(2.0, 2.0, 2.0, 1.0);
    out.uv = vec2f(0.0); out.alpha = 0.0; out.seed = 0.0; out.view_z = 0.0;
    if spray_params.amount <= 0.0 || arrayLength(&volumes) == 0u { return out; }
    let impact_index = instance / 64u;
    let particle = instance % 64u;
    if impact_index >= arrayLength(&impacts) { return out; }
    let e = impacts[impact_index];
    let age = e.position_age.w;
    let energy = e.radius_strength.y;
    if energy <= 0.0 || age < 0.0 || age > 1.6 { return out; }
    let seed = f32(particle + impact_index * 67u);
    let r1 = hash21(vec2f(seed, 1.7));
    let r2 = hash21(vec2f(seed, 8.3));
    let r3 = hash21(vec2f(seed, 4.1));
    let life = 0.5 + 1.1 * r2;
    if age > life { return out; }
    let drag = 1.2; let gravity = 9.81;
    let ex = (1.0 - exp(-drag * age)) / drag;
    let angle = r1 * 2.0 * PI;
    let outward = vec2f(cos(angle), sin(angle));
    let strength = sqrt(max(energy / 0.2, 0.0));
    let vy = (1.5 + 4.0 * r3 * r3) * strength;
    let xz = e.position_age.xz + outward * (e.radius_strength.x * (0.6 + 0.5 * r2) + (0.5 + 2.0 * r3) * strength * ex);
    let y = e.position_age.y + (vy + gravity / drag) * ex - gravity * age / drag;
    if y < e.position_age.y - 0.2 { return out; }
    let q = array<vec2f, 6>(vec2f(-1.0,-1.0), vec2f(1.0,-1.0), vec2f(1.0,1.0), vec2f(-1.0,-1.0), vec2f(1.0,1.0), vec2f(-1.0,1.0))[vertex % 6u];
    let size = (0.05 + 0.25 * age) * (0.5 + r2) * spray_params.size;
    let view_pos = cameras[0].view * vec4f(xz.x, y, xz.y, 1.0);
    out.position = cameras[0].proj * (view_pos + vec4f(q * size, 0.0, 0.0));
    out.uv = q; out.seed = r1; out.view_z = -view_pos.z;
    out.alpha = spray_params.amount * 0.6 * smoothstep(0.0, 0.05, age) * pow(1.0 - age / life, 1.5)
        * smoothstep(-0.2, 0.1, y - e.position_age.y) * smoothstep(1.0, 4.0, out.view_z);
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let r = length(in.uv);
    if in.alpha <= 0.0 || r > 1.0 { discard; }
    let noise = value_noise(in.uv * 0.8 + vec2f(in.seed * 17.0));
    var alpha = in.alpha * exp(-3.5 * r * r) * (0.5 + noise);
    let pixel = clamp(vec2i(in.position.xy), vec2i(0), vec2i(textureDimensions(scene_depth)) - vec2i(1));
    let depth = textureLoad(scene_depth, pixel, 0);
    if !helio_is_sky(depth) {
        // Fragment position is already in framebuffer UV space after division;
        // the prelude helper then performs the UV→NDC conversion itself.
        let uv = in.position.xy / vec2f(textureDimensions(scene_depth));
        let scene_pos = helio_world_from_depth(cameras[0].view_proj_inv, uv, depth);
        // Compare view-axis depth to view-axis depth. Radial distance would
        // produce an angle-dependent soft intersection on perspective views.
        let scene_z = -((cameras[0].view * vec4f(scene_pos, 1.0)).z);
        alpha *= smoothstep(0.0, 0.8, scene_z - in.view_z);
    }
    return vec4f(vec3f(0.9, 0.96, 1.0) * alpha, alpha);
}
