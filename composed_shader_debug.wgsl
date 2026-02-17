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

    // INJECT_FRAGMENTMAIN

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
