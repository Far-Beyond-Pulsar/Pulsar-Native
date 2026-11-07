// The default transparent surface (class 0 of the transparent drawers'
// registry): the material's colour under the ambient light plus its
// emission, at its alpha. The drawer adds the lights (the lights' forward
// function) to what this returns. A transparent template replaces this
// function: `radiant_eval_transparent(material, input, ambient)`, `ambient`
// the drawer's ambient radiance.
fn radiant_eval_transparent(material: GpuMaterial, input: MaterialInput, ambient: vec3<f32>) -> vec4<f32> {
    let emission = material.emissive.rgb * material.emissive.a;
    return vec4<f32>(material.base_color.rgb * ambient + emission, clamp(material.base_color.a, 0.0, 1.0));
}
