// 3D hash: per-component sinusoidal
fn hash3(p: vec3<f32>) -> vec3<f32> {
    let s = p * 127.1 + vec3<f32>(311.7, 74.7, 237.5);
    return fract(sin(s) * 43758.5453);
}

// 3D Voronoi (cellular) noise returning feature-point color
fn voronoi3(p: vec3<f32>) -> vec3<f32> {
    let cell = floor(p);
    let frac = p - cell;
    var min_d2 = 10.0;
    var feat = vec3<f32>(0.0);
    for (var i = -1i; i <= 1i; i++) {
        for (var j = -1i; j <= 1i; j++) {
            for (var k = -1i; k <= 1i; k++) {
                let n = cell + vec3<f32>(f32(i), f32(j), f32(k));
                let o = hash3(n);
                let d = n + o - p;
                let d2 = dot(d, d);
                if d2 < min_d2 {
                    min_d2 = d2;
                    feat = o;
                }
            }
        }
    }
    return feat;
}

// Spectral opal color palette: blue → purple → teal → pink
fn opal_spectrum(t: f32) -> vec3<f32> {
    let a = vec3<f32>(0.08, 0.15, 0.90); // deep blue
    let b = vec3<f32>(0.55, 0.10, 0.85); // purple
    let c = vec3<f32>(0.10, 0.80, 0.60); // teal
    let d = vec3<f32>(0.75, 0.20, 0.50); // pink
    let e = vec3<f32>(0.08, 0.15, 0.90); // back to blue
    let phases = vec4<f32>(t, t - 1.0, t - 2.0, t - 3.0);
    let w = clamp(phases, vec4<f32>(0.0), vec4<f32>(1.0));
    let w2 = w * w * (3.0 - 2.0 * w);
    return mix(mix(mix(mix(a, b, w2.x), c, w2.y), d, w2.z), e, w2.w);
}

fn radiant_eval_surface(material: GpuMaterial,
                        material_tex: MaterialTextureData,
                        input: MaterialInput) -> SurfaceData {
    var s = default_pbr_surface(material, material_tex, input);

    let V = normalize(input.view_position - input.world_position);
    let R = reflect(-V, s.normal);
    let NdV = max(dot(s.normal, V), 0.0);

    // class_params: x = patch_scale, y = chroma_strength, z = depth_fade, w = unused
    let patch_scale = material.class_params.x;
    let chroma_strength = material.class_params.y;
    let depth_fade = material.class_params.z;

    // Sample position: world pos + view reflection offset for play-of-color shift
    let p = input.world_position * max(patch_scale, 0.5) + R * depth_fade;

    // Multi-octave voronoi for rich internal 3D structure
    let c1 = voronoi3(p);
    let c2 = voronoi3(p * 0.6 + 1.7);
    let c3 = voronoi3(p * 1.8 + 4.3);

    // Blend octaves: weighted sum of feature colors
    let blend = (c1 + c2 * 0.5 + c3 * 0.25) / (1.0 + 0.5 + 0.25);
    let hue = dot(blend, vec3<f32>(0.6, 0.3, 0.1)) * 4.0;

    // Opal play-of-color from the spectral palette
    let play = opal_spectrum(hue) * chroma_strength;

    // Milky translucent body with play-of-color layered on top
    let milky = vec3<f32>(0.92, 0.87, 0.80);
    s.albedo = vec4<f32>(mix(milky, play, 0.85), s.albedo.a);

    // Enable subsurface scattering — light scatters through the milky body
    s.flags = s.flags | SURFACE_FLAG_SUBSURFACE;
    s.subsurface_color = vec3<f32>(0.95, 0.90, 0.85);
    s.subsurface_radius = 0.6;

    // Subtle emission on the colored patches for internal glow
    s.emissive = play * 0.6;

    // Glassy surface with low roughness
    s.roughness = 0.03;
    s.metallic = 0.0;
    s.specular_f0 = vec3<f32>(0.04);

    // Anisotropic sheen that rotates with the pattern
    s.roughness_aniso_x = 0.03;
    s.roughness_aniso_y = 0.08;
    s.aniso_rotation = hue * 6.28;

    return s;
}