//!use helio_prelude
//!use atmosphere
//!use atmosphere_clouds
//!use atmosphere_sky_table
//!use sun_transmittance
// The clouds seen from the camera (`clouds` module docs), at half the
// lighting target's resolution, a quarter of its pixels traced each frame.
//
// `trace` marches one pixel of each 2x2 block of half-resolution pixels (a
// different one each frame, in a 4-frame cycle) through the layer, up to
// the farthest geometry under the pixel, and crosses the cirrus sheet: the
// light the clouds scatter towards the eye, through the air in front of
// them (aerial perspective), and their transmittance. `reconstruct` fills every half-resolution pixel:
// the traced ones blend into their reprojected history, the others keep it,
// clamped to the traced neighbourhood (or take the trace's interpolation
// where there is no history).
//
// `sun_visibility` (full resolution) is the sun's visibility at each
// pixel's surface for deferred lighting: the terrain's
// (`directional_visibility`, where a voxel pass publishes it) times the
// clouds' shadow, with the sun's direction (yzw) it holds for.
//
// Outputs (half resolution): colour (rgb radiance, a opacity: one minus
// the transmittance, so zero is clear sky) and
// depth (Rg32Float, full precision for planets seen from orbit; r: distance
// to the clouds' front, the transmittance-weighted mean depth, km; g: the
// geometry the pixel's footprint ends at, km, or CLOUD_UNLIMITED where it
// reaches the sky), which the composite reads.

struct TraceParams {
    full: vec2<u32>,
    half: vec2<u32>,
    quarter: vec2<u32>,
    // The half-resolution pixel of each 2x2 block traced this frame.
    sub: vec2<u32>,
    frame: u32,
    // Non-zero when last frame's output is this frame's history.
    history: u32,
    _pad: vec2<u32>,
}

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<storage, read> frame: AtmosphereFrame;
@group(0) @binding(2) var<storage, read> atmosphere_sky_table: array<vec4<f32>>;
@group(0) @binding(3) var transmittance_lut: texture_2d<f32>;
@group(0) @binding(4) var multi_scattering_lut: texture_2d<f32>;
@group(0) @binding(5) var aerial_lut: texture_3d<f32>;
@group(0) @binding(6) var depth_texture: texture_depth_2d;
@group(0) @binding(7) var cloud_weather: texture_3d<f32>;
@group(0) @binding(8) var cloud_shape: texture_3d<f32>;
@group(0) @binding(9) var cloud_detail: texture_3d<f32>;
@group(0) @binding(10) var cloud_noise_sampler: sampler;
@group(0) @binding(11) var lut_sampler: sampler;
@group(0) @binding(12) var<uniform> params: TraceParams;
@group(0) @binding(13) var trace_color_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(14) var trace_depth_out: texture_storage_2d<rg32float, write>;
@group(0) @binding(15) var trace_color: texture_2d<f32>;
@group(0) @binding(16) var trace_depth: texture_2d<f32>;
@group(0) @binding(17) var history_color: texture_2d<f32>;
@group(0) @binding(18) var history_depth: texture_2d<f32>;
@group(0) @binding(19) var color_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(20) var depth_out: texture_storage_2d<rg32float, write>;
@group(0) @binding(21) var<uniform> sun_transmittance_frame: SunTransmittanceFrame;
@group(0) @binding(22) var sun_transmittance_map: texture_2d<f32>;
@group(0) @binding(23) var directional_visibility: texture_2d<f32>;
@group(0) @binding(24) var sun_visibility_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(25) var cloud_planet_weather: texture_cube<f32>;

