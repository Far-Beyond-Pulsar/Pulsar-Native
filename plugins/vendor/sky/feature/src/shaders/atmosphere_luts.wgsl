//!use atmosphere
// Atmosphere LUT kernels (see atmosphere_common.wgsl). Every kernel reads
// the frame `resolve` wrote; all leave their outputs untouched while no
// atmosphere is active.

struct Camera {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    position_near: vec4<f32>,
    forward_far: vec4<f32>,
    jitter_frame: vec4<f32>,
    prev_view_proj: mat4x4<f32>,
}

struct Resolve {
    // The frame's world origin (camera-relative frames), split into an f32
    // and its remainder so planet centres far from it stay exact.
    origin_hi: vec4<f32>,
    origin_lo: vec4<f32>,
    // x: how far the frame clock advanced (s), which the wind carries the
    // clouds over.
    clock: vec4<f32>,
}

@group(0) @binding(0) var<uniform> resolve_input: Resolve;
@group(0) @binding(1) var<storage, read> atmospheres: array<AtmosphereParams>;
// `scene_lights`: 128-byte rows (8 vec4s).
@group(0) @binding(2) var<storage, read> lights: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(4) var<storage, read_write> frame: AtmosphereFrame;
@group(0) @binding(5) var lut_sampler: sampler;

@group(1) @binding(0) var transmittance_out: texture_storage_2d<rgba16float, write>;
@group(1) @binding(1) var transmittance_lut: texture_2d<f32>;
@group(1) @binding(2) var multi_scattering_out: texture_storage_2d<rgba16float, write>;
@group(1) @binding(3) var multi_scattering_lut: texture_2d<f32>;
@group(1) @binding(4) var sky_view_out: texture_storage_2d<rgba16float, write>;
@group(1) @binding(5) var sky_view_lut: texture_2d<f32>;
@group(1) @binding(6) var aerial_out: texture_storage_3d<rgba16float, write>;
@group(1) @binding(7) var<storage, read_write> sky_table_out: array<vec4<f32>>;

const LIGHT_ROW_VEC4S: u32 = 8u;

// Picks the first enabled atmosphere and the first directional light, and
// places the planet relative to the camera.
@compute @workgroup_size(1)
fn resolve() {
    frame.planet = vec4<f32>(0.0);
    var found = false;
    var params: AtmosphereParams;
    for (var i = 0u; i < arrayLength(&atmospheres); i++) {
        let a = atmospheres[i];
        if a.enabled != 0u && a.top_radius > a.bottom_radius && a.bottom_radius > 0.0 {
            params = a;
            found = true;
            break;
        }
    }
    if !found { return; }
    // Keep authored atmosphere data intact while resolving a cloudless XR sky.
    if resolve_input.clock.y > 0.5 {
        params.clouds_enabled = 0u;
    }
    // Sun: the first directional light with any intensity (direction_outer
    // is the direction the light travels).
    var sun = vec3<f32>(0.0, 1.0, 0.0);
    var illuminance = vec3<f32>(0.0);
    let rows = arrayLength(&lights) / LIGHT_ROW_VEC4S;
    for (var i = 0u; i < rows; i++) {
        let base = i * LIGHT_ROW_VEC4S;
        let colour = lights[base + 2u];
        let light_type = bitcast<u32>(lights[base + 3u].y);
        if light_type == 0u && colour.w > 0.0 {
            sun = -normalize(lights[base + 1u].xyz);
            illuminance = colour.rgb * colour.w;
            break;
        }
    }
    var center = params.center;
    if params.placement == 0u { center = vec3<f32>(0.0, -params.bottom_radius * 1000.0, 0.0); }
    let camera = cameras[0].position_near.xyz;
    // Metres; the large terms cancel first.
    let relative = (center - resolve_input.origin_hi.xyz) - resolve_input.origin_lo.xyz - camera;
    let planet = relative * 0.001;
    let eye = -planet;
    let r = length(eye);
    // Aerial perspective reaches the horizon (and the far side of the
    // atmosphere's shell seen past it), at least 32 km.
    let horizon = sqrt(max(r * r - params.bottom_radius * params.bottom_radius, 0.0));
    let shell = sqrt(max(params.top_radius * params.top_radius - params.bottom_radius * params.bottom_radius, 0.0));
    let range = clamp(horizon + shell, 32.0, 4000.0);
    frame.planet = vec4<f32>(planet, 1.0);
    frame.sun = vec4<f32>(sun, cos(params.sun_angular_radius));
    frame.sun_illuminance = vec4<f32>(illuminance, range);
    frame.eye = vec4<f32>(eye, 0.0);
    frame.params = params;
    // The wind carries the clouds' noise; each offset wraps to its noise's
    // period, so it never loses precision however long the clock runs.
    // The planet's weather's phase advances by the clock times the
    // evolution's time lapse.
    let drift = params.cloud_wind * (resolve_input.clock.x * 0.001);
    let phase = fract(frame.cloud_groups_offset.w + resolve_input.clock.x * max(params.cloud_evolution, 0.0) / WEATHER_PERIOD_S);
    frame.cloud_groups_offset = vec4<f32>(fract(frame.cloud_groups_offset.xyz - drift / max(params.cloud_weather_scale, 1e-3)), phase);
    let meso = drift / max(params.cloud_mesoscale_scale, 1e-3);
    frame.cloud_meso_offset = vec4<f32>(fract(frame.cloud_meso_offset.xyz - CLOUD_MESO_TURN_A * meso), 0.0);
    frame.cloud_meso_fine_offset = vec4<f32>(fract(frame.cloud_meso_fine_offset.xyz - CLOUD_MESO_TURN_B * (meso * CLOUD_MESO_RATIO)), 0.0);
    frame.cloud_shape_offset = vec4<f32>(fract(frame.cloud_shape_offset.xyz - drift / max(params.cloud_shape_scale, 1e-3)), 0.0);
    frame.cloud_detail_offset = vec4<f32>(fract(frame.cloud_detail_offset.xyz - drift / max(params.cloud_detail_scale, 1e-3)), 0.0);
}

