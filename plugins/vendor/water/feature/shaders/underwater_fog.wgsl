//!use helio_prelude
//!use water_shore_surface
//!use atmosphere
//!use water_sky
//
// Underwater participating medium.
//
// Runs fullscreen when the camera is inside a water volume. Every pixel is
// integrated against the distance the view ray travels through water, so near
// objects stay readable and far ones dissolve into the medium — which is the
// difference between being submerged and having a filter laid over the image.
//
// This replaces a depth-independent screen filter (lens warp + chromatic
// aberration + flat tint + vignette). Those touches are kept but scaled back,
// because the depth cue now does the work. See #145.
//
// Output goes to a scratch texture with alpha = 1; the Rust side blits it back
// over water_output.
//
// Bindings
//   0  cameras       storage  array<Camera, 2> (prelude layout)
//   1  volumes       storage  array<WaterVolume>
//   2  scene_tex     texture  water_output bound as source
//   3  scene_samp    sampler  linear clamp
//   4  depth_texture texture_depth_2d
//   5  depth_samp    sampler  nearest clamp
//   6  water_interaction texture array  (legacy volume-local height)
//   7  water_samp    sampler  linear

struct WaterVolume {
    bounds_min:            vec4f,
    bounds_max:            vec4f,  // w = surface_height
    wave_params:           vec4f,  // x = wave_amplitude
    wave_direction:        vec4f,
    water_color:           vec4f,  // xyz = medium colour
    extinction:            vec4f,  // xyz = absorption per metre
    reflection_refraction: vec4f,
    caustics_params:       vec4f,
    fog_params:            vec4f,  // x = density, y = god_rays_intensity
    sim_params:            vec4f,
    shadow_params:         vec4f,
    sun_direction:         vec4f,
    ssr_params:            vec4f,
    sim_dynamics:          vec4f,
    wind_params:           vec4f,
    world_phase:           vec4f,
}

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<storage, read> volumes:    array<WaterVolume>;
@group(0) @binding(2) var scene_tex:     texture_2d<f32>;
@group(0) @binding(3) var scene_samp:    sampler;
@group(0) @binding(4) var depth_texture: texture_depth_2d;
@group(0) @binding(5) var depth_samp:    sampler;
@group(0) @binding(6) var water_interaction: texture_2d_array<f32>;
@group(0) @binding(7) var water_samp:    sampler;
@group(0) @binding(9) var ocean_displacement: texture_2d_array<f32>;
struct OceanPhase { patches: array<vec4f, 4> }
@group(0) @binding(10) var<uniform> ocean_phase: OceanPhase;
@group(0) @binding(11) var shafts_tex: texture_2d<f32>;
const UNDER_SCATTERING: f32 = 0.03;  // UnderwaterPass params.scattering (1/m)
@group(0) @binding(20) var<uniform> atmosphere: AtmosphereFrame;
@group(0) @binding(21) var sky_view_lut: texture_2d<f32>;
fn water_sky_lut(uv: vec2f) -> vec3f {
    return textureSampleLevel(sky_view_lut, scene_samp, uv, 0.0).rgb;
}

/// Vertical distance over which entering the water ramps in, in metres. Without
/// this the effect switches on the instant the camera crosses the plane, which
/// pops hard exactly when the camera sits at the waterline with waves rolling
/// over it.
const SUBMERGE_BLEND: f32 = 0.35;


struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0)       uv:       vec2f,
}

