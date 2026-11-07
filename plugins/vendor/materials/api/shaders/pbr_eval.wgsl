//! Shared PBR evaluation functions.
//!
//! Pure BRDF math: no pass-specific types, no bindings.
//! Both the deferred and forward lighting passes include this.

const PI: f32 = 3.14159265359;

fn pow5(x: f32) -> f32 { let x2 = x * x; return x2 * x2 * x; }

// ── Cook-Torrance Normal Distribution Function (GGX / Trowbridge-Reitz) ──────

fn distribution_ggx(N: vec3<f32>, H: vec3<f32>, roughness: f32) -> f32 {
    let a    = roughness * roughness;
    let a2   = a * a;
    let NdH  = max(dot(N, H), 0.0);
    let denom = NdH * NdH * (a2 - 1.0) + 1.0;
    return a2 / (PI * denom * denom + 0.0001);
}

fn distribution_ggx_anisotropic(NdotH: f32, ax: f32, ay: f32, phi_h: f32) -> f32 {
    let cos2_h = NdotH * NdotH;
    let sin2_h = 1.0 - cos2_h;
    let ax2 = ax * ax;
    let ay2 = ay * ay;
    let cos_phi = cos(phi_h);
    let sin_phi = sin(phi_h);
    let denom = cos2_h + sin2_h * (cos_phi * cos_phi / ax2 + sin_phi * sin_phi / ay2);
    return 1.0 / (PI * ax * ay * denom * denom + 0.0001);
}

// ── Geometry masking/shadowing (Smith GGX) ──────────────────────────────────

fn geometry_schlick_ggx(NdotV: f32, roughness: f32) -> f32 {
    let r = roughness + 1.0;
    let k = (r * r) / 8.0;
    return NdotV / (NdotV * (1.0 - k) + k + 0.0001);
}

fn geometry_smith(N: vec3<f32>, V: vec3<f32>, L: vec3<f32>, roughness: f32) -> f32 {
    let NdV = max(dot(N, V), 0.0);
    let NdL = max(dot(N, L), 0.0);
    return geometry_schlick_ggx(NdV, roughness) * geometry_schlick_ggx(NdL, roughness);
}

fn geometry_smith_anisotropic(NdotV: f32, NdotL: f32, ax: f32, ay: f32) -> f32 {
    let Gv = NdotV / (NdotV + sqrt(ax*ax + (1.0 - ax*ax) * NdotV * NdotV) + 0.0001);
    let Gl = NdotL / (NdotL + sqrt(ay*ay + (1.0 - ay*ay) * NdotL * NdotL) + 0.0001);
    return Gv * Gl;
}

fn compute_phi_h(N: vec3<f32>, H: vec3<f32>, T: vec3<f32>, B: vec3<f32>) -> f32 {
    let H_tangent = dot(H, T);
    let H_bitangent = dot(H, B);
    return atan2(H_bitangent, H_tangent);
}

// ── Fresnel (Schlick) ───────────────────────────────────────────────────────

fn fresnel_schlick(cos_theta: f32, F0: vec3<f32>) -> vec3<f32> {
    return F0 + (1.0 - F0) * pow5(clamp(1.0 - cos_theta, 0.0, 1.0));
}

fn fresnel_schlick_roughness(cos_theta: f32, F0: vec3<f32>, roughness: f32) -> vec3<f32> {
    let one_minus_r = vec3<f32>(1.0 - roughness);
    return F0 + (max(one_minus_r, F0) - F0) * pow5(clamp(1.0 - cos_theta, 0.0, 1.0));
}

// ── IBL split-sum DFG approximation (Lazarov analytic fit) ──────────────────

fn env_brdf_approx(NdotV: f32, roughness: f32) -> vec2<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r  = roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * NdotV)) * r.x + r.y;
    return vec2<f32>(-1.04, 1.04) * a004 + r.zw;
}

// ── Surface flags (packed into gbuf_extra.a) ────────────────────────────────

const SURFACE_FLAG_SUBSURFACE: u32 = 1u << 0u;
const SURFACE_FLAG_ANISOTROPIC: u32 = 1u << 1u;
const SURFACE_FLAG_LOW_SPECULAR: u32 = 1u << 2u;
// Two-sided foliage: light from behind the surface passes through it,
// tinted by gbuf_sss.rgb (the transmitted share of the light, by colour;
// what lies between the light and the surface is the light's visibility).
const SURFACE_FLAG_TWO_SIDED: u32 = 1u << 3u;

// Highest mip index in the pre-filtered reflection chain (roughness 1.0 maps here).
const ENV_MAX_LOD: f32 = 8.0;