// Stored as the limit of a footprint that reaches the sky (km).
const CLOUD_UNLIMITED: f32 = 60000.0;
// Primary steps: at least, at most, and per layer thickness.
const CLOUD_MIN_STEPS: f32 = 24.0;
const CLOUD_MAX_STEPS: f32 = 96.0;
const CLOUD_STEPS_PER_THICKNESS: f32 = 40.0;
// The farthest span marched (km): grazing rays along the layer stop there.
const CLOUD_MAX_SPAN_THICKNESSES: f32 = 40.0;
// Steps towards the sun, the first (in layer thicknesses) and the growth of
// their lengths; footprints wider than CLOUD_LIGHT_COARSE thicknesses take
// the coarse set, as far through the layer in a third of the steps.
const CLOUD_LIGHT_STEPS: u32 = 6u;
const CLOUD_LIGHT_FIRST: f32 = 0.05;
const CLOUD_LIGHT_GROWTH: f32 = 1.8;
const CLOUD_LIGHT_COARSE: f32 = 0.06;
const CLOUD_LIGHT_COARSE_STEPS: u32 = 2u;
const CLOUD_LIGHT_COARSE_FIRST: f32 = 0.25;
const CLOUD_LIGHT_COARSE_GROWTH: f32 = 3.0;
// Steps at least, where the noise under a pixel is resolved by fewer.
const CLOUD_FEWEST_STEPS: f32 = 6.0;
// Empty space: where the weather covers less than this share of the sky
// under a pixel, the march looks CLOUD_LEAP_STEPS steps wide, and leaps
// half of that where the cover there is as thin.
const CLOUD_LEAP_COVER: f32 = 1e-3;
const CLOUD_LEAP_STEPS: f32 = 16.0;
// Primary steps sharing one lookup of the weather.
const CLOUD_WEATHER_REUSE: f32 = 3.0;
// How much of the history a traced pixel keeps: each pixel is traced every
// fourth frame, so it averages about 4 / (1 - weight) frames.
const CLOUD_HISTORY_WEIGHT: f32 = 0.85;
// How far past the traced neighbourhood's range the history may lie: the
// neighbourhood's traces sit at another sub-pixel each frame, and a tight
// clamp would make the history follow their noise.
const CLOUD_CLAMP_SLACK: f32 = 0.5;
// Steps of the air in front of the clouds from space.
const CLOUD_SPACE_AIR_STEPS: f32 = 12.0;

