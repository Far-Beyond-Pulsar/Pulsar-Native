struct ShoreParams { center_extent: vec4f, state: vec4f }
@group(0) @binding(15) var shore_fields: texture_2d_array<f32>;
@group(0) @binding(16) var<uniform> shore_params: ShoreParams;

struct ShoreWave {
    displacement: vec3f,
    slope: vec2f,
    foam: f32,
    direction: vec2f,
    first_train: vec4f, // amplitude, k, omega, phase
    residual: f32,
}

const WATER_GEOMETRIC_PATCH_SIZES: array<f32, 3> = array(600.0, 85.0, 12.0);

fn hash21(p: vec2f) -> f32 {
    var h = fract(p * vec2f(0.1031, 0.1030));
    h += dot(h, h.yx + 33.33);
    return fract((h.x + h.y) * h.x);
}

fn value_noise(p: vec2f) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    return mix(
        mix(hash21(i), hash21(i + vec2f(1.0, 0.0)), u.x),
        mix(hash21(i + vec2f(0.0, 1.0)), hash21(i + vec2f(1.0, 1.0)), u.x),
        u.y,
    );
}

fn water_wave_amplitude(vol: WaterVolume) -> f32 {
    let rest = vol.bounds_max.w;
    let headroom = min(rest - vol.bounds_min.y, vol.bounds_max.y - rest);
    return clamp(vol.wave_params.x, 0.0, max(headroom, 0.0));
}

