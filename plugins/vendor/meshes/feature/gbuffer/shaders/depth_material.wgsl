enable wgpu_binding_array;
//!use pbr_eval
//!use material_row
//!use specular_aa
//!use material_surface

// The material table for cut-out-aware depth drawers (shadow casters): the
// materials' bindings (group 1, the G-buffer's layout) and the cut-out test
// (`material_cut_out`, the materials' snippet). The drawer appends its own
// geometry program.

@group(1) @binding(0) var<storage, read>    materials:          array<GpuMaterial>;
@group(1) @binding(1) var<storage, read>    material_textures:  array<MaterialTextureData>;
@group(1) @binding(2) var                   scene_textures:     binding_array<texture_2d<f32>, 256>;
@group(1) @binding(3) var                   scene_samplers:     binding_array<sampler, 256>;

fn reject_cutout(material_id: u32, uv: vec2<f32>) {
    if material_cut_out(material_id, uv) { discard; }
}
