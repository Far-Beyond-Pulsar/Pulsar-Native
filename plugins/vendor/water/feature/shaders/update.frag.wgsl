// update.frag.wgsl — one step of the shallow-water wave propagation.
//
// Texture layout (Rgba16Float):
//   R = height
//   G = velocity
//   B = normal.x  (written by normal pass)
//   A = normal.z  (written by normal pass)

@group(0) @binding(0) var water_texture: texture_2d<f32>;
@group(0) @binding(1) var water_sampler: sampler;

// The pass-wide dynamics below apply to a volume whose row leaves its own
// at zero (`sim_dynamics.x`, its spring, is 0: a host that writes pass rows
// itself); a volume authored with dynamics simulates with its own, so two
// volumes settle and move independently (Pulsar-Native#1065).
struct UpdateUniforms {
    /// Texel size: (1 / texture_width, 1 / texture_height)
    delta: vec2<f32>,
    /// Wave spring constant (scales wave propagation speed in SWE).
    spring: f32,
    /// Per-step energy damping multiplier (0..1).
    damping: f32,
    /// Wind direction in XZ sim-texture space (pre-normalised; zero = no wind).
    wind_dir: vec2<f32>,
    /// Wind strength multiplier. 0 = calm; ~1 = gentle ripples; ~5 = choppy.
    wind_strength: f32,
    /// Simulated seconds on the renderer's frame clock -- scaled by the
    /// volume's wave speed, it drives traveling wave phases.
    time: f32,
    /// Wave scale: smaller = shorter wavelengths (chop); larger = long swells.
    wave_scale: f32,
    /// Fixed sim-step duration for stable injection magnitude regardless of wave_speed.
    time_step: f32,
    /// Patch size (metres per tile) for this cascade — scales wind wavenumbers
    /// so smaller patches produce shorter wavelengths (choppy) and larger patches
    /// produce long swells.
    cascade_patch_size: f32,
    /// Cascade index (0, 1, 2) — unused in shader body but available for debug.
    cascade_id: u32,
    /// Wave speed: how fast `time` runs for the wave phases.
    wave_speed: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}
@group(0) @binding(2) var<uniform> u: UpdateUniforms;

/// `GpuWaterVolume` (16 vec4s); only the dynamics are read here.
struct WaterVolume {
    bounds_min:            vec4<f32>,
    bounds_max:            vec4<f32>,
    wave_params:           vec4<f32>,  // z = wave speed
    wave_direction:        vec4<f32>,
    water_color:           vec4<f32>,
    extinction:            vec4<f32>,
    reflection_refraction: vec4<f32>,
    caustics_params:       vec4<f32>,
    fog_params:            vec4<f32>,
    sim_params:            vec4<f32>,
    shadow_params:         vec4<f32>,
    sun_direction:         vec4<f32>,
    ssr_params:            vec4<f32>,
    sim_dynamics:          vec4<f32>,  // x = spring, y = damping, z = wave scale
    wind_params:           vec4<f32>,  // xy = direction (XZ), z = strength
    world_phase:           vec4<f32>,
}
@group(0) @binding(3) var<storage, read> volumes: array<WaterVolume>;

/// One volume's dynamics: its row's, or the pass-wide ones for a row that
/// leaves them zero.
struct Dynamics {
    spring: f32,
    damping: f32,
    wind_dir: vec2<f32>,
    wind_strength: f32,
    wave_scale: f32,
    wave_speed: f32,
}

fn volume_dynamics(volume: u32) -> Dynamics {
    var d = Dynamics(u.spring, u.damping, u.wind_dir, 0.0, u.wave_scale, u.wave_speed);
    if volume >= arrayLength(&volumes) {
        return d;
    }
    let row = volumes[volume];
    if row.sim_dynamics.x <= 0.0 {
        return d;
    }
    d.spring = clamp(row.sim_dynamics.x, 0.1, 2.0);
    d.damping = clamp(row.sim_dynamics.y, 0.0, 1.0);
    d.wave_scale = max(row.sim_dynamics.z, 0.01);
    let dir = row.wind_params.xy;
    let len = length(dir);
    d.wind_dir = select(vec2<f32>(0.0), dir / len, len > 1e-6);
    d.wind_strength = max(row.wind_params.z, 0.0);
    d.wave_speed = max(row.wave_params.z, 0.0);
    // The open sea's wind waves come from the global ocean FFT
    // (`ocean_fft.rs`); the volume solver carries local disturbances
    // (objects, drops) plus the cheap traveling wind trains below, which give
    // a volume its own wind look where the FFT sea does not reach.
    return d;
}

@fragment
fn fs_main(
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) volume: u32,
) -> @location(0) vec4<f32> {
    let dynamics = volume_dynamics(volume);
    var info = textureSample(water_texture, water_sampler, uv);

    let dx = vec2<f32>(u.delta.x, 0.0);
    let dy = vec2<f32>(0.0, u.delta.y);

    // Average of the four cardinal neighbours' heights
    let avg = (
        textureSample(water_texture, water_sampler, uv - dx).r +
        textureSample(water_texture, water_sampler, uv - dy).r +
        textureSample(water_texture, water_sampler, uv + dx).r +
        textureSample(water_texture, water_sampler, uv + dy).r
    ) * 0.25;

    // Velocity = displacement toward mean (spring) + energy damping
    info.g += (avg - info.r) * dynamics.spring;
    info.g *= dynamics.damping;
    // Euler-integrate height
    info.r += info.g;

    // Wind sea and swell come from the four-cascade FFT. This solver remains
    // solely for localized hitbox/drop disturbances.
    return info;
}