fn interleaved_gradient(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// The farthest depth under half-resolution pixel `p` (its 2x2 full-
// resolution pixels).
fn footprint_depth(p: vec2<u32>) -> f32 {
    let base = vec2<i32>(p * 2u);
    let last = vec2<i32>(params.full) - 1;
    var farthest = 1.0;
    for (var k = 0; k < 4; k++) {
        let d = textureLoad(depth_texture, min(base + vec2<i32>(k & 1, k >> 1), last), 0);
        farthest = helio_depth_farthest(farthest, d);
    }
    return farthest;
}

// The view ray (unit, world) through the centre of half-resolution pixel
// `p`, and its length per unit of view depth.
fn pixel_ray(p: vec2<u32>) -> vec4<f32> {
    let uv = (vec2<f32>(p) + 0.5) / vec2<f32>(params.half);
    let ray = atmosphere_view_ray(cameras[0].view, cameras[0].proj, helio_uv_to_ndc(uv));
    let len = length(ray);
    return vec4<f32>(ray / len, len);
}

// The distance (km) geometry under depth `depth` lies along a ray of
// `ray_len` per unit of view depth, or CLOUD_UNLIMITED for sky.
fn geometry_limit(depth: f32, ray_len: f32) -> f32 {
    if helio_is_sky(depth) { return CLOUD_UNLIMITED; }
    return min(ray_len * helio_view_depth(cameras[0].proj, depth) * 0.001, CLOUD_UNLIMITED);
}

// Optical depth towards the sun from `x` (a cloud sample under `weather`,
// for a footprint of `footprint` km): steps growing away from it, the
// wisps only on the first two.
fn light_optical_depth(c: CloudLayerParams, x: vec3<f32>, weather: vec2<f32>, footprint: f32, toward_light: vec3<f32>) -> f32 {
    let coarse = footprint * c.inv_thickness > CLOUD_LIGHT_COARSE;
    let count = select(CLOUD_LIGHT_STEPS, CLOUD_LIGHT_COARSE_STEPS, coarse);
    let growth = select(CLOUD_LIGHT_GROWTH, CLOUD_LIGHT_COARSE_GROWTH, coarse);
    var step = select(CLOUD_LIGHT_FIRST, CLOUD_LIGHT_COARSE_FIRST, coarse) / c.inv_thickness;
    var travelled = 0.0;
    var od = 0.0;
    for (var j = 0u; j < count; j++) {
        let p = x + toward_light * (travelled + 0.5 * step);
        let h = cloud_height(c, p);
        if h >= 1.0 { break; }
        // The weather is large: the sample's own holds along the short path.
        od += cloud_density_under(c, p, h, weather, footprint * exp2(f32(j) * 0.5), j < 2u) * step;
        travelled += step;
        step *= growth;
    }
    return od * c.extinction;
}

// Direct illumination from every directional light row. Atmosphere.resolve
// still chooses one primary sun for the sky LUTs, but cloud scattering is
// summed per light so a moon directional light illuminates the cloud volume
// with its own direction and color/intensity.
fn cloud_directional_radiance(
    c: CloudLayerParams, x: vec3<f32>, weather: vec2<f32>, footprint: f32, view_ray: vec3<f32>,
) -> vec3<f32> {
    var direct = vec3<f32>(0.0);
    let r = length(x);
    for (var i = 0u; i < frame.directional_light_count.x; i++) {
        let base = i * 3u;
        let toward_light = frame.directional_lights[base].xyz;
        let energy = frame.directional_lights[base + 1u].rgb;
        let transmittance = atmosphere_sun_transmittance_lut(
            frame.params, transmittance_lut, lut_sampler, r, dot(x / r, toward_light));
        let od = light_optical_depth(c, x, weather, footprint, toward_light);
        direct += transmittance * energy
            * cloud_sun_scattering(dot(view_ray, toward_light), od);
    }
    return direct;
}

struct CloudTrace {
    color: vec4<f32>,
    front: f32,
}

// The cirrus sheet seen along `d` (unit) from the eye, crossed `t` km away
// with `footprint` km pixels: rgb its radiance, w its transmittance.
fn trace_cirrus(c: CloudLayerParams, d: vec3<f32>, t: f32, footprint: f32) -> vec4<f32> {
    let x = frame.eye.xyz + d * t;
    let depth = cirrus_depth(c, x, footprint);
    if depth <= 0.0 { return vec4<f32>(0.0, 0.0, 0.0, 1.0); }
    let up = normalize(x);
    // Slanted through the sheet, the view and the light cross more of it.
    let od = depth / max(abs(dot(d, up)), 0.08);
    var direct = vec3<f32>(0.0);
    for (var i = 0u; i < frame.directional_light_count.x; i++) {
        let base = i * 3u;
        let toward_light = frame.directional_lights[base].xyz;
        let energy = frame.directional_lights[base + 1u].rgb;
        let od_sun = depth / max(dot(toward_light, up), 0.08);
        let sunlight = atmosphere_sun_transmittance_lut(
            frame.params, transmittance_lut, lut_sampler, length(x), dot(up, toward_light));
        direct += sunlight * energy
            * cloud_sun_scattering(dot(d, toward_light), od_sun);
    }
    let sky = atmosphere_sky_table_eval(frame, d * t, up, vec3<f32>(1.0));
    let source = c.albedo * (direct + 0.5 * sky);
    let opacity = 1.0 - exp(-od);
    return vec4<f32>(source * opacity, 1.0 - opacity);
}

fn trace_clouds(d: vec3<f32>, limit: f32, uv: vec2<f32>, jitter: f32) -> CloudTrace {
    var out: CloudTrace;
    out.color = vec4<f32>(0.0);
    out.front = limit;
    let c = cloud_layer_params(frame);
    let eye = frame.eye.xyz;
    // Radians between traced rays (a quarter-resolution pixel), for the
    // noise's mips: finer noise would alias between them.
    let pixel_angle = 2.0 / (cameras[0].proj[1][1] * f32(params.quarter.y));
    let cirrus_t = cirrus_crossing(c, eye, d);
    var cirrus = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    if c.cirrus > 0.0 && cirrus_t > 0.0 && cirrus_t < limit {
        cirrus = trace_cirrus(c, d, cirrus_t, cirrus_t * pixel_angle);
    }
    let segment = cloud_layer_segment(c, eye, d);
    let thickness = 1.0 / c.inv_thickness;
    let t0 = segment.x;
    let t1 = min(min(segment.y, limit), t0 + CLOUD_MAX_SPAN_THICKNESSES * thickness);
    let marched = c.covered && t1 > t0;
    if !marched && cirrus.w >= 1.0 { return out; }
    let span = max(t1 - t0, 0.0);
    // Voxel cells near enough to see take a step each at most.
    let per_cell = select(0.0, span / max(c.voxel, 1e-6), c.voxel > 0.0 && t0 * pixel_angle < c.voxel);
    // No finer than half the footprint of a pixel where the layer begins:
    // from afar the noise under a pixel is its mean (the mips' spread).
    let resolved = ceil(span / max(0.5 * t0 * pixel_angle, 1e-4));
    let wanted = max(min(ceil(span * c.inv_thickness * CLOUD_STEPS_PER_THICKNESS), resolved), per_cell);
    let fewest = clamp(resolved, CLOUD_FEWEST_STEPS, CLOUD_MIN_STEPS);
    let steps = select(0.0, clamp(wanted, fewest, CLOUD_MAX_STEPS), marched);
    let dt = span / max(steps, 1.0);
    // The sky's light on the layer, from its middle along the ray: above
    // (dimmed towards the base, under the cloud) and below (the ground's).
    let mid = d * (t0 + 0.5 * span);
    let up = normalize(eye + mid);
    let sky_above = atmosphere_sky_table_eval(frame, mid, up, vec3<f32>(1.0));
    let sky_below = atmosphere_sky_table_eval(frame, mid, -up, vec3<f32>(1.0));
    var radiance = vec3<f32>(0.0);
    var transmittance = 1.0;
    var depth_sum = 0.0;
    var weight_sum = 0.0;
    // The weather, looked up every CLOUD_WEATHER_REUSE steps while those
    // steps lie within a texel of its finest noise (it changes over
    // kilometres, short steps over tens of metres); every step otherwise,
    // or long grazing steps band it.
    let reuse = select(1.0, CLOUD_WEATHER_REUSE, dt * CLOUD_WEATHER_REUSE < 1.0 / c.groups_texels);
    var weather = vec2<f32>(0.0);
    var weather_age = reuse;
    for (var i = 0.0; i < steps; i += 1.0) {
        let t = t0 + (i + jitter) * dt;
        let x = cloud_point(c, eye + d * t);
        let footprint = t * pixel_angle;
        let r_x = length(x);
        let h_x = (r_x - c.inner) * c.inv_thickness;
        if h_x <= 0.0 || h_x >= 1.0 { continue; }
        if weather_age >= reuse {
            weather = cloud_weather_at(c, x * (c.inner / r_x), footprint);
            weather_age = 0.0;
        }
        weather_age += 1.0;
        let sample = vec4<f32>(cloud_density_under(c, x, h_x, weather, footprint, true), h_x, weather);
        if sample.x <= 0.0 {
            // Clear weather: leap ahead while it stays clear wider round.
            if sample.z < CLOUD_LEAP_COVER {
                let wide = max(CLOUD_LEAP_STEPS * dt, footprint);
                if cloud_weather_at(c, x * (c.inner / length(x)), wide).x < CLOUD_LEAP_COVER {
                    i += max(floor(0.5 * wide / dt) - 1.0, 0.0);
                    weather_age = reuse;
                }
            }
            continue;
        }
        let sigma = sample.x * c.extinction;
        let direct = cloud_directional_radiance(c, x, sample.zw, footprint, d);
        // Isotropic in-scattering of the sky: half of each hemisphere's
        // irradiance / pi.
        let h = sample.y;
        let ambient = 0.5 * (sky_above * mix(0.3, 1.0, h) + sky_below * mix(1.0, 0.5, h));
        let source = c.albedo * (direct + ambient) * sigma;
        let step_t = exp(-sigma * dt);
        radiance += transmittance * source * (1.0 - step_t) / sigma;
        let weight = transmittance * (1.0 - step_t);
        depth_sum += t * weight;
        weight_sum += weight;
        transmittance *= step_t;
        if transmittance < 0.01 { break; }
    }
    // The cirrus, in front of the layer or behind it.
    if cirrus.w < 1.0 {
        let weight = 1.0 - cirrus.w;
        if cirrus_t < t0 || !marched {
            radiance = cirrus.rgb + cirrus.w * radiance;
            depth_sum = depth_sum * cirrus.w + cirrus_t * weight;
            weight_sum = weight_sum * cirrus.w + weight;
        } else {
            radiance += transmittance * cirrus.rgb;
            depth_sum += transmittance * weight * cirrus_t;
            weight_sum += transmittance * weight;
        }
        transmittance *= cirrus.w;
    }
    if weight_sum <= 0.0 { return out; }
    out.front = depth_sum / weight_sum;
    // The air between the eye and the clouds' front.
    var air_radiance: vec3<f32>;
    var air_transmittance: f32;
    if length(eye) > frame.params.top_radius {
        let air_segment = atmosphere_segment(frame.params, eye, d, false);
        air_radiance = vec3<f32>(0.0);
        air_transmittance = 1.0;
        for (var i = 0u; i < frame.directional_light_count.x; i++) {
            let base = i * 3u;
            let air = atmosphere_integrate(frame.params, frame.directional_lights[base].xyz,
                transmittance_lut, multi_scattering_lut, lut_sampler, eye, d, air_segment.x,
                max(min(out.front, air_segment.y), air_segment.x), CLOUD_SPACE_AIR_STEPS, false);
            air_radiance += air.radiance * frame.directional_lights[base + 1u].rgb;
            if i == 0u {
                air_transmittance = dot(air.transmittance, vec3<f32>(1.0 / 3.0));
            }
        }
    } else {
        let slice = aerial_slice(out.front, frame.sun_illuminance.w) * AERIAL_SLICES - 0.5;
        var aerial = textureSampleLevel(aerial_lut, lut_sampler, vec3<f32>(uv, (max(slice, 0.0) + 0.5) / AERIAL_SLICES), 0.0);
        if slice < 0.0 { aerial = mix(vec4<f32>(0.0, 0.0, 0.0, 1.0), aerial, clamp(slice + 1.0, 0.0, 1.0)); }
        air_radiance = aerial.rgb;
        air_transmittance = aerial.a;
    }
    out.color = vec4<f32>(radiance * air_transmittance + air_radiance * (1.0 - transmittance), 1.0 - transmittance);
    return out;
}

@compute @workgroup_size(8, 8)
fn trace(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= params.quarter) { return; }
    let p = min(id.xy * 2u + params.sub, params.half - 1u);
    let ray = pixel_ray(p);
    let limit = geometry_limit(footprint_depth(p), ray.w);
    var result: CloudTrace;
    result.color = vec4<f32>(0.0);
    result.front = limit;
    if atmosphere_has_clouds(frame) {
        // A different start along the ray each time the pixel is traced.
        let jitter = fract(interleaved_gradient(vec2<f32>(p)) + f32(params.frame >> 2u) * 0.618034);
        let uv = (vec2<f32>(p) + 0.5) / vec2<f32>(params.half);
        result = trace_clouds(ray.xyz, limit, uv, jitter);
    }
    textureStore(trace_color_out, id.xy, result.color);
    textureStore(trace_depth_out, id.xy, vec4<f32>(result.front, limit, 0.0, 0.0));
}