fn shore_field(world_xz: vec2f, layer: i32) -> vec4f {
    if shore_params.state.x < 0.5 {
        if layer == 1 { return vec4f(0.0); }
        return vec4f(-60.0, 1e6, 0.0, 0.0);
    }
    let extent = max(shore_params.center_extent.zw, vec2f(1e-3));
    let uv = (world_xz - shore_params.center_extent.xy) / extent + vec2f(0.5);
    if any(uv < vec2f(0.0)) || any(uv > vec2f(1.0)) {
        if layer == 1 { return vec4f(0.0); }
        // Match OceanSurface.shoreSample outside the baked map: deep offshore
        // water keeps the full swell spectrum and has no nearby wave front.
        return vec4f(-60.0, 1e6, 0.0, 0.0);
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
    return mix(
        mix(textureLoad(shore_fields, p00, layer, 0), textureLoad(shore_fields, p10, layer, 0), f.x),
        mix(textureLoad(shore_fields, p01, layer, 0), textureLoad(shore_fields, p11, layer, 0), f.x),
        f.y,
    );
}

fn shore_ocean_damp(world_xz: vec2f) -> vec4f {
    if shore_params.state.x < 0.5 { return vec4f(1.0); }
    let depth = max(-shore_field(world_xz, 0).x, 0.0);
    let swell = smoothstep(2.0, 26.0, depth);
    let chop = smoothstep(-0.4, 2.0, depth) * mix(0.55, 1.0, smoothstep(0.0, 10.0, depth));
    return vec4f(swell, chop, chop, chop);
}

/// Authored shallow-water trains on the shore map's travel-time and depth fields.
fn shore_wave(world_xz: vec2f) -> ShoreWave {
    var result = ShoreWave(vec3f(0.0), vec2f(0.0), 0.0, vec2f(0.0), vec4f(0.0), 0.0);
    if shore_params.state.x < 0.5 { return result; }
    let field = shore_field(world_xz, 0);
    let travel = shore_field(world_xz, 1);
    let depth = -field.x;
    let env = (1.0 - smoothstep(70.0, 180.0, field.y))
            * smoothstep(0.35, 0.8, length(field.zw))
            * smoothstep(-0.9, 0.5, depth);
    if env <= 1e-4 { return result; }
    let has_front = dot(travel.yz, travel.yz) > 0.25;
    let fallback_dir = -field.zw / max(length(field.zw), 1e-3);
    let dir = select(fallback_dir, normalize(travel.yz), has_front);
    let shoal = select(pow(clamp(max(depth, 0.6) / 14.0, 0.04, 1.0), -0.25), max(travel.w, 0.05), has_front);
    let celerity = sqrt(9.81 * clamp(depth, 0.3, 14.0));
    let phase_noise = value_noise(world_xz * 0.0042) * 9.0
                    + value_noise(world_xz * 0.011 + vec2f(7.0)) * 2.5;
    let group = 0.45 + 0.55 * value_noise(world_xz * 0.004
        + vec2f(shore_params.state.y * 0.021, -shore_params.state.y * 0.017));
    // defaultOceanParams().swell.direction is -15 degrees. This differs from
    // the wind vector used to orient the FFT spectrum.
    let swell_dir = vec2f(-0.9659258, 0.2588190);
    let exposure = mix(0.25, 1.0, smoothstep(-0.3, 0.7, dot(dir, swell_dir)));
    let wave_env = env * exposure;
    var compression = 0.85;
    for (var train = 0u; train < 2u; train++) {
        let lambda = 30.0 * select(1.0, 0.61, train == 1u);
        let omega = sqrt(9.81 * 2.0 * HELIO_PI / lambda);
        let k = omega / celerity;
        let amp_scale = select(1.0, 0.45, train == 1u);
        let amplitude_cap = (0.42 * max(depth, 0.0) + 0.14) * select(1.0, 0.5, train == 1u);
        let amplitude = min(0.55 * amp_scale * wave_env * shoal * group, amplitude_cap);
        let phase = omega * (travel.x + shore_params.state.y) + phase_noise * select(1.0, 1.37, train == 1u) + f32(train) * 2.1;
        let c = 0.5 + 0.5 * cos(phase);
        let s = sin(phase);
        let cp = pow(max(c, 1e-4), 0.6);
        let relative_height = amplitude / max(depth, 0.05);
        let cycle = floor((phase + HELIO_PI) / (2.0 * HELIO_PI));
        let early = value_noise(world_xz * 0.03 + vec2f(cycle * 7.3, cycle * 3.1 + f32(train) * 31.0));
        let breaking = smoothstep(0.12 + 0.14 * early, 0.3 + 0.2 * early, relative_height);
        var pitch = 0.0;
        if train == 0u {
            pitch = 2.0 * amplitude * smoothstep(0.16, 0.28, relative_height)
                * (1.0 - smoothstep(0.4, 0.6, relative_height));
        }
        pitch = min(pitch, 0.35 / (1.08 * max(k, 1e-4)));
        compression -= 1.08 * pitch * k;
        let qak = min(compression, amplitude * k * 8.0);
        compression -= qak;
        result.displacement.y += amplitude * (2.0 * cp * c - 0.78);
        let c5 = c * c * c * c * c;
        let horizontal = (qak / max(k, 1e-4)) * s + pitch * c5 * c;
        result.displacement.x += dir.x * horizontal;
        result.displacement.z += dir.y * horizontal;
        let slope_scale = amplitude * k * 1.6 * cp * s
            / max(1.0 - qak * cos(phase) + 3.0 * pitch * k * c5 * s, 0.2);
        result.slope += dir * slope_scale;
        // JS uses mod(phase + PI, 2PI) - PI, equivalent to wrapping phase.
        let psi = atan2(sin(phase), cos(phase));
        result.foam += breaking * smoothstep(-0.45, 0.0, psi) * exp(-max(psi, 0.0) * 0.8);
        if train == 0u {
            // floor-based wrap is stable for negative travel time and keeps
            // the analytic foam-age envelope within [0, 2π).
            let phase_wrapped = phase - floor(phase / (2.0 * HELIO_PI)) * (2.0 * HELIO_PI);
            result.direction = dir;
            result.first_train = vec4f(amplitude, k, omega, phase);
            result.residual = smoothstep(0.18, 0.4, relative_height)
                * (0.3 + 0.45 * exp(-phase_wrapped * 0.4));
        }
    }
    return result;
}

/// Three geometric FFT bands, damped by authored shore depth, plus travelling
/// shore-wave displacement. The capillary band is intentionally omitted.
fn water_geometric_displacement(world_xz: vec2f, cell_size: f32, force_lod0: bool) -> vec3f {
    var displacement = shore_wave(world_xz).displacement;
    let shore_damp = shore_ocean_damp(world_xz);
    for (var i = 0u; i < 3u; i++) {
        let patch_size = WATER_GEOMETRIC_PATCH_SIZES[i];
        let uv = fract((world_xz + ocean_phase.patches[i].xy) / patch_size);
        var lod = 0.0;
        if !force_lod0 {
            lod = max(log2(max(cell_size, 1e-4) * 256.0 / patch_size) + 1.0, 0.0);
        }
        displacement += textureSampleLevel(ocean_displacement, water_samp, uv, i32(i), lod).xyz
            * shore_damp[i];
    }
    return displacement;
}

/// Volume-local interaction height at the same undisplaced XZ used by the mesh.
fn water_interaction_height(world_xz: vec2f, vol: WaterVolume, vol_index: u32) -> f32 {
    let extent = max(vol.bounds_max.xz - vol.bounds_min.xz, vec2f(1e-4));
    let uv = (world_xz - vol.bounds_min.xz) / extent;
    if any(uv < vec2f(0.0)) || any(uv > vec2f(1.0)) { return 0.0; }
    return textureSampleLevel(water_interaction, water_samp, uv, i32(vol_index * 3u), 0.0).r
        * water_wave_amplitude(vol);
}

fn water_surface_height_offset(
    world_xz: vec2f,
    vol: WaterVolume,
    vol_index: u32,
    cell_size: f32,
    force_lod0: bool,
) -> f32 {
    return water_geometric_displacement(world_xz, cell_size, force_lod0).y
        + water_interaction_height(world_xz, vol, vol_index);
}

/// World-XZ interface height. Four fixed-point steps invert the lateral
/// choppiness before evaluating vertical displacement and interaction.
fn water_surface_height_at(
    world_xz: vec2f,
    vol: WaterVolume,
    vol_index: u32,
    cell_size: f32,
    force_lod0: bool,
    inversion_steps: u32,
) -> f32 {
    var param_xz = world_xz;
    for (var step = 0u; step < inversion_steps; step++) {
        let displacement = water_geometric_displacement(param_xz, cell_size, force_lod0);
        param_xz = world_xz - displacement.xz;
    }
    return vol.bounds_max.w
        + water_surface_height_offset(param_xz, vol, vol_index, cell_size, force_lod0);
}
