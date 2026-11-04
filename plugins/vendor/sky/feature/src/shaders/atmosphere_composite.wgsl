//!use helio_prelude
//!use atmosphere
// Atmosphere composite over the lit scene (`pre_aa`), blended as
// dst * alpha + src: sky pixels (no geometry) are replaced by the sky and the
// sun's disk (alpha 0); geometry keeps its light attenuated by the air in
// front of it plus the light the air scatters towards the eye (aerial
// perspective, alpha = transmittance). Without an atmosphere it writes
// (0, 1): the scene is unchanged.
//
// The clouds (`cloud_trace.wgsl`, half resolution) lie in front of
// whatever is behind them: the sky, or geometry farther than their front.
// With the air in front of them already applied, the pixel becomes
// behind * (1 - opacity) + clouds.
//
// Inside the atmosphere the sky comes from the sky-view LUT and the air in
// front of geometry from the aerial-perspective volume. From space both
// would spend their resolution on empty space around a thin shell (and the
// volume would not reach the planet), so each pixel marches its own ray
// through the shell instead.

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<storage, read> frame: AtmosphereFrame;
@group(0) @binding(2) var transmittance_lut: texture_2d<f32>;
@group(0) @binding(3) var sky_view_lut: texture_2d<f32>;
@group(0) @binding(4) var aerial_lut: texture_3d<f32>;
@group(0) @binding(5) var depth_texture: texture_depth_2d;
@group(0) @binding(6) var lut_sampler: sampler;
@group(0) @binding(7) var multi_scattering_lut: texture_2d<f32>;
@group(0) @binding(8) var cloud_color: texture_2d<f32>;
@group(0) @binding(9) var cloud_depth: texture_2d<f32>;

// A cloud footprint reaching the sky (`cloud_trace.wgsl`), km.
const CLOUD_UNLIMITED: f32 = 60000.0;

const SPACE_STEPS: f32 = 32.0;

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) v: u32) -> VertexOut {
    let p = vec2<f32>(f32((v << 1u) & 2u), f32(v & 2u));
    var out: VertexOut;
    out.position = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(p.x, 1.0 - p.y);
    return out;
}

fn transmittance(r: f32, mu: f32) -> vec3<f32> {
    return textureSampleLevel(transmittance_lut, lut_sampler, transmittance_uv(frame.params, r, mu), 0.0).rgb;
}

// The air along `d` from the camera, marched per pixel: up to geometry
// `limit` km away, or (sky pixels, `limit` infinite) to the analytic ground,
// whose sunlit albedo it then includes.
fn march_from_space(eye: vec3<f32>, d: vec3<f32>, limit: f32) -> Scattering {
    let p = frame.params;
    let segment = atmosphere_segment(p, eye, d, limit > 1.0e37);
    let end = min(segment.y, limit);
    var out: Scattering;
    out.radiance = vec3<f32>(0.0);
    out.transmittance = vec3<f32>(1.0);
    if end <= segment.x { return out; }
    out = atmosphere_integrate(p, frame.sun.xyz, transmittance_lut, multi_scattering_lut, lut_sampler,
        eye, d, segment.x, end, SPACE_STEPS, false);
    if segment.z > 0.5 && limit >= segment.y {
        let n = normalize(eye + d * segment.y);
        let mu_s = dot(n, frame.sun.xyz);
        out.radiance += out.transmittance
            * atmosphere_sun_transmittance_lut(p, transmittance_lut, lut_sampler, p.bottom_radius, mu_s)
            * max(mu_s, 0.0) * p.ground_albedo / ATMOSPHERE_PI;
    }
    return out;
}