fn atmosphere_active() -> bool {
    return frame.planet.w > 0.5;
}

// The shared integrators, bound to this pass's LUTs.
fn sun_transmittance(r: f32, mu_s: f32) -> vec3<f32> {
    return atmosphere_sun_transmittance_lut(frame.params, transmittance_lut, lut_sampler, r, mu_s);
}

fn integrate(o: vec3<f32>, d: vec3<f32>, t0: f32, t1: f32, steps: f32) -> Scattering {
    return atmosphere_integrate(frame.params, frame.sun.xyz, transmittance_lut, multi_scattering_lut,
        lut_sampler, o, d, t0, t1, steps, true);
}

@compute @workgroup_size(8, 8)
fn transmittance_lut_kernel(@builtin(global_invocation_id) id: vec3<u32>) {
    if !atmosphere_active() || any(vec2<f32>(id.xy) >= TRANSMITTANCE_SIZE) { return; }
    let p = frame.params;
    let uv = (vec2<f32>(id.xy) + 0.5) / TRANSMITTANCE_SIZE;
    let r_mu = transmittance_r_mu(p, uv);
    let o = vec3<f32>(0.0, r_mu.x, 0.0);
    let d = vec3<f32>(sqrt(max(1.0 - r_mu.y * r_mu.y, 0.0)), r_mu.y, 0.0);
    let length_top = max(ray_sphere(o, d, p.top_radius).y, 0.0);
    let steps = 40.0;
    let dt = length_top / steps;
    var depth = vec3<f32>(0.0);
    for (var i = 0.0; i < steps; i += 1.0) {
        let x = o + d * (i + 0.5) * dt;
        depth += medium_at(p, length(x) - p.bottom_radius).extinction * dt;
    }
    textureStore(transmittance_out, vec2<i32>(id.xy), vec4<f32>(exp(-depth), 1.0));
}

