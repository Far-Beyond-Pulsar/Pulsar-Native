//!use helio_prelude
//!use water_shore_surface
// Half-resolution underwater light shafts: Ultra-Water's `UnderwaterPass`
// SHAFTS pass. Per pixel the view ray is marched through the water column and
// each step adds the sunlight that reached it: the wave caustic pattern at the
// point where the refracted sun ray crosses the surface (so the beams are
// modulated by the same field that lights the seabed), the Beer-Lambert
// attenuation along the sun's path in and the view path out, a depth ramp at
// the surface, and the phase of the water's forward-scattering. rgb is the
// in-scattered sun (without the sun colour and strength), a is the distance
// the composite uses to upsample without halos.

struct WaterVolume {
    bounds_min: vec4f, bounds_max: vec4f, wave_params: vec4f,
    wave_direction: vec4f, water_color: vec4f, extinction: vec4f,
    reflection_refraction: vec4f, caustics_params: vec4f, fog_params: vec4f,
    sim_params: vec4f, shadow_params: vec4f, sun_direction: vec4f,
    ssr_params: vec4f, sim_dynamics: vec4f, wind_params: vec4f,
    world_phase: vec4f,
}
struct OceanPhase { patches: array<vec4f, 4> }
struct VertexOutput { @builtin(position) position: vec4f, @location(0) uv: vec2f }

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<storage, read> volumes: array<WaterVolume>;
@group(0) @binding(2) var depth_texture: texture_depth_2d;
@group(0) @binding(4) var caustics_tex: texture_2d<f32>;
@group(0) @binding(5) var caustics_samp: sampler;
@group(0) @binding(6) var ocean_displacement: texture_2d_array<f32>;
@group(0) @binding(7) var water_samp: sampler;
@group(0) @binding(8) var<uniform> ocean_phase: OceanPhase;
@group(0) @binding(9) var caustics_deep_tex: texture_2d<f32>;
@group(0) @binding(10) var water_interaction: texture_2d_array<f32>;

/// Scattering coefficient of the water (1/m): UnderwaterPass `params.scattering`.
const UNDER_SCATTERING: f32 = 0.03;
/// How far the beams are traced (m): `params.shaftRange`.
const SHAFT_RANGE: f32 = 40.0;
const SHAFT_STEPS: i32 = 24;
/// Texels per metre of the near caustic target (512 texels over 12 m).
const CAUSTIC_TEXELS_PER_M: f32 = 512.0 / 12.0;
const IOR_WATER: f32 = 1.33;

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let p = vec2f(f32((vi << 1u) & 2u), f32(vi & 2u));
    var out: VertexOutput;
    out.position = vec4f(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
    out.uv = p;
    return out;
}

/// `waveCaustic(entryXZ, depth, lod)`: the direct-sun multiplier at `depth`
/// below the surface, mean ~1, from the three-depth FFT caustic fields. Past
/// the 1.5 m slice the deeper, coarser targets take over; no caustic at zero
/// depth.
fn wave_caustic(uv: vec2f, depth: f32, lod: f32) -> f32 {
    var v = textureSampleLevel(caustics_tex, caustics_samp, uv, lod).r;
    if depth > 1.5 {
        let d = textureSampleLevel(caustics_deep_tex, caustics_samp, uv,
            max(lod - 1.0, 0.0) + log2(max(depth / 10.0, 1.0))).rg;
        v = select(
            mix(d.r, d.g, clamp((depth - 4.0) / 6.0, 0.0, 1.0)),
            mix(v, d.r, (depth - 1.5) / 2.5),
            depth < 4.0,
        );
    }
    return mix(1.0, v, clamp(depth / 1.5, 0.0, 1.0));
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let dims = vec2i(textureDimensions(depth_texture));
    let pixel = clamp(vec2i(in.position.xy * 2.0), vec2i(0), dims - vec2i(1));
    let depth = textureLoad(depth_texture, pixel, 0);
    let cam = cameras[0].position_near.xyz;

    var dist = 1000.0;
    var dir: vec3f;
    if helio_is_sky(depth) {
        // Only the direction matters: any depth along the ray gives it.
        dir = normalize(helio_world_from_depth(cameras[0].view_proj_inv, in.uv, 0.5) - cam);
    } else {
        let hit = helio_world_from_depth(cameras[0].view_proj_inv, in.uv, depth);
        dist = min(distance(hit, cam), 1000.0);
        dir = normalize(hit - cam);
    }
    if arrayLength(&volumes) == 0u {
        return vec4f(0.0, 0.0, 0.0, dist);
    }
    let vol = volumes[0];
    if vol.fog_params.y <= 1e-4 || vol.caustics_params.x <= 0.5 {
        return vec4f(0.0, 0.0, 0.0, dist);
    }

    let sun = normalize(vol.sun_direction.xyz);
    let rest = vol.bounds_max.w;
    let sig_t = max(vol.extinction.rgb, vec3f(1e-4)) + vec3f(UNDER_SCATTERING);
    // The sun's direction of travel inside the water; a sun at or below the
    // horizon throws no shafts.
    let rs = refract(-sun, vec3f(0.0, 1.0, 0.0), 1.0 / IOR_WATER);
    var acc = vec3f(0.0);
    if rs.y < -0.05 {
        let range = min(dist, SHAFT_RANGE);
        let step_l = range / f32(SHAFT_STEPS);
        // Interleaved gradient noise, shifted by the golden ratio each frame.
        let jit = fract(52.9829189 * fract(dot(in.position.xy, vec2f(0.06711056, 0.00583715)))
            + cameras[0].jitter_frame.z * 0.618034);
        let mu = dot(dir, -rs);
        let g = 0.7;
        let phase = (1.0 - g * g)
            / (4.0 * HELIO_PI * pow(1.0 + g * g - 2.0 * g * mu, 1.5)) + 0.04;
        // Prefilter along the ray: rays sweeping across the caustics average to
        // haze, rays along the light keep shafts.
        let to_entry = rs.xz / rs.y;
        let dbase = dir.xz - to_entry * dir.y;
        let lod = max(log2(max(length(dbase) * step_l * CAUSTIC_TEXELS_PER_M, 1.0)), 1.0);
        for (var i = 0; i < SHAFT_STEPS; i++) {
            let s = (f32(i) + jit) * step_l;
            let p = cam + dir * s;
            let pd = rest - p.y;
            if pd <= 0.0 { continue; }
            let entry = p.xz + to_entry * pd;
            let uv = fract((entry + ocean_phase.patches[2].xy) / 12.0);
            acc += wave_caustic(uv, pd, lod)
                * exp(-sig_t * (s + pd / -rs.y))
                * smoothstep(0.0, 0.6, pd);
        }
        acc *= phase * step_l;
    }
    return vec4f(acc, dist);
}