@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> VertexOutput {
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    var out: VertexOutput;
    out.position = vec4f(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2f(x, y);
    return out;
}

// ── Volume geometry ──────────────────────────────────────────────────────────

/// The water height is sampled at LOD 0 for fullscreen depth tests. This keeps
/// the interface temporally stable as half/full-resolution pixel footprints
/// change and includes the finest geometric FFT band available to the surface.
fn water_surface_at(world_xz: vec2f, vol: WaterVolume, vol_idx: u32) -> f32 {
    return water_surface_height_at(world_xz, vol, vol_idx, 0.0, true, 4u);
}

/// Reconstruct the global FFT interface for underwater sky rays. Central
/// differences use a footprint close to the finest authored band, so the
/// Snell window follows the actual displaced surface instead of a flat plane.
fn global_surface_height(world_xz: vec2f, vol: WaterVolume, vol_idx: u32) -> f32 {
    return water_surface_at(world_xz, vol, vol_idx);
}

fn global_surface_normal(world_xz: vec2f, vol: WaterVolume, vol_idx: u32) -> vec3f {
    let e = 0.12;
    let dx = global_surface_height(world_xz + vec2f(e, 0.0), vol, vol_idx)
           - global_surface_height(world_xz - vec2f(e, 0.0), vol, vol_idx);
    let dz = global_surface_height(world_xz + vec2f(0.0, e), vol, vol_idx)
           - global_surface_height(world_xz - vec2f(0.0, e), vol, vol_idx);
    return normalize(vec3f(-dx / (2.0 * e), 1.0, -dz / (2.0 * e)));
}

fn underwater_sky(ray: vec3f, light_dir: vec3f) -> vec3f {
    let up = clamp(ray.y, 0.0, 1.0);
    let sky = mix(vec3f(0.80, 0.90, 1.0), vec3f(0.10, 0.30, 0.80), up * up);
    let sun = pow(max(dot(normalize(light_dir), ray), 0.0), 5000.0);
    return sky + vec3f(sun) * vec3f(10.0, 8.0, 6.0);
}

// ── Noise (lens wobble) ──────────────────────────────────────────────────────

fn hash2(p: vec2f) -> vec2f {
    let k = vec2f(127.1, 311.7);
    let s = sin(dot(p, k)) * 43758.5453;
    return fract(vec2f(s, s + 0.618));
}

fn smooth_noise(p: vec2f) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = dot(hash2(i),                   f - vec2f(0.0, 0.0));
    let b = dot(hash2(i + vec2f(1.0, 0.0)), f - vec2f(1.0, 0.0));
    let c = dot(hash2(i + vec2f(0.0, 1.0)), f - vec2f(0.0, 1.0));
    let d = dot(hash2(i + vec2f(1.0, 1.0)), f - vec2f(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y) * 0.5 + 0.5;
}

fn water_distortion(uv: vec2f, t: f32) -> vec2f {
    let scale = 3.0;
    let nx = smooth_noise(uv * scale + vec2f( t * 0.11, t * 0.07)) - 0.5;
    let ny = smooth_noise(uv * scale + vec2f(-t * 0.09, t * 0.13)) - 0.5;
    return vec2f(nx, ny);
}

/// Four-tap depth-guided reconstruction of the half-resolution shaft buffer.
fn upsample_shafts(uv: vec2f, linear_depth: f32) -> vec3f {
    // Ultra-Water's weighting: the 4 nearest by bilinear weight, divided by
    // how far their distance disagrees with this pixel's, so shafts in front
    // of an object don't bleed onto it. rgb in-scatter, a distance.
    let dims = vec2i(textureDimensions(shafts_tex));
    let half_pos = uv * vec2f(dims) - vec2f(0.5);
    let base = vec2i(floor(half_pos));
    let f = fract(half_pos);
    var sum = vec3f(0.0);
    var total = 1e-5;
    for (var y = 0; y < 2; y++) {
        for (var x = 0; x < 2; x++) {
            let coord = clamp(base + vec2i(x, y), vec2i(0), dims - vec2i(1));
            let t = textureLoad(shafts_tex, coord, 0);
            let bilinear = select(1.0 - f.x, f.x, x == 1) * select(1.0 - f.y, f.y, y == 1);
            let w = bilinear / (1e-3 + abs(t.a - linear_depth) / max(linear_depth, 0.5));
            sum += t.rgb * w;
            total += w;
        }
    }
    return sum / total;
}

// ── Fragment ─────────────────────────────────────────────────────────────────

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let cam_pos = cameras[0].position_near.xyz;

    // ── Which volume is the camera in, and how far under? ────────────────────
    var vol_idx: i32 = -1;
    var submersion = 0.0;
    for (var i = 0u; i < arrayLength(&volumes); i++) {
        let v = volumes[i];
        // Against the DISPLACED surface, not the flat plane — otherwise the
        // test is wrong precisely when the camera is at the waterline. The
        // old volume floor is not a boundary for the continuous global ocean.
        let surf = water_surface_at(cam_pos.xz, v, i);
        let depth_under = surf - cam_pos.y;
        if depth_under <= -SUBMERGE_BLEND { continue; }

        submersion = smoothstep(-SUBMERGE_BLEND, SUBMERGE_BLEND, depth_under);
        vol_idx = i32(i);
        break;
    }

    let scene_raw = textureSampleLevel(scene_tex, scene_samp, in.uv, 0.0);
    if vol_idx < 0 || submersion <= 0.0 {
        return scene_raw;
    }

    let vol       = volumes[u32(vol_idx)];
    let surface_h = water_surface_at(cam_pos.xz, vol, u32(vol_idx));
    let cam_depth = max(surface_h - cam_pos.y, 0.0);

    // ── Lens wobble + chromatic aberration ───────────────────────────────────
    // Kept, but well below the previous magnitudes: the depth-based extinction
    // below now carries the underwater read, and these become distracting once
    // it does.
    let t         = cameras[0].jitter_frame.z * 0.016 * max(vol.wave_params.z, 0.1);
    let dist_raw  = water_distortion(in.uv, t) * 0.7
                  + water_distortion(in.uv * 2.1 + vec2f(0.37, 0.71), t * 0.6) * 0.3;
    let dist_str  = clamp(water_wave_amplitude(vol) * (0.05 + cam_depth * 0.01), 0.002, 0.06);
    let warp_uv   = in.uv + dist_raw * dist_str;

    let ca     = clamp(cam_depth * 0.0015, 0.0002, 0.004);
    let radial = in.uv - 0.5;
    let uv_r   = clamp(warp_uv + radial * ca,       vec2f(0.001), vec2f(0.999));
    let uv_g   = clamp(warp_uv,                     vec2f(0.001), vec2f(0.999));
    let uv_b   = clamp(warp_uv - radial * ca * 1.4, vec2f(0.001), vec2f(0.999));

    let scene = vec3f(
        textureSampleLevel(scene_tex, scene_samp, uv_r, 0.0).r,
        textureSampleLevel(scene_tex, scene_samp, uv_g, 0.0).g,
        textureSampleLevel(scene_tex, scene_samp, uv_b, 0.0).b,
    );

    // ── Distance through the medium ──────────────────────────────────────────
    let depth = textureSampleLevel(depth_texture, depth_samp, uv_g, 0);
    let far   = cameras[0].forward_far.w;

    var dist: f32;
    var ray_dir: vec3f;
    var lit = scene;
    // The distance the shafts pass stored for this pixel (before `dist` is
    // clipped at the surface below); sky pixels store its far clamp.
    var shaft_dist = 1000.0;
    if helio_is_sky(depth) {
        // Nothing there: the ray runs until the medium has fully absorbed it.
        ray_dir = normalize(helio_world_from_depth(cameras[0].view_proj_inv, uv_g, 0.5) - cam_pos);
        dist    = far;
    } else {
        let world_pos = helio_world_from_depth(cameras[0].view_proj_inv, uv_g, depth);
        ray_dir = normalize(world_pos - cam_pos);
        dist    = distance(world_pos, cam_pos);
        shaft_dist = min(dist, 1000.0);

        // The old caustics map was projected into a legacy volume footprint.
        // The surface path now evaluates a continuous FFT area ratio; a
        // matching submerged-camera shaft/receiver caustics path still needs
        // a graph-owned global field and remains a separate follow-up.
    }

    // A ray heading upward leaves the water at the surface; it should only
    // accumulate medium up to that crossing.
    if ray_dir.y > 1e-4 {
        var t_surface = (surface_h - cam_pos.y) / ray_dir.y;
        // Resolve the ray/interface intersection against the displaced FFT
        // field. Two fixed-point steps are stable for the bounded wave slope
        // and avoid the visible flat-waterline cut at grazing angles.
        for (var step = 0u; step < 2u; step++) {
            if t_surface > 0.0 {
                let point = cam_pos + ray_dir * t_surface;
                t_surface = (global_surface_height(point.xz, vol, u32(vol_idx)) - cam_pos.y) / ray_dir.y;
            }
        }
        if t_surface > 0.0 {
            dist = min(dist, t_surface);
            // The opaque scene's sky ray is the only environment available to
            // this pass. Refract it through the local water interface, then
            // apply Schlick transmission inside Snell's window. Rays outside
            // the critical cone stay in the submerged medium (TIR).
            if helio_is_sky(depth) {
                let hit_pos = cam_pos + ray_dir * t_surface;
            let normal_up = global_surface_normal(hit_pos.xz, vol, u32(vol_idx));
                let transmitted = refract(ray_dir, -normal_up, 1.0 / max(vol.sim_params.x, 1.0));
                if dot(transmitted, transmitted) > 0.25 {
                    let cos_theta = clamp(dot(-ray_dir, -normal_up), 0.0, 1.0);
                    let f0_base = (max(vol.sim_params.x, 1.0) - 1.0)
                                / (max(vol.sim_params.x, 1.0) + 1.0);
                    let f0 = f0_base * f0_base;
                    let fresnel = f0 + (1.0 - f0) * pow(1.0 - cos_theta, 5.0);
                    let window = underwater_sky(normalize(transmitted), normalize(vol.sun_direction.xyz));
                    lit = mix(lit, window, 1.0 - fresnel);
                }
            }
        }
    }

    // ── Beer-Lambert toward the in-scattered medium colour ───────────────────
    // Ultra-Water's UnderwaterPass, as written: extinction plus a 0.03 /m
    // scattering coefficient, towards the surface underside's deep-water
    // colour (the same `water_sky` helpers the surface uses), darker with the
    // camera's depth and brighter looking up towards the light.
    let extinction = max(vol.extinction.rgb, vec3f(1e-4));
    let sigma_t    = extinction + vec3f(UNDER_SCATTERING);
    let transmit   = exp(-sigma_t * dist);

    let sun_dir  = normalize(vol.sun_direction.xyz);
    let sun_rad  = water_sun_radiance(sun_dir);
    let inscatter = vol.water_color.rgb
        * (water_sky_ambient(sun_dir) + sun_rad * max(sun_dir.y, 0.0))
        * exp(-extinction * cam_depth * 0.5)
        * mix(0.55, 1.35, ray_dir.y * 0.5 + 0.5);

    var color = lit * transmit + inscatter * (1.0 - transmit);

    // ── God rays ─────────────────────────────────────────────────────────────
    // col += sunColor * shafts * (scattering * shaftStrength): the JS default
    // strength is 2, here 2 x the volume's god-ray intensity (1.0 by default).
    // The shafts buffer holds the in-scattered sun for the distance the
    // shafts pass saw; sky pixels use the same far distance it clamps to.
    color += sun_rad * upsample_shafts(uv_g, shaft_dist)
        * (UNDER_SCATTERING * 2.0 * vol.fog_params.y);

    // Ramp the whole effect in across the waterline.
    return vec4f(mix(scene_raw.rgb, color, submersion), 1.0);
}
