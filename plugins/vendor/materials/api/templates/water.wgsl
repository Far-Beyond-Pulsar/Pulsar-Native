// 6-octave Gerstner wave height
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
    let steepness = 0.60;
    return normalize(vec3<f32>(-g.x * steepness, 1.0, -g.y * steepness));
}

// Foam noise (value noise from sin hash)
fn foam_noise(p: vec2<f32>, t: f32) -> f32 {
    let d = p * 4.0 + t * 0.15;
    let n = sin(dot(d, vec2<f32>(12.9898, 78.233))) * 43758.5453;
    return fract(n);
}

// Caustic shimmer
fn caustic(p: vec2<f32>, t: f32) -> f32 {
    let q = p * 2.5 + t * 0.12;
    let c1 = sin(q.x * 5.0 + sin(q.y * 3.0 + t * 0.50)) * 0.50 + 0.50;
    let c2 = sin(q.y * 4.0 + sin(q.x * 6.0 + t * 0.70 + 1.0)) * 0.50 + 0.50;
    return c1 * c2;
}

fn radiant_eval_surface(material: GpuMaterial,
                        material_tex: MaterialTextureData,
                        input: MaterialInput) -> SurfaceData {
    var s = default_pbr_surface(material, material_tex, input);

    let t = f32(input.frame) * 0.016;
    let uv = input.tex_coords * 8.0;

    // ── Animated wave normal ───────────────────────────────────────────
    let N = wave_normal(uv, t);

    // ── View direction and Fresnel ─────────────────────────────────────
    let V = normalize(input.view_position - input.world_position);
    let NdV = max(dot(N, V), 0.0001);
    let fresnel = pow(1.0 - NdV, 4.0);

    // ── Water body color (deep ocean → shallow reef) ───────────────────
    let deep_col    = vec3<f32>(0.003, 0.015, 0.055);
    let shallow_col = vec3<f32>(0.030, 0.200, 0.250);
    let reflection  = vec3<f32>(0.080, 0.150, 0.220);

    let depth_factor = clamp(NdV * 0.80 + 0.20, 0.0, 1.0);
    let depth_col = mix(deep_col, shallow_col, depth_factor);

    // ── Foam at wave crests ────────────────────────────────────────────
    let wave_h = wave_height(uv, t);
    let f_noise = foam_noise(uv + t * 0.02, t);
    let foam_amount = smoothstep(0.55, 0.90, abs(wave_h) + f_noise * 0.12);
    let foam_col = vec3<f32>(0.70, 0.78, 0.85);

    // ── Caustic shimmer ────────────────────────────────────────────────
    let caustic_val = caustic(uv * 0.50, t);
    let caustic_col = vec3<f32>(0.08, 0.30, 0.25) * caustic_val * 0.30;

    // ── Composite ──────────────────────────────────────────────────────
    let base_col = mix(depth_col, reflection, fresnel) + caustic_col;
    let albedo_col = mix(base_col, foam_col, foam_amount * 0.40);

    s.albedo = vec4<f32>(albedo_col, 0.75);
    s.normal = N;

    // ── Subsurface scattering (volumetric light penetration) ───────────
    s.flags = s.flags | SURFACE_FLAG_SUBSURFACE;
    s.subsurface_color  = vec3<f32>(0.030, 0.100, 0.070);
    s.subsurface_radius = 1.50;

    // ── Specular (glossy for sharp SSR reflections) ────────────────────
    s.roughness = 0.012;
    s.metallic  = 0.0;
    s.specular_f0 = mix(vec3<f32>(0.020), vec3<f32>(0.080), fresnel);

    // ── Anisotropic highlight streaks (wind-driven) ────────────────────
    s.roughness_aniso_x = 0.012;
    s.roughness_aniso_y = 0.140;
    s.aniso_rotation = t * 0.040;

    // ── Subtle emissive glow (caustics + foam) ─────────────────────────
    s.emissive = caustic_col * 0.40 + foam_col * foam_amount * 0.08;

    return s;
}