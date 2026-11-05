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
@group(0) @binding(10) var moon_albedo: texture_2d<f32>;

const SOLID_EMITTER_DISK: u32 = 253u;
const TEXTURED_EMITTER_DISK: u32 = 254u;

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
    for (var i = 0u; i < frame.directional_light_count.x; i++) {
        let base = i * 3u;
        let light_dir = frame.directional_lights[base].xyz;
        let energy = frame.directional_lights[base + 1u].rgb;
        let s = atmosphere_integrate(p, light_dir, transmittance_lut, multi_scattering_lut, lut_sampler,
            eye, d, segment.x, end, SPACE_STEPS, false);
        out.radiance += s.radiance * energy;
        if i == 0u { out.transmittance = s.transmittance; }
        if segment.z > 0.5 && limit >= segment.y {
            let n = normalize(eye + d * segment.y);
            let mu_s = dot(n, light_dir);
            out.radiance += s.transmittance * energy
                * atmosphere_sun_transmittance_lut(p, transmittance_lut, lut_sampler, p.bottom_radius, mu_s)
                * max(mu_s, 0.0) * p.ground_albedo / ATMOSPHERE_PI;
        }
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

fn sky_is_night() -> bool {
    if frame.params.night_sky_enabled == 0u { return false; }
    for (var i = 0u; i < frame.directional_light_count.x; i++) {
        if frame.directional_lights[i * 3u].y >= 0.0 { return false; }
    }
    return true;
}

fn directional_disk_radiance(d: vec3<f32>, through: vec3<f32>) -> vec3<f32> {
    var radiance = vec3<f32>(0.0);
    let radius = max(frame.params.sun_angular_radius, 1e-5);
    let solid_angle = max(2.0 * ATMOSPHERE_PI * (1.0 - cos(radius)), 1e-8);
    for (var i = 0u; i < frame.directional_light_count.x; i++) {
        let base = i * 3u;
        let emitter = frame.directional_lights[base + 2u];
        let kind = u32(emitter.z);
        if emitter.w > 0.5 && (kind == SOLID_EMITTER_DISK || kind == TEXTURED_EMITTER_DISK) {
            continue;
        }
        let light_dir = frame.directional_lights[base].xyz;
        if dot(d, light_dir) <= cos(radius) { continue; }
        let energy = frame.directional_lights[base + 1u].rgb;
        let edge = clamp((dot(d, light_dir) - cos(radius)) / max(1.0 - cos(radius), 1e-7), 0.0, 1.0);
        radiance += energy / solid_angle * (0.4 + 0.6 * sqrt(edge)) * through;
    }
    return radiance;
}

fn hash2(p: vec2<f32>, seed: u32) -> vec3<f32> {
    let h = vec3<f32>(
        dot(p, vec2<f32>(127.1, 311.7)) + f32(seed) * 0.013,
        dot(p, vec2<f32>(269.5, 183.3)) + f32(seed) * 0.017,
        dot(p, vec2<f32>(419.2, 371.9)) + f32(seed) * 0.019);
    return fract(sin(h) * 43758.5453);
}

fn night_radiance(d: vec3<f32>) -> vec3<f32> {
    let p = frame.params;
    if p.night_sky_enabled == 0u || dot(d, vec3<f32>(0.0, 1.0, 0.0)) < -0.02 {
        return vec3<f32>(0.0);
    }
    let count = clamp(p.star_count, 0.0, 20000.0);
    let gx = max(sqrt(max(count, 1.0) * 12.56637), 1.0);
    let gy = gx * 0.5;
    let lon = atan2(d.x, d.z) / (2.0 * ATMOSPHERE_PI) + 0.5;
    let lat = asin(clamp(d.y, -1.0, 1.0)) / ATMOSPHERE_PI + 0.5;
    let cell = vec2<f32>(gx, gy);
    let q = vec2<f32>(lon, lat) * cell;
    let base = floor(q);
    let cell_uv = fract(q);
    var result = vec3<f32>(0.0);
    let occupancy = clamp(count / max(gx * gy, 1.0), 0.0, 1.0);
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let id = base + vec2<f32>(f32(x), f32(y));
            let h = hash2(id, p.star_seed);
            if h.z > occupancy { continue; }
            let jitter = h.xy;
            let delta = (vec2<f32>(f32(x), f32(y)) + jitter - cell_uv) / cell;
            let angular = length(vec2<f32>(delta.x * 6.2831853 * cos((lat - 0.5) * 3.14159265), delta.y * 3.14159265));
            let radius = mix(0.00016, 0.00072, pow(h.y, 3.0));
            if angular >= radius { continue; }
            let twinkle = select(1.0, 0.58 + 0.42 * sin(frame.cloud_meso_offset.w * p.star_twinkle_speed * (1.5 + h.x * 3.0) + h.z * 6.2831853), h.x < p.star_twinkle_fraction);
            let core = 1.0 - smoothstep(radius * 0.12, radius, angular);
            let halo = exp(-angular * angular / max(radius * radius * 4.0, 1e-9)) * 0.22;
            let temperature = mix(vec3<f32>(0.62, 0.76, 1.0), vec3<f32>(1.0, 0.82, 0.62), h.z);
            result += temperature * (core + halo) * twinkle * p.star_brightness * mix(0.35, 1.0, h.y);
        }
    }
    if p.shooting_stars_enabled != 0u && p.shooting_star_rate > 0.0 {
        let period = 60.0 / max(p.shooting_star_rate, 0.001);
        let slot = floor(frame.cloud_meso_offset.w / period);
        let age = frame.cloud_meso_offset.w - slot * period;
        let event = hash2(vec2<f32>(slot, 31.7), p.star_seed + 991u);
        let start_lon = event.x * 6.2831853;
        let start_lat = (event.y - 0.5) * 2.3;
        let start = vec3<f32>(cos(start_lat) * sin(start_lon), sin(start_lat), cos(start_lat) * cos(start_lon));
        let side = normalize(cross(start, vec3<f32>(0.0, 1.0, 0.0)) + vec3<f32>(0.001, 0.0, 0.0));
        let travel = min(p.shooting_star_speed * age * 0.1, p.shooting_star_length);
        let center = normalize(start + side * travel);
        let along = dot(d - center, side);
        let across = length(d - center - side * along);
        let event_live = select(1.0, 0.0, age > 0.6 / max(p.shooting_star_speed, 0.05));
        let trail = smoothstep(-p.shooting_star_length, -p.shooting_star_length * 0.65, along)
            * (1.0 - smoothstep(-0.02, 0.0, along))
            * exp(-across * across / 0.000008) * event_live;
        let head = exp(-dot(d - center, d - center) / 0.000012) * event_live;
        result += vec3<f32>(0.58, 0.76, 1.0) * (trail * 24.0 + head * 70.0);
    }
    return result;
}

