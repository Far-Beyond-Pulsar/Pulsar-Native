// Transparent water template for the transparent pass.
// Renders a animated wave surface with real alpha blending,
// SSR-friendly low roughness, and volumetric fog integration.
// (`radiant_eval_transparent(material, input, ambient) -> vec4f`).

fn wave_height(p: vec2<f32>, t: f32) -> f32 {
    let w  = sin(p.x * 1.80 + p.y * 1.20 + t * 0.70) * 0.35;
    let w2 = sin(p.x * 1.00 - p.y * 2.50 + t * 0.90 + 1.40) * 0.25;
    let w3 = sin(p.x * 3.50 + p.y * 3.10 + t * 1.30 + 2.90) * 0.15;
    let w4 = sin(p.x * 0.70 + p.y * 0.50 + t * 0.40 + 0.60) * 0.45;
    let w5 = sin(p.x * 5.20 - p.y * 4.80 + t * 1.80 + 4.10) * 0.08;
    let w6 = sin(p.x * 7.00 + p.y * 6.00 + t * 2.20 + 5.30) * 0.04;
    return w + w2 + w3 + w4 + w5 + w6;
}

fn wave_gradient(p: vec2<f32>, t: f32) -> vec2<f32> {
    let e = 0.015;
    let dx = wave_height(p + vec2<f32>(e, 0.0), t) - wave_height(p - vec2<f32>(e, 0.0), t);
    let dy = wave_height(p + vec2<f32>(0.0, e), t) - wave_height(p - vec2<f32>(0.0, e), t);
    return vec2<f32>(dx, dy) / (2.0 * e);
}

fn wave_normal(p: vec2<f32>, t: f32) -> vec3<f32> {
    let g = wave_gradient(p * 0.80, t);
    return normalize(vec3<f32>(-g.x * 0.60, 1.0, -g.y * 0.60));
}

fn radiant_eval_transparent(material: GpuMaterial, input: MaterialInput, ambient: vec3<f32>) -> vec4<f32> {
    let t = f32(input.frame) * 0.016;
    let uv = input.tex_coords * 8.0;

    // Animated wave normal
    let N = wave_normal(uv, t);

    // Fresnel
    let V = normalize(input.view_position - input.world_position);
    let NdV = max(dot(N, V), 0.0001);
    let fresnel = pow(1.0 - NdV, 4.0);

    // Water color: deep ocean → shallow cyan, with reflection at grazing
    let deep_col    = vec3<f32>(0.005, 0.020, 0.060);
    let shallow_col = vec3<f32>(0.040, 0.220, 0.280);
    let reflection  = vec3<f32>(0.100, 0.160, 0.230);
    let depth_factor = clamp(NdV * 0.80 + 0.20, 0.0, 1.0);
    let color = mix(mix(deep_col, shallow_col, depth_factor), reflection, fresnel);

    // Partial transparency — more transparent straight-on, opaque at grazing
    let alpha = mix(0.60, 0.90, fresnel);

    return vec4<f32>(color, alpha);
}