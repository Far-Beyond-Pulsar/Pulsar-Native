//!use helio_prelude
//!use pbr_eval
//!use light_list
//!use light_forward
//!use specular_aa
//!use material_row
//!use material_surface
//!use mesh_draw_rows

// The meshes' forward program: lit in its drawer by the lights' forward
// function. A material class's template (its surface function) is composed
// after it (`RadiantTemplate::compose`); the material row, its texture table
// and `default_pbr_surface` are the materials' snippets.

enable wgpu_binding_array;


struct Globals {
    frame:             u32,
    delta_time:        f32,
    light_count:       u32,
    ambient_intensity: f32,
    ambient_color:     vec4<f32>,
    num_tiles_x:       u32,
    num_tiles_y:       u32,
    screen_width:      f32,
    screen_height:     f32,
    _pad0: u32,
    time: f32, // RADIANT_GLOBALS_TIME: seconds since start, for graph time nodes
    _pad1: u32,
    _pad2: u32,
}

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<uniform>          globals:           Globals;
@group(0) @binding(2) var<storage, read>    instance_data:     array<GpuInstanceData>;
@group(0) @binding(3) var<storage, read>    compacted_indices: array<u32>;
@group(0) @binding(4) var<storage, read>    lights:            array<GpuLight>;
@group(0) @binding(5) var<storage, read>    tile_light_lists:  array<u32>;
@group(0) @binding(6) var<storage, read>    tile_light_counts: array<u32>;
struct SkinVertex { joints: vec4<u32>, weights: vec4<f32> }
@group(0) @binding(9) var<storage, read> skin_vertices: array<SkinVertex>;
@group(0) @binding(10) var<storage, read> bone_palette: array<mat4x4<f32>>;

@group(1) @binding(0) var<storage, read>    materials:         array<GpuMaterial>;
@group(1) @binding(1) var<storage, read>    material_textures: array<MaterialTextureData>;
@group(1) @binding(2) var                   scene_textures:    binding_array<texture_2d<f32>, 256>;
@group(1) @binding(3) var                   scene_samplers:    binding_array<sampler, 256>;

struct Vertex {
    @location(0) position:       vec3<f32>,
    @location(1) bitangent_sign: f32,
    @location(2) tex_coords:     vec2<f32>,
    @location(3) normal:         u32,
    @location(4) tangent:        u32,
}

struct VertexOutput {
    @invariant @builtin(position) clip_position: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) world_normal:   vec3<f32>,
    @location(2) tex_coords:     vec2<f32>,
    @location(3) world_tangent:  vec3<f32>,
    @location(4) bitangent_sign: f32,
    @location(5) @interpolate(flat) material_id: u32,
}

fn decode_snorm8x4(packed: u32) -> vec3<f32> {
    return unpack4x8snorm(packed).xyz;
}

// `slot` is per-instance vertex data, which every backend offsets by the
// indirect draw's `first_instance` (`helio_mesh_api::draw_slots`).
@vertex
fn vs_main(v: Vertex, @location(15) slot: u32, @builtin(vertex_index) vertex_index: u32) -> VertexOutput {
    let inst       = instance_data[compacted_indices[slot]];
    var local_position = v.position;
    var local_normal = decode_snorm8x4(v.normal);
    var local_tangent = decode_snorm8x4(v.tangent);
    if inst.bone_palette_count > 0u {
        let skin = skin_vertices[vertex_index + inst.skin_vertex_offset];
        var blended = mat4x4<f32>(vec4f(0.0), vec4f(0.0), vec4f(0.0), vec4f(0.0));
        var total = 0.0;
        for (var i = 0u; i < 4u; i = i + 1u) {
            if skin.weights[i] > 0.0 && skin.joints[i] < inst.bone_palette_count {
                blended += bone_palette[inst.bone_palette_offset + skin.joints[i]] * skin.weights[i];
                total += skin.weights[i];
            }
        }
        if total > 0.00001 {
            blended = blended * (1.0 / total);
            local_position = (blended * vec4f(v.position, 1.0)).xyz;
            let skin_normal = mat3x3<f32>(blended[0].xyz, blended[1].xyz, blended[2].xyz);
            local_normal = normalize(skin_normal * local_normal);
            local_tangent = normalize(skin_normal * local_tangent);
        }
    }
    let world_pos  = inst.transform * vec4<f32>(local_position, 1.0);

    let normal_mat = mat3x3<f32>(
        inst.normal_mat_0.xyz,
        inst.normal_mat_1.xyz,
        inst.normal_mat_2.xyz,
    );

    let model_mat3 = mat3x3<f32>(
        inst.transform[0].xyz,
        inst.transform[1].xyz,
        inst.transform[2].xyz,
    );

    var out: VertexOutput;
    out.clip_position  = cameras[0].proj * (cameras[0].view * world_pos);
    out.world_position = world_pos.xyz;
    out.world_normal   = normalize(normal_mat  * local_normal);
    out.world_tangent  = normalize(model_mat3  * local_tangent);
    out.bitangent_sign = v.bitangent_sign;
    out.tex_coords     = v.tex_coords;
    out.material_id    = inst.material_id;
    return out;
}

@fragment
fn fs_main(input: VertexOutput) -> @location(0) vec4<f32> {
    let material = material_at(input.material_id);
    let material_tex = material_texture_data(material,input.material_id);

    let surface = radiant_eval_surface(material, material_tex, MaterialInput(
        input.clip_position, input.world_position, input.world_normal,
        input.world_tangent, input.bitangent_sign, input.tex_coords,
        cameras[0].position_near.xyz, globals.frame,
    ));

    if surface.alpha <= 0.001 { discard; }

    let V = normalize(cameras[0].position_near.xyz - input.world_position);
    let N = surface.normal;
    let NdV = max(dot(N, V), 0.0);
    let albedo = surface.albedo.rgb;
    let alpha = surface.alpha;
    let roughness = surface.roughness;
    let metallic = surface.metallic;
    let F0 = surface.specular_f0;
    let emissive = surface.emissive;
    let ao = surface.ao;

    var Lo = vec3<f32>(0.0);

    // The lights of the pixel's tile (the lights' forward function).
    Lo += light_forward(
        input.clip_position.xy, globals.num_tiles_x, input.world_position,
        N, V, F0, albedo, roughness, metallic,
    );

    let ambient = globals.ambient_color.rgb * globals.ambient_intensity * albedo * ao;
    let color = Lo + ambient + emissive;

    return vec4<f32>(color, alpha);
}