fn emitter_disk_radiance(d: vec3<f32>) -> vec3<f32> {
    let p = frame.params;
    var radiance = vec3<f32>(0.0);
    for (var i = 0u; i < frame.directional_light_count.x; i++) {
        let base = i * 3u;
        let emitter = frame.directional_lights[base + 2u];
        let emitter_type = u32(emitter.z);
        if emitter.w < 0.5
            || (emitter_type != SOLID_EMITTER_DISK && emitter_type != TEXTURED_EMITTER_DISK) {
            continue;
        }
        let toward_light = frame.directional_lights[base].xyz;
        let radius = max(emitter.x, 0.0001);
        if dot(d, toward_light) < cos(radius) { continue; }
        let right = normalize(cross(toward_light, vec3<f32>(0.0, 1.0, 0.0)) + vec3<f32>(0.001, 0.0, 0.0));
        let up = normalize(cross(right, toward_light));
        let x = dot(d, right) / sin(radius);
        let y = dot(d, up) / sin(radius);
        let r2 = x * x + y * y;
        if r2 > 1.0 { continue; }
        let z = sqrt(max(1.0 - r2, 0.0));
        let normal = normalize(right * x + up * y + toward_light * z);
        let uv = vec2<f32>(atan2(x, z) / (2.0 * ATMOSPHERE_PI) + 0.5,
            0.5 - asin(clamp(y, -1.0, 1.0)) / ATMOSPHERE_PI);
        let light_energy = frame.directional_lights[base + 1u].rgb;
        let brightness = emitter.y;
        if emitter_type == TEXTURED_EMITTER_DISK {
            let albedo = textureSampleLevel(moon_albedo, lut_sampler, uv, 0.0).rgb;
            // Every other directional source illuminates the visible disk;
            // the emitter light itself controls moonlight on the scene.
            var disk_light = vec3<f32>(0.0);
            for (var j = 0u; j < frame.directional_light_count.x; j++) {
                if i == j { continue; }
                let solar_base = j * 3u;
                let solar_dir = frame.directional_lights[solar_base].xyz;
                let solar_energy = frame.directional_lights[solar_base + 1u].rgb;
                disk_light += solar_energy * max(dot(-normal, solar_dir), 0.08);
            }
            radiance += albedo * disk_light * brightness;
        } else {
            // A textureless emitter remains a visible, scalable solid disk.
            radiance += light_energy * brightness;
        }
    }
    return radiance;
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
    let eye = frame.eye.xyz;
    let r = length(eye);
    let in_space = r > frame.params.top_radius;
    if helio_is_sky(depth) {
        var radiance: vec3<f32>;
        // The air between the eye and the sun's disk along `d`.
        var through = vec3<f32>(1.0);
        if in_space {
            let air = march_from_space(eye, d, 3.0e38);
            radiance = air.radiance;
            through = air.transmittance;
        } else {
            through = transmittance(r, dot(eye / r, d));
            let angles = sky_view_direction_angles(frame, d);
            radiance = textureSampleLevel(sky_view_lut, lut_sampler,
                sky_view_uv(frame.params, r, angles.x, angles.y), 0.0).rgb;
        }
        // Every untextured directional emitter gets the atmosphere's normal
        // angular radius; textured or enlarged emitters use their own size.
        if ray_sphere(eye, d, frame.params.bottom_radius).x < 0.0 {
            radiance += directional_disk_radiance(d, through);
            radiance += emitter_disk_radiance(d) * through;
        }
        if sky_is_night() {
            radiance += night_radiance(d);
        }
        let clouds = clouds_in_front(in.uv, CLOUD_UNLIMITED);
        return vec4<f32>(radiance * (1.0 - clouds.a) + clouds.rgb, 0.0);
    }
    let distance = length(ray) * helio_view_depth(camera.proj, depth) * 0.001;
    let clouds = clouds_in_front(in.uv, distance);
    let clear = 1.0 - clouds.a;
    if in_space {
        let air = march_from_space(eye, d, distance);
        return vec4<f32>(air.radiance * clear + clouds.rgb, dot(air.transmittance, vec3<f32>(1.0 / 3.0)) * clear);
    }
    let range = frame.sun_illuminance.w;
    // Slices hold values at their centres; the first fades in from the eye.
    let slice = aerial_slice(distance, range) * AERIAL_SLICES - 0.5;
    var aerial = textureSampleLevel(aerial_lut, lut_sampler,
        vec3<f32>(in.uv, (max(slice, 0.0) + 0.5) / AERIAL_SLICES), 0.0);
    if slice < 0.0 {
        aerial = mix(vec4<f32>(0.0, 0.0, 0.0, 1.0), aerial, clamp(slice + 1.0, 0.0, 1.0));
    }
    return vec4<f32>(aerial.rgb * clear + clouds.rgb, aerial.a * clear);
}