// The clouds in front of a pixel at `uv` whose geometry is `distance` km
// away (CLOUD_UNLIMITED: sky): rgb their light, a their opacity. Of the four
// half-resolution texels around it, those whose footprint reached at least
// as far count (the others stopped at nearer geometry and saw less sky);
// a texel's clouds count only where their front is nearer than the
// geometry.
fn clouds_in_front(uv: vec2<f32>, distance: f32) -> vec4<f32> {
    if !atmosphere_has_clouds(frame) { return vec4<f32>(0.0); }
    let size = vec2<i32>(textureDimensions(cloud_color));
    let at = uv * vec2<f32>(size) - 0.5;
    let base = vec2<i32>(floor(at));
    let f = at - floor(at);
    // Geometry within this of the clouds' front is partly in front of them:
    // a sliver of the layer's thickness, so terrain under a cloud seen from
    // orbit (2 km below it, 400 km away) stays behind it.
    let thickness = frame.params.cloud_top - frame.params.cloud_bottom;
    var matched = vec4<f32>(0.0);
    var matched_weight = 0.0;
    var unmatched = vec4<f32>(0.0);
    for (var k = 0; k < 4; k++) {
        let o = vec2<i32>(k & 1, k >> 1);
        let q = clamp(base + o, vec2<i32>(0), size - 1);
        let weight = select(1.0 - f.x, f.x, o.x == 1) * select(1.0 - f.y, f.y, o.y == 1);
        let depth = textureLoad(cloud_depth, q, 0);
        var clouds = textureLoad(cloud_color, q, 0);
        if distance < CLOUD_UNLIMITED * 0.98 {
            let band = clamp(depth.x * 0.01, 0.005, 0.25 * thickness);
            clouds *= clamp((distance - depth.x) / band, 0.0, 1.0);
        }
        unmatched += clouds * weight;
        if depth.y >= min(distance, CLOUD_UNLIMITED) * 0.98 {
            matched += clouds * weight;
            matched_weight += weight;
        }
    }
    if matched_weight > 1e-3 { return matched / matched_weight; }
    return unmatched;
}

@fragment
fn fs_composite(in: VertexOut) -> @location(0) vec4<f32> {
    if frame.planet.w < 0.5 { return vec4<f32>(0.0, 0.0, 0.0, 1.0); }
    let camera = cameras[0];
    let pixel = vec2<i32>(in.position.xy);
    let depth = textureLoad(depth_texture, pixel, 0);
    let ndc = vec2<f32>(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0);
    let ray = atmosphere_view_ray(camera.view, camera.proj, ndc);
    let d = normalize(ray);
    let sun = frame.sun_illuminance.rgb;
    let eye = frame.eye.xyz;
    let r = length(eye);
    let in_space = r > frame.params.top_radius;
    if helio_is_sky(depth) {
        var radiance: vec3<f32>;
        // The air between the eye and the sun's disk along `d`.
        var through = vec3<f32>(1.0);
        if in_space {
            let air = march_from_space(eye, d, 3.0e38);
            radiance = air.radiance * sun;
            through = air.transmittance;
        } else {
            through = transmittance(r, dot(eye / r, d));
            let angles = sky_view_direction_angles(frame, d);
            radiance = textureSampleLevel(sky_view_lut, lut_sampler,
                sky_view_uv(frame.params, r, angles.x, angles.y), 0.0).rgb * sun;
        }
        // The sun's disk, through the air in front of it, unless the planet
        // hides it.
        let cos_sun = dot(d, frame.sun.xyz);
        if cos_sun > frame.sun.w && ray_sphere(eye, d, frame.params.bottom_radius).x < 0.0 {
            let solid_angle = 2.0 * ATMOSPHERE_PI * (1.0 - frame.sun.w);
            let edge = clamp((cos_sun - frame.sun.w) / max(1.0 - frame.sun.w, 1e-7), 0.0, 1.0);
            let limb = 0.4 + 0.6 * sqrt(edge);
            radiance += sun / solid_angle * limb * through;
        }
        let clouds = clouds_in_front(in.uv, CLOUD_UNLIMITED);
        return vec4<f32>(radiance * (1.0 - clouds.a) + clouds.rgb, 0.0);
    }
    let distance = length(ray) * helio_view_depth(camera.proj, depth) * 0.001;
    let clouds = clouds_in_front(in.uv, distance);
    let clear = 1.0 - clouds.a;
    if in_space {
        let air = march_from_space(eye, d, distance);
        return vec4<f32>(air.radiance * sun * clear + clouds.rgb, dot(air.transmittance, vec3<f32>(1.0 / 3.0)) * clear);
    }
    let range = frame.sun_illuminance.w;
    // Slices hold values at their centres; the first fades in from the eye.
    let slice = aerial_slice(distance, range) * AERIAL_SLICES - 0.5;
    var aerial = textureSampleLevel(aerial_lut, lut_sampler,
        vec3<f32>(in.uv, (max(slice, 0.0) + 0.5) / AERIAL_SLICES), 0.0);
    if slice < 0.0 {
        aerial = mix(vec4<f32>(0.0, 0.0, 0.0, 1.0), aerial, clamp(slice + 1.0, 0.0, 1.0));
    }
    return vec4<f32>(aerial.rgb * sun * clear + clouds.rgb, aerial.a * clear);
}