fn load_trace(q: vec2<i32>) -> vec4<f32> {
    return textureLoad(trace_color, clamp(q, vec2<i32>(0), vec2<i32>(params.quarter) - 1), 0);
}

fn load_trace_depth(q: vec2<i32>) -> vec4<f32> {
    return textureLoad(trace_depth, clamp(q, vec2<i32>(0), vec2<i32>(params.quarter) - 1), 0);
}

@compute @workgroup_size(8, 8)
fn reconstruct(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= params.half) { return; }
    let p = id.xy;
    let ray = pixel_ray(p);
    let limit = geometry_limit(footprint_depth(p), ray.w);
    // This frame's traces around the pixel: trace texel b is half-
    // resolution pixel 2b + sub.
    let at = (vec2<f32>(p) - vec2<f32>(params.sub)) * 0.5;
    let base = vec2<i32>(floor(at));
    let f = at - floor(at);
    let c00 = load_trace(base);
    let c10 = load_trace(base + vec2<i32>(1, 0));
    let c01 = load_trace(base + vec2<i32>(0, 1));
    let c11 = load_trace(base + vec2<i32>(1, 1));
    var current = mix(mix(c00, c10, f.x), mix(c01, c11, f.x), f.y);
    // The clouds' front only means something where there are clouds: a
    // trace without any stores its limit (the sky's is CLOUD_UNLIMITED), so
    // fronts average by opacity, never blending that in.
    let d00 = load_trace_depth(base).x;
    let d10 = load_trace_depth(base + vec2<i32>(1, 0)).x;
    let d01 = load_trace_depth(base + vec2<i32>(0, 1)).x;
    let d11 = load_trace_depth(base + vec2<i32>(1, 1)).x;
    let w = vec4<f32>((1.0 - f.x) * (1.0 - f.y), f.x * (1.0 - f.y), (1.0 - f.x) * f.y, f.x * f.y)
        * vec4<f32>(c00.a, c10.a, c01.a, c11.a);
    let opacity_sum = w.x + w.y + w.z + w.w;
    var front = select(limit, dot(w, vec4<f32>(d00, d10, d01, d11)) / opacity_sum, opacity_sum > 1e-4);
    let traced = all(p % 2u == params.sub);
    if traced {
        current = c00;
        front = d00;
    }
    // The traced neighbourhood bounds the history.
    let centre = vec2<i32>(round(at));
    var low = vec4<f32>(1e9);
    var high = vec4<f32>(-1e9);
    for (var k = 0; k < 9; k++) {
        let s = load_trace(centre + vec2<i32>(k % 3 - 1, k / 3 - 1));
        low = min(low, s);
        high = max(high, s);
    }
    // Where the clouds were last frame: their front along this pixel's ray.
    let camera = cameras[0];
    let world = camera.position_near.xyz + ray.xyz * (min(front, 1.0e4) * 1000.0);
    let clip = camera.prev_view_proj * vec4<f32>(world, 1.0);
    let prev_uv = helio_ndc_to_uv(clip.xy / clip.w);
    let valid = params.history != 0u && clip.w > 0.0 && all(prev_uv >= vec2<f32>(0.0)) && all(prev_uv <= vec2<f32>(1.0));
    let slack = (high - low) * CLOUD_CLAMP_SLACK + vec4<f32>(1e-3);
    low -= slack;
    high += slack;
    var color = current;
    var out_front = front;
    if valid {
        let history = clamp(textureSampleLevel(history_color, lut_sampler, prev_uv, 0.0), low, high);
        let history_size = vec2<i32>(textureDimensions(history_depth));
        let history_front = textureLoad(history_depth, clamp(vec2<i32>(prev_uv * vec2<f32>(history_size)), vec2<i32>(0), history_size - 1), 0).x;
        // Fronts blend by the opacity each brings, as above.
        let keep = select(1.0, CLOUD_HISTORY_WEIGHT, traced);
        let mine = select(0.0, (1.0 - CLOUD_HISTORY_WEIGHT) * current.a, traced);
        let theirs = keep * history.a;
        color = select(history, mix(current, history, CLOUD_HISTORY_WEIGHT), traced);
        if mine + theirs > 1e-4 {
            out_front = (mine * front + theirs * history_front) / (mine + theirs);
        }
    }
    if color.a <= 1e-4 { out_front = limit; }
    textureStore(color_out, p, color);
    textureStore(depth_out, p, vec4<f32>(out_front, limit, 0.0, 0.0));
}