// Second-order scattering transfer (Hillaire's psi_ms): the light reaching
// a point after any number of isotropic bounces, for a unit sun.
@compute @workgroup_size(8, 8)
fn multi_scattering_kernel(@builtin(global_invocation_id) id: vec3<u32>) {
    if !atmosphere_active() || any(vec2<f32>(id.xy) >= vec2<f32>(MULTI_SCATTERING_SIZE)) { return; }
    let p = frame.params;
    let uv = (vec2<f32>(id.xy) + 0.5) / MULTI_SCATTERING_SIZE;
    let mu_s = uv.x * 2.0 - 1.0;
    let r = p.bottom_radius + clamp(uv.y, 0.002, 0.998) * (p.top_radius - p.bottom_radius);
    let o = vec3<f32>(0.0, r, 0.0);
    let sun = vec3<f32>(sqrt(max(1.0 - mu_s * mu_s, 0.0)), mu_s, 0.0);
    let directions = 64u;
    var second = vec3<f32>(0.0);
    var transfer = vec3<f32>(0.0);
    for (var n = 0u; n < directions; n++) {
        // Fibonacci sphere.
        let z = 1.0 - (f32(n) + 0.5) * 2.0 / f32(directions);
        let a = f32(n) * 2.39996323;
        let s = sqrt(max(1.0 - z * z, 0.0));
        let d = vec3<f32>(s * cos(a), z, s * sin(a));
        let ground = ray_sphere(o, d, p.bottom_radius);
        let top = ray_sphere(o, d, p.top_radius);
        var t_max = max(top.y, 0.0);
        let hit = ground.x > 0.0;
        if hit { t_max = ground.x; }
        let steps = 20.0;
        let dt = t_max / steps;
        var throughput = vec3<f32>(1.0);
        var light = vec3<f32>(0.0);
        var bounce = vec3<f32>(0.0);
        for (var i = 0.0; i < steps; i += 1.0) {
            let x = o + d * (i + 0.5) * dt;
            let rx = length(x);
            let m = medium_at(p, rx - p.bottom_radius);
            let step_t = exp(-m.extinction * dt);
            let integral = (vec3<f32>(1.0) - step_t) / max(m.extinction, vec3<f32>(1e-7));
            let lit = sun_transmittance(rx, dot(x / rx, sun));
            light += throughput * m.scattering * lit / (4.0 * ATMOSPHERE_PI) * integral;
            bounce += throughput * m.scattering * integral;
            throughput *= step_t;
        }
        if hit {
            let x = o + d * t_max;
            let n_ground = normalize(x);
            let lit = sun_transmittance(p.bottom_radius, dot(n_ground, sun));
            light += throughput * lit * max(dot(n_ground, sun), 0.0) * p.ground_albedo / ATMOSPHERE_PI;
        }
        // Uniform phase over the sphere: each direction weighs 1 / count.
        second += light / f32(directions);
        transfer += bounce / f32(directions);
    }
    let psi = second / max(vec3<f32>(1.0) - transfer, vec3<f32>(1e-3));
    textureStore(multi_scattering_out, vec2<i32>(id.xy), vec4<f32>(psi, 1.0));
}

@compute @workgroup_size(8, 8)
fn sky_view_kernel(@builtin(global_invocation_id) id: vec3<u32>) {
    if !atmosphere_active() || any(vec2<f32>(id.xy) >= SKY_VIEW_SIZE) { return; }
    let p = frame.params;
    let eye = frame.eye.xyz;
    let r = length(eye);
    let uv = (vec2<f32>(id.xy) + 0.5) / SKY_VIEW_SIZE;
    let angles = sky_view_angles(p, r, uv);
    let basis = sky_view_basis(eye / r, frame.sun.xyz);
    let d = basis * vec3<f32>(sin(angles.x) * cos(angles.y), cos(angles.x), sin(angles.x) * sin(angles.y));
    let segment = atmosphere_segment(frame.params, eye, d, true);
    var radiance = vec3<f32>(0.0);
    if segment.y > segment.x {
        let s = integrate(eye, d, segment.x, segment.y, 32.0);
        radiance = s.radiance;
        if segment.z > 0.5 {
            // The ground seen through the atmosphere (where no geometry
            // draws it): sunlit Lambertian albedo.
            let x = eye + d * segment.y;
            let n = normalize(x);
            let mu_s = dot(n, frame.sun.xyz);
            radiance += s.transmittance * sun_transmittance(p.bottom_radius, mu_s) * max(mu_s, 0.0) * p.ground_albedo / ATMOSPHERE_PI;
        }
    }
    textureStore(sky_view_out, vec2<i32>(id.xy), vec4<f32>(radiance, 1.0));
}

