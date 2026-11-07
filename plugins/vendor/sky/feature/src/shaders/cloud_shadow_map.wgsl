//!use atmosphere
//!use atmosphere_clouds
//!use sun_transmittance
// The clouds' shadow map (`cloud_shadow.wgsl`), built each frame after the
// atmosphere resolves: `cloud_shadow_resolve` places its cascades round the
// camera (their texels on lattices fixed to the planet, so shadows hold
// still while the camera moves), `cloud_shadow_kernel` follows each
// texel's line through the layer towards the sun: the near cascade marches
// the billows, the far one (texels too large for them) takes the share of
// the sun the weather's cover lets through.

@group(0) @binding(0) var<storage, read> frame: AtmosphereFrame;
@group(0) @binding(1) var cloud_weather: texture_3d<f32>;
@group(0) @binding(2) var cloud_shape: texture_3d<f32>;
@group(0) @binding(3) var cloud_detail: texture_3d<f32>;
@group(0) @binding(4) var cloud_noise_sampler: sampler;
@group(0) @binding(5) var shadow_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(6) var<storage, read_write> sun_transmittance_frame: SunTransmittanceFrame;
// Declared for the shared lookup, which this module never calls.
@group(0) @binding(7) var sun_transmittance_map: texture_2d<f32>;
@group(0) @binding(8) var cloud_planet_weather: texture_cube<f32>;

// Half the near cascade's extent (km), at most the layer's radius.
const CLOUD_SHADOW_HALF_KM: f32 = 16.0;
// The far cascade reaches at least this many times as far.
const CLOUD_SHADOW_FAR_MIN: f32 = 4.0;
// The longest path through the layer a texel marches (km): a low sun's
// grazing lines are cut to it.
const CLOUD_SHADOW_MAX_PATH_KM: f32 = 40.0;
const CLOUD_SHADOW_STEPS: u32 = 40u;
// Stored where a line meets no cloud: its clouds begin past any point.
const CLOUD_SHADOW_NONE: f32 = 60000.0;
// A far texel's clouds: their mean density (of the full extinction) where
// the weather covers the sky.
const CLOUD_SHADOW_FAR_DENSITY: f32 = 0.25;

// The texel corner nearest the camera on a lattice of `texel` km along the
// map's axes, relative to the camera.
fn lattice_centre(eye: vec3<f32>, right: vec3<f32>, up: vec3<f32>, texel: f32) -> vec3<f32> {
    let px = dot(eye, right);
    let py = dot(eye, up);
    return right * (round(px / texel) * texel - px) + up * (round(py / texel) * texel - py);
}

@compute @workgroup_size(1)
fn cloud_shadow_resolve() {
    if !atmosphere_has_clouds(frame) {
        sun_transmittance_frame.right = vec4<f32>(0.0);
        return;
    }
    let c = cloud_layer_params(frame);
    let forward = -frame.sun.xyz;
    let reference = select(vec3<f32>(0.0, 0.0, 1.0), vec3<f32>(1.0, 0.0, 0.0), abs(forward.z) > 0.9);
    let right = normalize(cross(reference, forward));
    let up = cross(forward, right);
    let side = f32(textureDimensions(shadow_out).y);
    let eye = frame.eye.xyz;
    let near = min(CLOUD_SHADOW_HALF_KM, c.outer);
    // The far cascade reaches the horizon of the layer's top seen from the
    // eye, in steps of a factor of two's square root (each step a lattice
    // of its own).
    let r = length(eye);
    let horizon = sqrt(max(r * r - c.planet_radius * c.planet_radius, 0.0))
        + sqrt(max(c.outer * c.outer - c.planet_radius * c.planet_radius, 0.0));
    let largest = max(c.outer, near);
    let reach = min(max(horizon, near * CLOUD_SHADOW_FAR_MIN), largest);
    let far = min(exp2(ceil(log2(reach) * 2.0) * 0.5), largest);
    sun_transmittance_frame.right = vec4<f32>(right, 1.0);
    sun_transmittance_frame.up = vec4<f32>(up, 0.0);
    sun_transmittance_frame.forward = vec4<f32>(forward, side);
    sun_transmittance_frame.near = vec4<f32>(lattice_centre(eye, right, up, 2.0 * near / side), near);
    sun_transmittance_frame.far = vec4<f32>(lattice_centre(eye, right, up, 2.0 * far / side), far);
}

