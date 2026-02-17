// Base Geometry Shader - provides basic geometry rendering without lighting
// Features can inject code at marked injection points

struct Camera {
    view_proj: mat4x4<f32>,
    position: vec3<f32>,
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

// ===== 3D Noise for Clouds =====
fn hash(p: vec3<f32>) -> f32 {
    var p3 = fract(p * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn noise3d(p: vec3<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    
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

fn fbm(p: vec3<f32>) -> f32 {
    var value = 0.0;
    var amplitude = 0.5;
    var frequency = 1.0;
    var pos = p;
    
    for (var i = 0; i < 4; i++) {
        value += amplitude * noise3d(pos * frequency);
        frequency *= 2.0;
        amplitude *= 0.5;
    }
    
    return value;
}

// ===== Atmospheric Sky Colors =====
fn get_sky_gradient(height: f32) -> vec3<f32> {
    // More realistic blue sky gradient
    let horizon_color = vec3<f32>(0.6, 0.75, 0.95);  // Lighter blue at horizon
    let zenith_color = vec3<f32>(0.1, 0.35, 0.8);    // Deep blue at zenith
    return mix(horizon_color, zenith_color, pow(height, 0.5));
}

fn get_sun_color() -> vec3<f32> {
    return vec3<f32>(1.0, 0.98, 0.95);  // Very bright white with slight warmth
}

// ===== Cloud Rendering =====
fn get_cloud_density(world_pos: vec3<f32>, time: f32) -> f32 {
    // Cloud layer parameters
    let cloud_height_min = 200.0;
    let cloud_height_max = 400.0;
    let cloud_thickness = cloud_height_max - cloud_height_min;
    
    // Only clouds in certain height range
    if (world_pos.y < cloud_height_min || world_pos.y > cloud_height_max) {
        return 0.0;
    }
    
    // Normalize height within cloud layer
    let height_factor = (world_pos.y - cloud_height_min) / cloud_thickness;
    
    // Cloud coverage decreases at edges of layer
    let coverage = smoothstep(0.0, 0.2, height_factor) * smoothstep(1.0, 0.8, height_factor);
    
    // Animate clouds by offsetting noise
    let cloud_speed = vec3<f32>(0.5, 0.0, 0.3);  // Wind direction
    let animated_pos = world_pos * 0.003 + cloud_speed * time * 0.001;
    
    // Layered noise for cloud detail
    let base_noise = fbm(animated_pos);
    let detail_noise = fbm(animated_pos * 3.0) * 0.3;
    
    let density = (base_noise + detail_noise - 0.6) * coverage;
    return clamp(density, 0.0, 1.0);
}

// Calculate sky color based on view direction
fn calculate_sky_color(world_pos: vec3<f32>, camera_pos: vec3<f32>) -> vec3<f32> {
    // Get view direction (from camera to fragment)
    let view_dir = normalize(world_pos - camera_pos);
    
    // Height in sky (0 = horizon, 1 = zenith)
    let height = clamp(view_dir.y * 0.5 + 0.5, 0.0, 1.0);
    
    // Base sky gradient
    var sky_color = get_sky_gradient(height);
    
    // Sun direction (elevated sun, slightly angled)
    let sun_dir = normalize(vec3<f32>(0.4, 0.6, -0.5));
    
    // Sun calculations
    let sun_dot = dot(view_dir, sun_dir);
    let sun_disc_size = 0.9998;  // Very tight cone for sun disc
    let sun_glow_size = 0.992;   // Wider cone for glow
    
    // Sun disc (VERY bright for bloom effect)
    if (sun_dot > sun_disc_size) {
        let sun_intensity = smoothstep(sun_disc_size, 1.0, sun_dot);
        sky_color = mix(sky_color, get_sun_color() * 50.0, sun_intensity);
    }
    // Sun glow/halo
    else if (sun_dot > sun_glow_size) {
        let glow_intensity = smoothstep(sun_glow_size, sun_disc_size, sun_dot);
        sky_color = mix(sky_color, get_sun_color() * 3.0, glow_intensity * 0.5);
    }
    
    // Atmospheric haze near horizon
    let horizon_factor = 1.0 - abs(view_dir.y);
    let horizon_glow = pow(horizon_factor, 4.0) * 0.3;
    sky_color += vec3<f32>(horizon_glow * 0.9, horizon_glow * 0.85, horizon_glow * 0.7);
    
    // Add clouds (simple approximation - actual raymarching would be better)
    // Use world position as proxy for cloud position
    let time = 0.0; // TODO: pass actual time uniform
    let sample_pos = camera_pos + view_dir * 300.0;  // Sample at fixed distance
    let cloud_density = get_cloud_density(sample_pos, time);
    
    if (cloud_density > 0.01) {
        let cloud_color = vec3<f32>(1.0, 1.0, 1.0);  // White clouds
        let cloud_shadow = 1.0 - cloud_density * 0.4;
        sky_color = mix(sky_color * cloud_shadow, cloud_color, cloud_density * 0.8);
    }
    
    return sky_color;
}

// Global material ID that can be set per-object
// For now, hardcoded materials by world position
fn get_material_for_fragment(world_pos: vec3<f32>, camera_pos: vec3<f32>) -> MaterialData {
    var mat: MaterialData;
    mat.base_color = vec4<f32>(1.0, 1.0, 1.0, 1.0);
    mat.metallic = 0.0;
    mat.roughness = 0.5;
    mat.emissive_strength = 0.0;
    mat.ao = 1.0;
    
    // Detect sky sphere by distance from camera (>400 units = sky sphere)
    let dist_from_camera = length(world_pos - camera_pos);
    if (dist_from_camera > 400.0) {
        // Sky sphere - calculate atmospheric sky color with clouds
        let sky_color = calculate_sky_color(world_pos, camera_pos);
        mat.base_color = vec4<f32>(sky_color, 1.0);
        mat.emissive_strength = 1.5; // Emissive so it's not affected by lighting
        mat.metallic = 0.0;
        mat.roughness = 1.0;
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


        // Check if emissive material and skip lighting
    let material = get_material_for_fragment(input.world_position, camera.position);
    if (material.emissive_strength > 0.0) {
        // Emissive - already applied in material, no lighting needed
    } else {
        // Apply lighting to non-emissive materials
        final_color = apply_basic_lighting(normalize(input.world_normal), final_color);
    }
    final_color = apply_bloom(final_color);


    // INJECT_FRAGMENTPOSTPROCESS

    return vec4<f32>(final_color, 1.0);
}