// Aerial perspective: per froxel column, in-scattered radiance (unit sun)
// and mean transmittance from the camera to each slice's distance.
@compute @workgroup_size(8, 8)
fn aerial_kernel(@builtin(global_invocation_id) id: vec3<u32>) {
    if !atmosphere_active() || any(vec2<f32>(id.xy) >= vec2<f32>(AERIAL_SLICES)) { return; }
    let camera = cameras[0];
    let uv = (vec2<f32>(id.xy) + 0.5) / AERIAL_SLICES;
    let d = normalize(atmosphere_view_ray(camera.view, camera.proj, vec2<f32>(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0)));
    let eye = frame.eye.xyz;
    let range = frame.sun_illuminance.w;
    // Geometry the volume is sampled for may lie below the analytic ground.
    let segment = atmosphere_segment(frame.params, eye, d, false);
    var radiance = vec3<f32>(0.0);
    var throughput = vec3<f32>(1.0);
    var t_prev = 0.0;
    for (var k = 0u; k < u32(AERIAL_SLICES); k++) {
        let t = aerial_distance((f32(k) + 0.5) / AERIAL_SLICES, range);
        // Only the part of [t_prev, t] inside the atmosphere scatters.
        let a = clamp(t_prev, segment.x, max(segment.y, segment.x));
        let b = clamp(t, segment.x, max(segment.y, segment.x));
        if b > a {
            let s = integrate(eye, d, a, b, 4.0);
            radiance += throughput * s.radiance;
            throughput *= s.transmittance;
        }
        t_prev = t;
        let mean = dot(throughput, vec3<f32>(1.0 / 3.0));
        textureStore(aerial_out, vec3<i32>(vec2<i32>(id.xy), i32(k)), vec4<f32>(radiance, mean));
    }
}

// Sky irradiance (diffuse ambient) projected onto L2 spherical harmonics
// from the sky-view LUT, convolved with the cosine lobe and divided by pi.
var<workgroup> partial: array<array<vec3<f32>, 9>, 64>;

fn sky_radiance(d: vec3<f32>) -> vec3<f32> {
    let r = length(frame.eye.xyz);
    let angles = sky_view_direction_angles(frame, d);
    let uv = sky_view_uv(frame.params, r, angles.x, angles.y);
    return textureSampleLevel(sky_view_lut, lut_sampler, uv, 0.0).rgb;
}

@compute @workgroup_size(64)
fn irradiance_kernel(@builtin(local_invocation_index) li: u32) {
    // No early return: the barrier below needs uniform control flow.
    let samples = 16u;
    let total = 64u * samples;
    var sh: array<vec3<f32>, 9>;
    for (var c = 0u; c < 9u; c++) { sh[c] = vec3<f32>(0.0); }
    for (var s = 0u; s < samples; s++) {
        let n = li * samples + s;
        let z = 1.0 - (f32(n) + 0.5) * 2.0 / f32(total);
        let a = f32(n) * 2.39996323;
        let q = sqrt(max(1.0 - z * z, 0.0));
        let d = vec3<f32>(q * cos(a), z, q * sin(a));
        let l = sky_radiance(d);
        sh[0] += l * 0.282095;
        sh[1] += l * 0.488603 * d.y;
        sh[2] += l * 0.488603 * d.z;
        sh[3] += l * 0.488603 * d.x;
        sh[4] += l * 1.092548 * d.x * d.y;
        sh[5] += l * 1.092548 * d.y * d.z;
        sh[6] += l * 0.315392 * (3.0 * d.z * d.z - 1.0);
        sh[7] += l * 1.092548 * d.x * d.z;
        sh[8] += l * 0.546274 * (d.x * d.x - d.y * d.y);
    }
    partial[li] = sh;
    workgroupBarrier();
    if li != 0u || !atmosphere_active() { return; }
    let weight = 4.0 * ATMOSPHERE_PI / f32(total);
    // Cosine-lobe convolution (Ramamoorthi & Hanrahan), then / pi.
    let band = array<f32, 9>(1.0, 2.0 / 3.0, 2.0 / 3.0, 2.0 / 3.0, 0.25, 0.25, 0.25, 0.25, 0.25);
    for (var c = 0u; c < 9u; c++) {
        var sum = vec3<f32>(0.0);
        for (var t = 0u; t < 64u; t++) { sum += partial[t][c]; }
        frame.irradiance[c] = vec4<f32>(sum * weight * band[c] * frame.sun_illuminance.rgb, 0.0);
    }
}