// The sun's visibility at each full-resolution pixel (see the header).
@compute @workgroup_size(8, 8)
fn sun_visibility(@builtin(global_invocation_id) id: vec3<u32>) {
    if any(id.xy >= params.full) { return; }
    let pixel = vec2<i32>(id.xy);
    let sun = frame.sun.xyz;
    let terrain_size = vec2<i32>(textureDimensions(directional_visibility));
    var terrain = textureLoad(directional_visibility, min(pixel, terrain_size - 1), 0);
    // Only a terrain visibility for this sun counts.
    if dot(terrain.yzw, terrain.yzw) < 0.5 || dot(normalize(terrain.yzw), sun) < 0.99999 {
        terrain = vec4<f32>(1.0, sun);
    }
    let depth = textureLoad(depth_texture, pixel, 0);
    if helio_is_sky(depth) {
        textureStore(sun_visibility_out, pixel, terrain);
        return;
    }
    let uv = (vec2<f32>(id.xy) + 0.5) / vec2<f32>(params.full);
    let ray = atmosphere_view_ray(cameras[0].view, cameras[0].proj, helio_uv_to_ndc(uv));
    let p = ray * helio_view_depth(cameras[0].proj, depth);
    textureStore(sun_visibility_out, pixel, vec4<f32>(terrain.x * sun_transmittance_at(p), terrain.yzw));
}