// Whether the line through `o` along `d` meets the sphere of radius `r`.
fn line_meets(o: vec3<f32>, d: vec3<f32>, r: f32) -> bool {
    let b = dot(o, d);
    return dot(o, o) - b * b < r * r;
}

// The cirrus' optical depth along the sun's line from `o` (texels of
// `texel` km).
fn cirrus_shadow(c: CloudLayerParams, o: vec3<f32>, sun: vec3<f32>, texel: f32) -> f32 {
    if c.cirrus <= 0.0 { return 0.0; }
    let sheet = ray_sphere(o, sun, c.cirrus_radius);
    if sheet.y <= 0.0 { return 0.0; }
    let x = o + sun * sheet.y;
    return cirrus_depth(c, x, texel) / max(dot(sun, normalize(x)), 0.08);
}

@compute @workgroup_size(8, 8)
fn cloud_shadow_kernel(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(shadow_out);
    if any(id.xy >= size) || sun_transmittance_frame.right.w < 0.5 { return; }
    let c = cloud_layer_params(frame);
    let side = size.y;
    let is_far = id.x >= side;
    let cascade = select(sun_transmittance_frame.near, sun_transmittance_frame.far, is_far);
    let half = cascade.w;
    let texel = 2.0 * half / f32(side);
    let uv = (vec2<f32>(vec2<u32>(id.x % side, id.y)) + 0.5) / f32(side);
    let q = cascade.xyz
        + sun_transmittance_frame.right.xyz * ((uv.x * 2.0 - 1.0) * half)
        + sun_transmittance_frame.up.xyz * ((1.0 - uv.y * 2.0) * half);
    let o = frame.eye.xyz + q;
    let sun = frame.sun.xyz;
    let none = vec4<f32>(CLOUD_SHADOW_NONE, 0.0, 0.0, 0.0);
    if !line_meets(o, sun, c.outer) {
        textureStore(shadow_out, id.xy, none);
        return;
    }
    let cirrus = cirrus_shadow(c, o, sun, texel);
    // Distances along the line towards the sun from `q`: the layer's span
    // between the sun and the near side of the planet.
    let outer = ray_sphere(o, sun, c.outer);
    var lo = outer.x;
    let hi = outer.y;
    if line_meets(o, sun, c.inner) { lo = max(lo, ray_sphere(o, sun, c.inner).y); }
    lo = max(lo, hi - CLOUD_SHADOW_MAX_PATH_KM);
    if hi <= lo || !c.covered {
        textureStore(shadow_out, id.xy, vec4<f32>(none.xyz, cirrus));
        return;
    }
    if is_far {
        // Clouds over a texel this large are the weather's cover of it,
        // nearly opaque where they are.
        let mid = o + sun * (0.5 * (lo + hi));
        let weather = cloud_weather_at(c, mid * (c.inner / length(mid)), texel);
        let inside = c.extinction * (hi - lo) * CLOUD_SHADOW_FAR_DENSITY * mix(0.4, 0.7, weather.y);
        let od = -log(max(1.0 - weather.x * (1.0 - exp(-inside)), 1e-4));
        if od <= 1e-4 {
            textureStore(shadow_out, id.xy, vec4<f32>(none.xyz, cirrus));
            return;
        }
        textureStore(shadow_out, id.xy, vec4<f32>(-hi, od / (hi - lo), od, cirrus));
        return;
    }
    let dt = (hi - lo) / f32(CLOUD_SHADOW_STEPS);
    var od = 0.0;
    var found = false;
    var first = 0.0;
    var last = 0.0;
    for (var i = 0u; i < CLOUD_SHADOW_STEPS; i++) {
        let s = hi - (f32(i) + 0.5) * dt;
        // The wisps too: without their erosion the shadows outgrow the clouds.
        let sigma = cloud_density(c, o + sun * s, texel, true).x * c.extinction;
        if sigma > 0.0 {
            if !found { first = s + 0.5 * dt; found = true; }
            last = s - 0.5 * dt;
            od += sigma * dt;
        }
    }
    if !found {
        textureStore(shadow_out, id.xy, vec4<f32>(none.xyz, cirrus));
        return;
    }
    // Along the light, from the map's plane: z = -s.
    let front = -first;
    let back = -last;
    textureStore(shadow_out, id.xy, vec4<f32>(front, od / max(back - front, dt), od, cirrus));
}
