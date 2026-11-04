// Glass transparent template for the transparent drawers
// (`radiant_eval_transparent(material, input, ambient) -> vec4f`).

fn radiant_eval_transparent(material: GpuMaterial, input: MaterialInput, ambient: vec3<f32>) -> vec4<f32> {
    // DEBUG: bright green so we can see it
    return vec4<f32>(0.0, 1.0, 0.0, 0.8);
}
