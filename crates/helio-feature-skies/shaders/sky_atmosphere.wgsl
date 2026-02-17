// Helio Skies — Volumetric Atmospheric Sky with Full-Sphere Clouds
//
// Fixes vs. original:
//   • Clouds now visible at all elevations (ray-slab intersection replaces
//     the broken altitude-band check that required looking nearly straight up)
//   • Sun disc rendered AFTER clouds so it is never blown-out by cloud mixing
//   • Proper Rayleigh + Mie gradient sky with day/night/sunset transitions
//   • Animated clouds via camera.time (requires CameraUniforms.time)
//   • Night-sky star field fades in as the sun drops below the horizon

// ───────────────────────────────────────────────────────────────────────────
// Cloud layer constants (world-space altitude, same as material_bindings.wgsl)
// ───────────────────────────────────────────────────────────────────────────
const SKY_DISTANCE_THRESHOLD: f32 = 400.0;
const CLOUD_HEIGHT_MIN: f32       = 200.0;
const CLOUD_HEIGHT_MAX: f32       = 400.0;
const CLOUD_COVERAGE: f32         = 0.58;

// ───────────────────────────────────────────────────────────────────────────
// Noise primitives
// ───────────────────────────────────────────────────────────────────────────
fn sky_hash(p: vec3<f32>) -> f32 {
    var p3 = fract(p * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// Quintic-interpolated 3D value noise — smoother than cubic
fn sky_noise3d(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);

    return mix(
        mix(
            mix(sky_hash(i + vec3<f32>(0.0,0.0,0.0)), sky_hash(i + vec3<f32>(1.0,0.0,0.0)), u.x),
            mix(sky_hash(i + vec3<f32>(0.0,1.0,0.0)), sky_hash(i + vec3<f32>(1.0,1.0,0.0)), u.x),
            u.y
        ),
        mix(
            mix(sky_hash(i + vec3<f32>(0.0,0.0,1.0)), sky_hash(i + vec3<f32>(1.0,0.0,1.0)), u.x),
            mix(sky_hash(i + vec3<f32>(0.0,1.0,1.0)), sky_hash(i + vec3<f32>(1.0,1.0,1.0)), u.x),
            u.y
        ),
        u.z
    );
}

fn sky_fbm(p: vec3<f32>, octaves: i32) -> f32 {
    var val  = 0.0;
    var amp  = 0.5;
    var freq = 1.0;
    var pos  = p;
    for (var i = 0; i < octaves; i++) {
        val  += amp  * sky_noise3d(pos * freq);
        freq *= 2.1;
        amp  *= 0.5;
    }
    return val;
}

// ───────────────────────────────────────────────────────────────────────────
// Cloud density at a world-space position
// ───────────────────────────────────────────────────────────────────────────
fn sky_cloud_density(world_pos: vec3<f32>, time: f32) -> f32 {
    let thickness   = CLOUD_HEIGHT_MAX - CLOUD_HEIGHT_MIN;
    let height_frac = (world_pos.y - CLOUD_HEIGHT_MIN) / thickness;
    if (height_frac < 0.0 || height_frac > 1.0) { return 0.0; }

    // Puffy lower two-thirds, wispy top
    let h_fade   = smoothstep(0.0, 0.12, height_frac) * smoothstep(1.0, 0.55, height_frac);

    let spd      = vec3<f32>(0.8, 0.0, 0.45);
    let anim_pos = world_pos * 0.0025 + spd * time * 0.00025;

    let base     = sky_fbm(anim_pos, 4);
    let thresh   = 1.0 - CLOUD_COVERAGE;
    let raw      = max(0.0, base - thresh) / CLOUD_COVERAGE;
    if (raw < 0.001) { return 0.0; }

    // Fluffy-edge detail erosion
    let detail = sky_fbm(anim_pos * 2.7 + vec3<f32>(0.1, 0.2, 0.3), 3) * 0.35;
    let shaped = max(0.0, raw - detail * (1.0 - raw));

    return clamp(shaped * h_fade, 0.0, 1.0);
}

// Integrate cloud density along the view ray through the cloud slab (3 taps)
fn sky_cloud_coverage(view_dir: vec3<f32>, camera_pos: vec3<f32>, time: f32) -> f32 {
    if (view_dir.y < 0.02) { return 0.0; }

    let t0      = (CLOUD_HEIGHT_MIN - camera_pos.y) / view_dir.y;
    let t1      = (CLOUD_HEIGHT_MAX - camera_pos.y) / view_dir.y;
    if (t0 < 0.0 && t1 < 0.0) { return 0.0; }

    let t_enter = max(0.0, t0);
    let step    = (t1 - t_enter) / 3.0;

    var total = 0.0;
    for (var i = 0; i < 3; i++) {
        let t   = t_enter + (f32(i) + 0.5) * step;
        total  += sky_cloud_density(camera_pos + view_dir * t, time);
    }
    return clamp(total / 3.0, 0.0, 1.0);
}

// Cheap self-shadow: 2 density samples toward the sun from the cloud midpoint.
// Returns [0,1] where 1 = fully lit, 0 = fully shadowed.
fn sky_cloud_self_shadow(view_dir: vec3<f32>, camera_pos: vec3<f32>, sun_dir: vec3<f32>, time: f32) -> f32 {
    if (view_dir.y < 0.02) { return 1.0; }
    let t_mid   = ((CLOUD_HEIGHT_MIN + CLOUD_HEIGHT_MAX) * 0.5 - camera_pos.y) / view_dir.y;
    let mid_pos = camera_pos + view_dir * t_mid;
    let d1      = sky_cloud_density(mid_pos + sun_dir * 50.0, time);
    let d2      = sky_cloud_density(mid_pos + sun_dir * 100.0, time);
    return exp(-(d1 + d2) * 2.5);
}

// ───────────────────────────────────────────────────────────────────────────
// Time-of-day colour tables
// ───────────────────────────────────────────────────────────────────────────
fn sky_zenith_col(h: f32) -> vec3<f32> {
    let night = vec3<f32>(0.003, 0.006, 0.022);
    let dusk  = vec3<f32>(0.10,  0.14,  0.44);
    let day   = vec3<f32>(0.07,  0.26,  0.78);
    if (h < -0.15) { return night; }
    if (h <  0.12) { return mix(night, dusk, smoothstep(-0.15, 0.12, h)); }
    return mix(dusk, day, smoothstep(0.12, 0.6, h));
}

fn sky_horiz_col(h: f32) -> vec3<f32> {
    let night = vec3<f32>(0.005, 0.008, 0.026);
    let dusk  = vec3<f32>(1.00,  0.42,  0.08);
    let day   = vec3<f32>(0.52,  0.72,  0.96);
    if (h < -0.15) { return night; }
    if (h <  0.12) { return mix(night, dusk, smoothstep(-0.15, 0.12, h)); }
    return mix(dusk, day, smoothstep(0.12, 0.5, h));
}

fn sky_sun_col(h: f32) -> vec3<f32> {
    return mix(vec3<f32>(1.0, 0.45, 0.05), vec3<f32>(1.0, 0.96, 0.88), smoothstep(0.0, 0.4, h));
}

// ───────────────────────────────────────────────────────────────────────────
// Star field (fades in at night)
// ───────────────────────────────────────────────────────────────────────────
fn sky_stars(view_dir: vec3<f32>, sun_height: f32) -> vec3<f32> {
    if (sun_height > 0.15) { return vec3<f32>(0.0); }
    let vis = smoothstep(0.15, -0.10, sun_height);

    let v1   = floor(view_dir * 180.0);
    let h1   = sky_hash(v1);
    let d1   = length(fract(view_dir * 180.0) - 0.5);
    let s1   = smoothstep(0.07, 0.0, d1) * select(0.0, h1 * 1.5, h1 > 0.97);

    let v2   = floor(view_dir * 320.0 + vec3<f32>(17.3, 31.7, 5.1));
    let h2   = sky_hash(v2);
    let d2   = length(fract(view_dir * 320.0 + vec3<f32>(17.3, 31.7, 5.1)) - 0.5);
    let s2   = smoothstep(0.04, 0.0, d2) * select(0.0, h2 * 0.9, h2 > 0.985);

    let col  = mix(vec3<f32>(0.80, 0.85, 1.00), vec3<f32>(1.00, 0.95, 0.80), h1);
    return col * (s1 + s2) * vis;
}

// ───────────────────────────────────────────────────────────────────────────
// Main sky-colour function (view-direction based, no world position required)
// ───────────────────────────────────────────────────────────────────────────
fn calculate_sky_color(view_dir: vec3<f32>) -> vec3<f32> {
    let time       = camera.time;
    let sun_dir    = normalize(vec3<f32>(0.4, 0.6, -0.5));
    let sun_height = sun_dir.y;
    let sun_dot    = dot(view_dir, sun_dir);

    // 1 — Sky gradient
    var sky: vec3<f32>;
    if (view_dir.y < 0.0) {
        let t = saturate(-view_dir.y * 4.0);
        sky   = mix(sky_horiz_col(sun_height) * 0.30, vec3<f32>(0.01), t);
    } else {
        sky   = mix(sky_horiz_col(sun_height), sky_zenith_col(sun_height),
                    1.0 - exp(-view_dir.y * 3.5));
    }

    // // Mie forward scatter: orange glow only near sunset/sunrise, zero at midday.
    // let sunset_factor = clamp(1.0 - sun_height * 4.0, 0.0, 1.0);
    // if (sunset_factor > 0.0 && sun_height > -0.15) {
    //     let mie      = pow(max(0.0, sun_dot), 6.0) * 0.40;
    //     let mie_wide = pow(max(0.0, sun_dot), 2.0) * 0.10;
    //     let mie_str  = max(0.0, sun_height + 0.15) * 0.4 * sunset_factor;
    //     sky         += vec3<f32>(1.0, 0.50, 0.15) * (mie + mie_wide) * mie_str;
    // }

    // 2 — Stars
    sky += sky_stars(view_dir, sun_height);

    // 3 — Clouds (before sun disc)
    //     Use the camera position from the Camera uniform; the sky sphere is
    //     centred on the camera so camera_pos is the ray origin.
    let cloud_density = sky_cloud_coverage(view_dir, camera.position, time);

    if (cloud_density > 0.005) {
        let lit_frac    = smoothstep(0.1, 0.8, view_dir.y);
        let self_shadow = sky_cloud_self_shadow(view_dir, camera.position, sun_dir, time);

        let lit_col   = mix(vec3<f32>(1.0,0.62,0.30), vec3<f32>(1.0,0.98,0.96),
                            smoothstep(0.0, 0.35, sun_height));
        let shad_col  = mix(vec3<f32>(0.28,0.20,0.30), vec3<f32>(0.55,0.62,0.76),
                            smoothstep(0.0, 0.35, sun_height));
        let night_col = vec3<f32>(0.035, 0.035, 0.055);

        let cloud_base = mix(shad_col, lit_col, lit_frac * self_shadow);
        let cloud_col  = mix(night_col, cloud_base, smoothstep(-0.1, 0.12, sun_height));

        let silver = lit_col * pow(1.0 - cloud_density, 3.0) * max(0.0, sun_height) * 0.45 * self_shadow;
        sky        = mix(sky * (1.0 - cloud_density * 0.55),
                         cloud_col + silver, cloud_density * 0.92);
    }

    // 4 — Sun disc (last — attenuated by cloud cover)
    let sun_col = sky_sun_col(sun_height);
    if (sun_height > -0.08 && sun_dot > 0.9985) {
        let disc  = smoothstep(0.9985, 1.0, sun_dot);
        let brt   = mix(6.0, 45.0, smoothstep(0.0, 0.4, sun_height));
        let atten = 1.0 - cloud_density * 0.95;
        sky       = mix(sky, sun_col * brt, disc * atten);
    }

    // Corona glow
    if (sun_height > -0.10 && sun_dot > 0.975) {
        let glow = pow((sun_dot - 0.975) / 0.025, 2.0);
        sky     += sun_col * mix(2.0, 7.0, smoothstep(0.0, 0.4, sun_height))
                 * glow * 0.35 * (1.0 - cloud_density * 0.7);
    }

    return sky;
}

// ───────────────────────────────────────────────────────────────────────────
// Feature entry point — replaces sky-sphere fragment colour only
// ───────────────────────────────────────────────────────────────────────────
fn apply_volumetric_sky(color: vec3<f32>, world_pos: vec3<f32>, camera_pos: vec3<f32>) -> vec3<f32> {
    let dist     = length(world_pos - camera_pos);
    let view_dir = normalize(world_pos - camera_pos);

    if (dist > SKY_DISTANCE_THRESHOLD) {
        return calculate_sky_color(view_dir);
    }
    return color;
}