// The sky's irradiance table (`AtmosphereFrame::sky_table`): one workgroup
// per entry (altitude, sun cosine), its 64 lanes two directions each of a
// Fibonacci sphere: the sky (single and multiple scattering) and the lit
// ground below the horizon, through the air, for a unit sun; projected
// on L2 harmonics, cosine-convolved, divided by pi. It depends on the
// medium alone.
var<workgroup> table_partial: array<array<vec3<f32>, 6>, 64>;

@compute @workgroup_size(64)
fn sky_table_kernel(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let entry = wg.x;
    let p = frame.params;
    let ir = entry / SKY_TABLE_MU;
    let im = entry % SKY_TABLE_MU;
    let height = pow(f32(ir) / f32(SKY_TABLE_R - 1u), 2.0);
    let r = p.bottom_radius + max(height * (p.top_radius - p.bottom_radius), 0.01);
    let mu_s = SKY_TABLE_MU_LO + (1.0 - SKY_TABLE_MU_LO) * f32(im) / f32(SKY_TABLE_MU - 1u);
    let sun = vec3<f32>(sqrt(max(1.0 - mu_s * mu_s, 0.0)), mu_s, 0.0);
    let o = vec3<f32>(0.0, r, 0.0);
    let total = 128u;
    var sh = array<vec3<f32>, 6>(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0));
    if atmosphere_active() {
        for (var k = 0u; k < 2u; k++) {
            let n = li * 2u + k;
            let zc = 1.0 - (f32(n) + 0.5) * 2.0 / f32(total);
            let a = f32(n) * 2.39996323;
            let q = sqrt(max(1.0 - zc * zc, 0.0));
            // y is up: the sphere's axis.
            let d = vec3<f32>(q * cos(a), zc, q * sin(a));
            let segment = atmosphere_segment(p, o, d, true);
            var l = vec3<f32>(0.0);
            if segment.y > segment.x {
                let sc = atmosphere_integrate(p, sun, transmittance_lut, multi_scattering_lut, lut_sampler, o, d, segment.x, segment.y, 12.0, true);
                l = sc.radiance;
                if segment.z > 0.5 {
                    let x = o + d * segment.y;
                    let g = normalize(x);
                    let mu = dot(g, sun);
                    l += sc.transmittance * sun_transmittance(p.bottom_radius, mu) * max(mu, 0.0) * p.ground_albedo / ATMOSPHERE_PI;
                }
            }
            sh[0] += l * 0.282095;
            sh[1] += l * 0.488603 * d.y;
            sh[2] += l * 0.488603 * d.x;
            sh[3] += l * 1.092548 * d.x * d.y;
            sh[4] += l * 0.315392 * (3.0 * d.z * d.z - 1.0);
            sh[5] += l * 0.546274 * (d.x * d.x - d.y * d.y);
        }
    }
    table_partial[li] = sh;
    workgroupBarrier();
    if li != 0u { return; }
    let weight = 4.0 * ATMOSPHERE_PI / f32(total);
    let band = array<f32, 6>(1.0, 2.0 / 3.0, 2.0 / 3.0, 0.25, 0.25, 0.25);
    for (var c = 0u; c < 6u; c++) {
        var sum = vec3<f32>(0.0);
        for (var t = 0u; t < 64u; t++) { sum += table_partial[t][c]; }
        sky_table_out[entry * SKY_TABLE_COEFFS + c] = vec4<f32>(sum * weight * band[c], 0.0);
    }
}
