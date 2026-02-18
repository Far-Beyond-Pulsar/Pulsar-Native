// Base Geometry Shader - provides basic geometry rendering without lighting
// Features can inject code at marked injection points

struct Camera {
    view_proj: mat4x4<f32>,
    position: vec3<f32>,
    time: f32,  // Elapsed time in seconds, packed in the vec3 padding slot
};
var<uniform> camera: Camera;

struct Transform {
    model: mat4x4<f32>,
};
var<uniform> transform: Transform;

struct Vertex {
    position: vec3<f32>,
    bitangent_sign: f32,
    tex_coords: vec2<f32>,
    normal: u32,
    tangent: u32,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) tex_coords: vec2<f32>,
};

// INJECT_VERTEXPREAMBLE

fn decode_normal(raw: u32) -> vec3<f32> {
    return unpack4x8snorm(raw).xyz;
}

@vertex
fn vs_main(vertex: Vertex) -> VertexOutput {
    // INJECT_VERTEXMAIN

    let world_pos = transform.model * vec4<f32>(vertex.position, 1.0);
    let world_normal = normalize((transform.model * vec4<f32>(decode_normal(vertex.normal), 0.0)).xyz);

    var output: VertexOutput;
    output.position = camera.view_proj * world_pos;
    output.world_position = world_pos.xyz;
    output.world_normal = world_normal;
    output.tex_coords = vertex.tex_coords;

    // INJECT_VERTEXPOSTPROCESS

    return output;
}

// Material data structures and bindings

struct MaterialData {
    base_color: vec4<f32>,
    metallic: f32,
    roughness: f32,
    emissive_strength: f32,
    ao: f32,
};

// ===== Cloud and Sky Constants =====
const CLOUD_HEIGHT_MIN: f32 = 200.0;   // World-space altitude of cloud base
const CLOUD_HEIGHT_MAX: f32 = 400.0;   // World-space altitude of cloud top
const CLOUD_COVERAGE: f32  = 0.58;     // Fraction of sky covered (0=clear, 1=overcast)
const SUN_DISC_SIZE: f32 = 0.9992;     // Sun angular size (0.9985 = larger, 0.9995 = smaller)
const SUN_GLOW_SIZE: f32 = 0.980;      // Sun corona/glow size

// Helper function: rendering expects BGR, so swap R and B channels
fn rgb(r: f32, g: f32, b: f32) -> vec3<f32> {
    return vec3<f32>(b, g, r);  // Swapped to BGR
}

// ===== 3D Noise =====

fn hash(p: vec3<f32>) -> f32 {
    var p3 = fract(p * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// Smooth quintic-interpolated 3D value noise
fn noise3d(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    // Quintic smoothstep for less grid-aliasing than cubic
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);

    return mix(
        mix(
            mix(hash(i + vec3<f32>(0.0, 0.0, 0.0)), hash(i + vec3<f32>(1.0, 0.0, 0.0)), u.x),
            mix(hash(i + vec3<f32>(0.0, 1.0, 0.0)), hash(i + vec3<f32>(1.0, 1.0, 0.0)), u.x),
            u.y
        ),
        mix(
            mix(hash(i + vec3<f32>(0.0, 0.0, 1.0)), hash(i + vec3<f32>(1.0, 0.0, 1.0)), u.x),
            mix(hash(i + vec3<f32>(0.0, 1.0, 1.0)), hash(i + vec3<f32>(1.0, 1.0, 1.0)), u.x),
            u.y
        ),
        u.z
    );
}

fn fbm(p: vec3<f32>, octaves: i32) -> f32 {
    var value     = 0.0;
    var amplitude = 0.5;
    var frequency = 1.0;
    var pos       = p;

    for (var i = 0; i < octaves; i++) {
        value     += amplitude * noise3d(pos * frequency);
        frequency *= 2.1;
        amplitude *= 0.5;
    }

    return value;
}

// ===== Volumetric Cloud Density =====

// Returns cloud density [0,1] at a world-space position.
// Only nonzero inside the cloud slab [CLOUD_HEIGHT_MIN, CLOUD_HEIGHT_MAX].
fn get_cloud_density(world_pos: vec3<f32>, time: f32) -> f32 {
    let cloud_thickness = CLOUD_HEIGHT_MAX - CLOUD_HEIGHT_MIN;

    // Height normalised within the cloud slab
    let height_frac = (world_pos.y - CLOUD_HEIGHT_MIN) / cloud_thickness;
    if (height_frac < 0.0 || height_frac > 1.0) {
        return 0.0;
    }

    // Height gradient: puffy in the lower two-thirds, wispy at the top
    let height_fade = smoothstep(0.0, 0.12, height_frac)
                    * smoothstep(1.0, 0.55, height_frac);

    // Animate by translating the noise domain over time
    let cloud_speed  = vec3<f32>(0.8, 0.0, 0.45);
    let anim_pos     = world_pos * 0.0025 + cloud_speed * time * 0.00025;

    // Large-scale base shape
    let base = fbm(anim_pos, 4);

    // Coverage threshold: erode below the threshold so we get distinct cloud masses
    let threshold    = 1.0 - CLOUD_COVERAGE;
    let coverage_raw = max(0.0, base - threshold) / CLOUD_COVERAGE;
    if (coverage_raw < 0.001) {
        return 0.0;
    }

    // Fine detail erosion for fluffy, billowing edges
    let detail_pos = anim_pos * 2.7 + vec3<f32>(0.1, 0.2, 0.3);
    let detail     = fbm(detail_pos, 3) * 0.35;
    let shaped     = max(0.0, coverage_raw - detail * (1.0 - coverage_raw));

    return clamp(shaped * height_fade, 0.0, 1.0);
}

// Integrate cloud density along the view ray through the cloud slab.
// Uses 3-sample ray marching through the cloud slab for correct coverage at
// all viewing angles (near-horizon clouds are now visible, not just overhead).
fn get_cloud_coverage_for_ray(view_dir: vec3<f32>, camera_pos: vec3<f32>, time: f32) -> f32 {
    // Camera is always below the clouds; ray must be pointing upward
    if (view_dir.y < 0.02) {
        return 0.0;
    }

    // Ray-slab intersection
    let t_min = (CLOUD_HEIGHT_MIN - camera_pos.y) / view_dir.y;
    let t_max = (CLOUD_HEIGHT_MAX - camera_pos.y) / view_dir.y;
    if (t_min < 0.0 && t_max < 0.0) {
        return 0.0;
    }

    let t_enter   = max(0.0, t_min);
    let step_size = (t_max - t_enter) / 3.0;

    var total = 0.0;
    for (var i = 0; i < 3; i++) {
        let t          = t_enter + (f32(i) + 0.5) * step_size;
        let sample_pos = camera_pos + view_dir * t;
        total         += get_cloud_density(sample_pos, time);
    }

    return clamp(total / 3.0, 0.0, 1.0);
}

// Cheap self-shadow: 2 density samples toward the sun from the cloud midpoint.
// Returns [0,1] where 1 = fully lit, 0 = fully shadowed.  Costs ~2 noise evals.
fn get_cloud_self_shadow(view_dir: vec3<f32>, camera_pos: vec3<f32>, sun_dir: vec3<f32>, time: f32) -> f32 {
    if (view_dir.y < 0.02) { return 1.0; }
    let t_mid   = ((CLOUD_HEIGHT_MIN + CLOUD_HEIGHT_MAX) * 0.5 - camera_pos.y) / view_dir.y;
    let mid_pos = camera_pos + view_dir * t_mid;
    let d1      = get_cloud_density(mid_pos + sun_dir * 50.0, time);
    let d2      = get_cloud_density(mid_pos + sun_dir * 100.0, time);
    return exp(-(d1 + d2) * 2.5);
}

// ===== Time-of-Day Sky Gradient =====
// sun_height: -1 = midnight below horizon, 0 = on the horizon, +1 = zenith noon

fn get_sky_zenith_color(sun_height: f32) -> vec3<f32> {
    // Define colors in RGB and convert to BGR for rendering
    let night    = rgb(0.003, 0.006, 0.022);   // Dark navy
    let twilight = rgb(0.10,  0.14,  0.44);    // Dusky purple-blue
    let day      = rgb(0.07,  0.26,  0.78);    // Rich azure

    if (sun_height < -0.15) {
        return night;
    } else if (sun_height < 0.12) {
        return mix(night, twilight, smoothstep(-0.15, 0.12, sun_height));
    } else {
        return mix(twilight, day, smoothstep(0.12, 0.6, sun_height));
    }
}

fn get_sky_horizon_color(sun_height: f32) -> vec3<f32> {
    // Define colors in RGB and convert to BGR for rendering
    let night    = rgb(0.005, 0.008, 0.026);  // Near-black blue
    let twilight = rgb(1.00,  0.42,  0.08);   // Burning orange
    let day      = rgb(0.52,  0.72,  0.96);   // Pale sky blue

    if (sun_height < -0.15) {
        return night;
    } else if (sun_height < 0.12) {
        return mix(night, twilight, smoothstep(-0.15, 0.12, sun_height));
    } else {
        return mix(twilight, day, smoothstep(0.12, 0.5, sun_height));
    }
}

fn get_sun_disc_color(sun_height: f32) -> vec3<f32> {
    // Define colors in RGB and convert to BGR for rendering
    let sunset = rgb(1.0, 0.45, 0.05);  // Orange-red
    let noon   = rgb(1.0, 0.96, 0.88);  // Warm white
    return mix(sunset, noon, smoothstep(0.0, 0.4, sun_height));
}

// ===== Star Field =====
fn get_stars(view_dir: vec3<f32>, sun_height: f32) -> vec3<f32> {
    if (sun_height > 0.15) {
        return rgb(0.0, 0.0, 0.0);  // No stars in daylight
    }
    let visibility = smoothstep(0.15, -0.10, sun_height);

    // Two layers of stars at different angular densities for depth
    let v1    = floor(view_dir * 180.0);
    let h1    = hash(v1);
    let d1    = length(fract(view_dir * 180.0) - 0.5);
    let star1 = smoothstep(0.07, 0.0, d1) * select(0.0, h1 * 1.5, h1 > 0.97);

    let v2    = floor(view_dir * 320.0 + vec3<f32>(17.3, 31.7, 5.1));
    let h2    = hash(v2);
    let d2    = length(fract(view_dir * 320.0 + vec3<f32>(17.3, 31.7, 5.1)) - 0.5);
    let star2 = smoothstep(0.04, 0.0, d2) * select(0.0, h2 * 0.9, h2 > 0.985);

    // Slight blue-orange variation like real stars
    let star_col = mix(rgb(0.80, 0.85, 1.00), rgb(1.00, 0.95, 0.80), h1);
    return star_col * (star1 + star2) * visibility;
}

// ===== Main Sky Colour Calculation =====

fn calculate_sky_color(world_pos: vec3<f32>, camera_pos: vec3<f32>) -> vec3<f32> {
    let view_dir = normalize(world_pos - camera_pos);
    let time     = camera.time;  // Elapsed seconds from CameraUniforms

    // --- Sun direction ---
    // Static midday position; swap for animated rotation once a time-of-day
    // system passes a cycle speed:
    //   let angle = time * 0.0002;
    //   let sun_dir = normalize(vec3(cos(angle), sin(angle) * 0.85, 0.3));
    let sun_dir    = normalize(vec3<f32>(0.4, 0.6, -0.5));
    let sun_height = sun_dir.y;                          // –1 … +1
    let sun_dot    = dot(view_dir, sun_dir);

    // === 1. Atmospheric sky gradient (Rayleigh-style) ===
    let zenith_col  = get_sky_zenith_color(sun_height);
    let horizon_col = get_sky_horizon_color(sun_height);

    var sky_color: vec3<f32>;
    if (view_dir.y < 0.0) {
        // Below horizon: dark ground fog fading to black
        let ground_t = saturate(-view_dir.y * 4.0);
        sky_color = mix(horizon_col * 0.30, rgb(0.01, 0.01, 0.01), ground_t);
    } else {
        // Exponential altitude blend (thicker atmosphere near horizon)
        let alt_t = 1.0 - exp(-view_dir.y * 3.5);
        sky_color = mix(horizon_col, zenith_col, alt_t);
    }

    // Mie forward-scatter: orange glow only near sunset/sunrise.
    // sunset_factor = 1.0 at horizon, ramps to 0.0 when sun_height >= 0.25,
    // so the midday sky stays clean blue with no warm tint.
    // let sunset_factor = clamp(1.0 - sun_height * 4.0, 0.0, 1.0);
    // if (sunset_factor > 0.0 && sun_height > -0.15) {
    //     let mie      = pow(max(0.0, sun_dot), 6.0) * 0.40;
    //     let mie_wide = pow(max(0.0, sun_dot), 2.0) * 0.10;
    //     let mie_str  = max(0.0, sun_height + 0.15) * 0.4 * sunset_factor;
    //     sky_color   += vec3<f32>(1.0, 0.50, 0.15) * (mie + mie_wide) * mie_str;
    // }

    // === 2. Night stars ===
    sky_color += get_stars(view_dir, sun_height);

    // === 3. Volumetric clouds — sampled BEFORE the sun disc ===
    //    Correct ray-slab intersection means clouds appear at all elevations,
    //    not just directly overhead.
    let cloud_density = get_cloud_coverage_for_ray(view_dir, camera_pos, time);

    if (cloud_density > 0.005) {
        // Lit fraction: top of cloud (high view_dir.y) = lit, underside = shadowed.
        let lit_frac = smoothstep(0.1, 0.8, view_dir.y);

        // Self-shadow: density above the cloud midpoint toward the sun darkens interiors.
        let self_shadow = get_cloud_self_shadow(view_dir, camera_pos, sun_dir, time);

        let lit_col    = mix(
            rgb(1.0,  0.62, 0.30),  // Warm golden at sunset
            rgb(1.0,  0.98, 0.96),  // Cool bright white at noon
            smoothstep(0.0, 0.35, sun_height)
        );
        let shadow_col = mix(
            rgb(0.28, 0.20, 0.30),  // Deep violet-grey at sunset
            rgb(0.55, 0.62, 0.76),  // Cool blue-grey at noon
            smoothstep(0.0, 0.35, sun_height)
        );
        let night_col  = rgb(0.035, 0.035, 0.055); // Almost-black night cloud

        // Combine view-elevation shading with self-shadow: thick cloud = dark interior
        let cloud_base  = mix(shadow_col, lit_col, lit_frac * self_shadow);
        let cloud_color = mix(night_col, cloud_base, smoothstep(-0.1, 0.12, sun_height));

        // Silver-lining: thin sunlit edges (attenuated where self-shadowed)
        let edge_bright = pow(1.0 - cloud_density, 3.0) * max(0.0, sun_height) * 0.45 * self_shadow;
        let silver      = lit_col * edge_bright;

        // Sky darkens under the cloud mass (shadow on the air below)
        let sky_shadow = 1.0 - cloud_density * 0.55;
        sky_color = mix(sky_color * sky_shadow, cloud_color + silver, cloud_density * 0.92);
    }

    // === 4. Sun disc — applied last, attenuated by cloud cover ===
    let sun_col  = get_sun_disc_color(sun_height);
    let sun_occl = cloud_density; // how much cloud is blocking the sun

    // Disc
    if (sun_height > -0.08 && sun_dot > SUN_DISC_SIZE) {
        let disc_t     = smoothstep(SUN_DISC_SIZE, 1.0, sun_dot);
        let brightness = mix(6.0, 45.0, smoothstep(0.0, 0.4, sun_height));
        let atten      = 1.0 - sun_occl * 0.95;
        sky_color      = mix(sky_color, sun_col * brightness, disc_t * atten);
    }

    // Corona / inner glow
    if (sun_height > -0.10 && sun_dot > SUN_GLOW_SIZE) {
        let glow       = pow((sun_dot - SUN_GLOW_SIZE) / (1.0 - SUN_GLOW_SIZE), 2.0);
        let glow_col   = sun_col * mix(2.0, 7.0, smoothstep(0.0, 0.4, sun_height));
        sky_color     += glow_col * glow * 0.35 * (1.0 - sun_occl * 0.7);
    }

    return sky_color;
}

// ===== Material Dispatch =====

fn get_material_for_fragment(world_pos: vec3<f32>, camera_pos: vec3<f32>) -> MaterialData {
    var mat: MaterialData;
    mat.base_color       = vec4<f32>(1.0, 1.0, 1.0, 1.0);
    mat.metallic         = 0.0;
    mat.roughness        = 0.5;
    mat.emissive_strength = 0.0;
    mat.ao               = 1.0;

    // Sky sphere detection: fragments more than 400 units from the camera
    // belong to the inverted sky sphere
    let dist = length(world_pos - camera_pos);
    if (dist > 400.0) {
        let sky_color         = calculate_sky_color(world_pos, camera_pos);
        mat.base_color        = vec4<f32>(sky_color, 1.0);
        mat.emissive_strength = 1.5;
        mat.metallic          = 0.0;
        mat.roughness         = 1.0;
    }

    return mat;
}

// Material processing functions with procedural textures

fn checkerboard_pattern(uv: vec2<f32>, scale: f32) -> f32 {
    let scaled_uv = uv * scale;
    let checker = floor(scaled_uv.x) + floor(scaled_uv.y);
    return fract(checker * 0.5) * 2.0;
}

fn get_texture_color(uv: vec2<f32>) -> vec3<f32> {
    // Create a procedural checkerboard texture
    let checker = checkerboard_pattern(uv, 8.0);

    // Alternate between two colors
    let color1 = vec3<f32>(0.9, 0.9, 0.9); // Light gray
    let color2 = vec3<f32>(0.3, 0.5, 0.7); // Blue-gray

    return mix(color2, color1, checker);
}

fn apply_material_color(base_color: vec3<f32>, tex_coords: vec2<f32>, world_pos: vec3<f32>, camera_pos: vec3<f32>) -> vec3<f32> {
    // Get material data for this fragment
    let material = get_material_for_fragment(world_pos, camera_pos);
    
    // If emissive, skip texture and return bright emissive color
    if (material.emissive_strength > 0.0) {
        return material.base_color.rgb * material.emissive_strength;
    }
    
    // Normal textured material
    let texture_color = get_texture_color(tex_coords);
    return base_color * texture_color * material.base_color.rgb;
}

// Basic lighting functions

fn calculate_diffuse_lighting(normal: vec3<f32>, light_dir: vec3<f32>, base_color: vec3<f32>) -> vec3<f32> {
    let ndotl = max(dot(normal, light_dir), 0.0);
    return base_color * ndotl;
}

fn calculate_ambient_lighting(base_color: vec3<f32>, ambient_strength: f32) -> vec3<f32> {
    return base_color * ambient_strength;
}

fn apply_basic_lighting(world_normal: vec3<f32>, base_color: vec3<f32>) -> vec3<f32> {
    // Simple directional light from top-right
    let light_dir = normalize(vec3<f32>(0.5, 1.0, 0.3));

    let diffuse = calculate_diffuse_lighting(world_normal, light_dir, base_color);
    let ambient = calculate_ambient_lighting(base_color, 0.2);

    return diffuse + ambient;
}

// Apply emissive to bypass lighting completely
fn apply_emissive_lighting(base_color: vec3<f32>, emissive_strength: f32) -> vec3<f32> {
    // If emissive strength > 0, return bright unlit color
    if (emissive_strength > 0.0) {
        return base_color * emissive_strength;
    }
    return base_color;
}

// High-quality realtime shadow mapping with PCF for multiple lights
// Supports up to 8 overlapping shadow-casting lights with attenuation.
// Point lights use 6-face cubemap shadows stored in consecutive 2D array layers.

// Maximum number of shadow-casting lights
const MAX_SHADOW_LIGHTS: u32 = 8u;

// Light types
const LIGHT_TYPE_DIRECTIONAL: f32 = 0.0;
const LIGHT_TYPE_POINT: f32 = 1.0;
const LIGHT_TYPE_SPOT: f32 = 2.0;
const LIGHT_TYPE_RECT: f32 = 3.0;

// Shadow map texture array and comparison sampler (bound automatically by ShaderData)
var shadow_maps: texture_depth_2d_array;
var shadow_sampler: sampler_comparison;

// GPU representation of a light
struct GpuLight {
    view_proj: mat4x4<f32>,
    position_and_type: vec4<f32>,      // xyz = position, w = light type
    direction_and_radius: vec4<f32>,   // xyz = direction, w = attenuation radius
    color_and_intensity: vec4<f32>,    // rgb = color, a = intensity
    params: vec4<f32>,                  // x = inner angle, y = outer angle, z = falloff, w = base shadow layer
}

// Lighting uniforms containing all lights
struct LightingUniforms {
    light_count: vec4<f32>,  // x = count, y = ambient, zw unused
    lights: array<GpuLight, MAX_SHADOW_LIGHTS>,
}
var<uniform> lighting: LightingUniforms;

// ACES filmic tone mapping (Narkowicz 2015).
// Maps HDR linear radiance to [0,1] with an S-curve: lifted shadows,
// rolled-off highlights. Prevents hard clipping when lights overlap.
fn aces_tonemap(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51;
    let b = 0.03;
    let c = 2.43;
    let d = 0.59;
    let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Encode linear light value to sRGB gamma for display (pow 1/2.2 approximation).
fn linear_to_srgb(linear: vec3<f32>) -> vec3<f32> {
    return pow(max(linear, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.2));
}

// Calculate attenuation for a light based on distance and light parameters
fn calculate_attenuation(light: GpuLight, world_pos: vec3<f32>) -> f32 {
    let light_type = light.position_and_type.w;

    // Directional lights have no attenuation
    if (light_type == LIGHT_TYPE_DIRECTIONAL) {
        return 1.0;
    }

    let light_pos = light.position_and_type.xyz;
    let attenuation_radius = light.direction_and_radius.w;
    let falloff = light.params.z;

    let distance = length(world_pos - light_pos);

    // Smooth falloff using inverse square law with custom exponent
    // Reaches zero at attenuation_radius
    if (distance >= attenuation_radius) {
        return 0.0;
    }

    let normalized_distance = distance / attenuation_radius;
    let attenuation = pow(1.0 - normalized_distance, falloff);

    return attenuation;
}

// Calculate spotlight cone attenuation
fn calculate_spot_cone_attenuation(light: GpuLight, world_pos: vec3<f32>) -> f32 {
    let light_type = light.position_and_type.w;

    if (light_type != LIGHT_TYPE_SPOT) {
        return 1.0;
    }

    let light_pos = light.position_and_type.xyz;
    let light_dir = light.direction_and_radius.xyz;
    let inner_angle = light.params.x;
    let outer_angle = light.params.y;

    let to_pixel = normalize(world_pos - light_pos);
    let cos_angle = dot(to_pixel, light_dir);

    let cos_inner = cos(inner_angle);
    let cos_outer = cos(outer_angle);

    // Outside spotlight cone
    if (cos_angle < cos_outer) {
        return 0.0;
    }

    // Smooth transition between inner and outer cone
    if (cos_angle > cos_inner) {
        return 1.0;
    }

    return smoothstep(cos_outer, cos_inner, cos_angle);
}

// Helper: right-handed look-at view matrix (matches glam::Mat4::look_at_rh)
fn look_at_rh(eye: vec3<f32>, center: vec3<f32>, up: vec3<f32>) -> mat4x4<f32> {
    let f = normalize(center - eye);
    let s = normalize(cross(f, up));
    let u = cross(s, f);

    return mat4x4<f32>(
        vec4<f32>(s.x, u.x, -f.x, 0.0),
        vec4<f32>(s.y, u.y, -f.y, 0.0),
        vec4<f32>(s.z, u.z, -f.z, 0.0),
        vec4<f32>(-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0),
    );
}

// Helper: right-handed perspective projection (matches glam::Mat4::perspective_rh)
// Produces Vulkan NDC depth [0, 1].
fn perspective_rh(fov_y: f32, aspect: f32, near: f32, far: f32) -> mat4x4<f32> {
    let h = cos(fov_y * 0.5) / sin(fov_y * 0.5);
    let w = h / aspect;
    let r = far / (near - far);
    return mat4x4<f32>(
        vec4<f32>(w, 0.0, 0.0, 0.0),
        vec4<f32>(0.0, h, 0.0, 0.0),
        vec4<f32>(0.0, 0.0, r, -1.0),
        vec4<f32>(0.0, 0.0, r * near, 0.0),
    );
}

// Select which of the 6 cube faces a direction vector maps to.
// Face indices: 0=+X, 1=-X, 2=+Y, 3=-Y, 4=+Z, 5=-Z
// Matches the CUBE_FACE_DIRS ordering in ProceduralShadows::CUBE_FACE_DIRS.
fn select_cube_face(d: vec3<f32>) -> i32 {
    let abs_d = abs(d);
    if (abs_d.x >= abs_d.y && abs_d.x >= abs_d.z) {
        if (d.x >= 0.0) { return 0; } else { return 1; }
    } else if (abs_d.y >= abs_d.z) {
        if (d.y >= 0.0) { return 2; } else { return 3; }
    } else {
        if (d.z >= 0.0) { return 4; } else { return 5; }
    }
}

// Reconstruct the view-projection matrix for a given cube face from a point light.
// Must match the matrices produced by ProceduralShadows::get_shadow_render_matrices.
fn get_cube_face_view_proj(light_pos: vec3<f32>, face: i32, near: f32, far: f32) -> mat4x4<f32> {
    let proj = perspective_rh(radians(90.0), 1.0, near, far);

    var forward: vec3<f32>;
    var up: vec3<f32>;
    if (face == 0) {        // +X
        forward = vec3<f32>(1.0, 0.0, 0.0);
        up      = vec3<f32>(0.0, -1.0, 0.0);
    } else if (face == 1) { // -X
        forward = vec3<f32>(-1.0, 0.0, 0.0);
        up      = vec3<f32>(0.0, -1.0, 0.0);
    } else if (face == 2) { // +Y
        forward = vec3<f32>(0.0, 1.0, 0.0);
        up      = vec3<f32>(0.0, 0.0, 1.0);
    } else if (face == 3) { // -Y
        forward = vec3<f32>(0.0, -1.0, 0.0);
        up      = vec3<f32>(0.0, 0.0, -1.0);
    } else if (face == 4) { // +Z
        forward = vec3<f32>(0.0, 0.0, 1.0);
        up      = vec3<f32>(0.0, -1.0, 0.0);
    } else {                // -Z (face == 5)
        forward = vec3<f32>(0.0, 0.0, -1.0);
        up      = vec3<f32>(0.0, -1.0, 0.0);
    }

    let view = look_at_rh(light_pos, light_pos + forward, up);
    return proj * view;
}

// High-quality PCF shadow sampling with 5x5 kernel for texture array
fn sample_shadow_pcf(shadow_coord: vec3<f32>, layer: i32, texel_size: vec2<f32>) -> f32 {
    var shadow = 0.0;
    let samples = 5;
    let half_samples = f32(samples) / 2.0;

    // 5x5 PCF kernel for smooth shadows
    for (var y = -2; y <= 2; y++) {
        for (var x = -2; x <= 2; x++) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel_size;
            let sample_coord = shadow_coord.xy + offset;

            // Use hardware comparison sampling for efficiency with array layer
            shadow += textureSampleCompareLevel(
                shadow_maps,
                shadow_sampler,
                sample_coord,
                layer,
                shadow_coord.z
            );
        }
    }

    // Average the samples (25 samples for 5x5 kernel)
    return shadow / 25.0;
}

// Optimized PCF shadow sampling with 3x3 kernel for texture array
fn sample_shadow_pcf_3x3(shadow_coord: vec3<f32>, layer: i32, texel_size: vec2<f32>) -> f32 {
    var shadow = 0.0;

    // 3x3 PCF kernel
    for (var y = -1; y <= 1; y++) {
        for (var x = -1; x <= 1; x++) {
            let offset = vec2<f32>(f32(x), f32(y)) * texel_size;
            let sample_coord = shadow_coord.xy + offset;

            shadow += textureSampleCompareLevel(
                shadow_maps,
                shadow_sampler,
                sample_coord,
                layer,
                shadow_coord.z
            );
        }
    }

    return shadow / 9.0;
}

// Sample shadow visibility using PCF for a specific light layer
fn sample_shadow_visibility(shadow_coord: vec3<f32>, layer: i32) -> f32 {
    // Get shadow map dimensions for texel size calculation
    let shadow_map_size = vec2<f32>(textureDimensions(shadow_maps));
    let texel_size = 1.0 / shadow_map_size;

    // Use 3x3 PCF for good quality and performance
    return sample_shadow_pcf_3x3(shadow_coord, layer, texel_size);
}

// Transform world position to shadow map space
fn world_to_shadow_coord(world_pos: vec3<f32>, light_view_proj: mat4x4<f32>) -> vec3<f32> {
    let light_space = light_view_proj * vec4<f32>(world_pos, 1.0);
    var shadow_coord = light_space.xyz / light_space.w;

    // Transform to [0, 1] range for texture coordinates
    shadow_coord.x = shadow_coord.x * 0.5 + 0.5;
    shadow_coord.y = shadow_coord.y * -0.5 + 0.5;  // Flip Y for texture coords

    return shadow_coord;
}

// Sample point light shadow by selecting the correct cube face layer.
// `base_layer` is params.w (= light_index * 6). The face offset 0-5 is added
// based on the fragment-to-light direction's dominant axis.
fn sample_point_light_shadow(
    light: GpuLight,
    base_layer: i32,
    offset_pos: vec3<f32>,
    bias: f32,
) -> f32 {
    let light_pos = light.position_and_type.xyz;
    let near = 0.1;
    let far = light.direction_and_radius.w;

    let to_fragment = offset_pos - light_pos;
    let face = select_cube_face(to_fragment);
    let view_proj = get_cube_face_view_proj(light_pos, face, near, far);

    var shadow_coord = world_to_shadow_coord(offset_pos, view_proj);

    // Fragment outside this face's frustum; treat as unoccluded
    if (shadow_coord.x < 0.0 || shadow_coord.x > 1.0 ||
        shadow_coord.y < 0.0 || shadow_coord.y > 1.0 ||
        shadow_coord.z < 0.0 || shadow_coord.z > 1.0) {
        return 1.0;
    }

    shadow_coord.z -= bias;

    let layer = base_layer + face;
    let shadow_map_size = vec2<f32>(textureDimensions(shadow_maps));
    let texel_size = 1.0 / shadow_map_size;
    return sample_shadow_pcf_3x3(shadow_coord, layer, texel_size);
}

// Calculate shadow and lighting contribution from a single light
fn calculate_light_contribution(
    light: GpuLight,
    base_layer: i32,
    world_pos: vec3<f32>,
    world_normal: vec3<f32>
) -> vec3<f32> {
    let light_type = light.position_and_type.w;
    let light_pos = light.position_and_type.xyz;
    let light_dir_stored = light.direction_and_radius.xyz;
    let light_color = light.color_and_intensity.xyz;
    let light_intensity = light.color_and_intensity.w;

    // Calculate light direction based on type
    var light_dir: vec3<f32>;
    if (light_type == LIGHT_TYPE_DIRECTIONAL) {
        light_dir = -light_dir_stored;
    } else {
        light_dir = normalize(light_pos - world_pos);
    }

    let normal = normalize(world_normal);
    let ndotl = max(dot(normal, light_dir), 0.0);

    // Calculate attenuation based on distance
    let distance_attenuation = calculate_attenuation(light, world_pos);
    if (distance_attenuation < 0.001) {
        return vec3<f32>(0.0);
    }

    // Calculate spotlight cone attenuation
    let cone_attenuation = calculate_spot_cone_attenuation(light, world_pos);
    if (cone_attenuation < 0.001) {
        return vec3<f32>(0.0);
    }

    // Back-face cull: surfaces facing away receive no light.
    if (ndotl < 0.001) {
        return vec3<f32>(0.0);
    }

    // No bias - PCF filtering handles shadow acne
    let normal_offset = 0.0;
    let offset_pos = world_pos + normal * normal_offset;
    let shadow_bias = 0.0;

    var visibility = 1.0;

    if (light_type == LIGHT_TYPE_POINT) {
        // Point lights use per-face cubemap lookup across 6 texture array layers
        visibility = sample_point_light_shadow(light, base_layer, offset_pos, shadow_bias);
    } else {
        var shadow_coord = world_to_shadow_coord(offset_pos, light.view_proj);

        // Check if position is within shadow map bounds
        if (shadow_coord.x >= 0.0 && shadow_coord.x <= 1.0 &&
            shadow_coord.y >= 0.0 && shadow_coord.y <= 1.0 &&
            shadow_coord.z >= 0.0 && shadow_coord.z <= 1.0) {

            shadow_coord.z -= shadow_bias;
            visibility = sample_shadow_visibility(shadow_coord, base_layer);
        }
    }

    // Lambertian diffuse: brightness scales with cos(angle to light) = ndotl.
    // Replaces the old binary face_fade so surfaces actually dim toward the terminator.
    let combined_attenuation = distance_attenuation * cone_attenuation * ndotl * visibility;
    return light_color * light_intensity * combined_attenuation;
}

// Apply multi-light shadows and lighting to color
fn apply_shadow(base_color: vec3<f32>, world_pos: vec3<f32>, world_normal: vec3<f32>) -> vec3<f32> {
    // Check for emissive materials - bypass all lighting and shadows
    let material = get_material_for_fragment(world_pos, camera.position);
    if (material.emissive_strength > 0.0) {
        // Return BRIGHT emissive color - no lighting or tone mapping
        return material.base_color.rgb * material.emissive_strength;
    }
    
    let light_count = i32(lighting.light_count.x);
    let ambient = lighting.light_count.y;

    // No lights - return ambient only
    if (light_count == 0) {
        return base_color * ambient;
    }

    // Accumulate lighting from all lights
    var total_lighting = vec3<f32>(0.0);

    for (var i = 0; i < light_count; i++) {
        let light = lighting.lights[i];
        let base_layer = i32(light.params.w);

        let light_contribution = calculate_light_contribution(
            light,
            base_layer,
            world_pos,
            world_normal
        );

        total_lighting += light_contribution;
    }

    // Ambient keeps unlit surfaces from going fully black.
    // Use max() instead of adding to prevent double-lighting
    let final_lighting = max(total_lighting, vec3<f32>(ambient));

    // Multiply albedo by accumulated radiance (no hard clamp; ACES handles HDR).
    let linear_radiance = base_color * final_lighting;

    // ACES tone map then gamma encode for display.
    return linear_to_srgb(aces_tonemap(linear_radiance));
}

// Simple efficient bloom through shader injection
// Adds glow to overlit fragments using a quick approximation

// Apply bloom glow to overlit pixels
fn apply_bloom(color: vec3<f32>) -> vec3<f32> {
    // Calculate luminance
    let luminance = dot(color, vec3<f32>(0.2126, 0.7152, 0.0722));
    
    // Only bloom pixels brighter than 1.0
    let bloom_threshold = 1.0;
    let excess = max(0.0, luminance - bloom_threshold);
    
    // Create bloom glow proportional to excess brightness
    let bloom_intensity = 0.3;
    let bloom = color * (excess * bloom_intensity);
    
    return color + bloom;
}



@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    // Default gray color
    var final_color = vec3<f32>(0.8);

        final_color = apply_material_color(final_color, input.tex_coords, input.world_position, camera.position);
    let shadow_albedo = final_color;


        // Check if emissive material and skip lighting
    let material = get_material_for_fragment(input.world_position, camera.position);
    if (material.emissive_strength > 0.0) {
        // Emissive - already applied in material, no lighting needed
    } else {
        // Apply lighting to non-emissive materials
        final_color = apply_basic_lighting(normalize(input.world_normal), final_color);
    }
    final_color = apply_shadow(shadow_albedo, input.world_position, input.world_normal);
    final_color = apply_bloom(final_color);


    // INJECT_FRAGMENTPOSTPROCESS

    return vec4<f32>(final_color, 1.0);
}
