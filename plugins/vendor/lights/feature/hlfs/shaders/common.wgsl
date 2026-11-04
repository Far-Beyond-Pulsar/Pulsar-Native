//!use helio_prelude
// Camera and the depth convention come from the prelude. GpuLight is the
// GPU-derived 64-byte prefix of SceneDB's 128-byte light row; no other light
// fields are read here.
struct GpuLight {
    position_range: vec4<f32>, direction_outer: vec4<f32>, color_intensity: vec4<f32>,
    shadow_index: u32, light_type: u32, inner_angle: f32, _pad: u32,
}
struct Globals {
    frame: u32, sample_count: u32, light_count: u32, history_valid: u32,
    screen_size: vec2<u32>, sample_size: vec2<u32>,
    sample_scale: u32, candidate_count: u32, has_velocity: u32, surface_flags: u32,
    max_history: f32, discovery_fraction: f32, exposure: f32, debug_mode: u32,
    ambient: vec4<f32>, csm_splits: vec4<f32>,
    previous_view: mat4x4<f32>,
    ray_settings: vec4<f32>,
    inverse_view: mat4x4<f32>,
    inverse_projection: mat4x4<f32>,
}
@group(0) @binding(0) var<uniform> globals: Globals;
@group(0) @binding(1) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(2) var<storage, read> lights: array<GpuLight>;
@group(0) @binding(7) var blue_noise: texture_2d_array<f32>;
// Live light rows in row order (compact_lights.wgsl `list_live`): [0] is the
// count, [1+k] the row of the k-th live light. `globals.light_count` stays
// the allocated row count and bounds light IDs; loops over the population
// and uniform light picks use this list (Pulsar-Native#838).
@group(0) @binding(11) var<storage, read> live_lights: array<u32>;
fn live_light_count() -> u32 { return live_lights[0]; }
fn live_light(k: u32) -> u32 { return live_lights[k+1u]; }

@group(1) @binding(0) var gbuf_albedo: texture_2d<f32>;
@group(1) @binding(1) var gbuf_normal: texture_2d<f32>;
@group(1) @binding(2) var gbuf_orm: texture_2d<f32>;
@group(1) @binding(3) var gbuf_emissive: texture_2d<f32>;
@group(1) @binding(4) var gbuf_depth: texture_depth_2d;
@group(1) @binding(5) var gbuf_lightmap_uv: texture_2d<f32>;
@group(1) @binding(6) var baked_lightmap: texture_2d<f32>;
@group(1) @binding(7) var lightmap_sampler: sampler;
@group(1) @binding(8) var pre_aa_texture: texture_2d<f32>;
@group(1) @binding(9) var gbuf_velocity: texture_2d<f32>;
// The GI probes' irradiance / pi at each pixel (rgb, half floats) and how
// much the probes hold it (the fourth half); a single pair without them.
@group(1) @binding(10) var<storage, read> gi_irradiance: array<vec2<u32>>;

const PI: f32 = 3.14159265359;
const INVALID_LIGHT: u32 = 0xffffffffu;
const ENABLE_SHADOWS: bool = true;
const NORMAL_OFFSET_SCALE: f32 = 0.01;
const ATLAS_SIZE: f32 = 1024.0;
const TILE_SIZE: u32 = 8u;
const COARSE_TILE_SIZE: u32 = 64u;
const GRID_CAPACITY: u32 = 64u;
const COARSE_CAPACITY: u32 = 256u;
const VISIBLE_CAPACITY: u32 = 16u;

struct LightTile { count: u32, has_directional: u32, indices: array<u32, 32>, }
struct CoarseTile { count: u32, has_directional: u32, indices: array<u32, 128>, }
struct VisibleTile { count: u32, indices: array<u32, 16>, confidence_low: u32, confidence_high: u32, }

fn div_ceil(n: vec2<u32>, d: u32) -> vec2<u32> { return (n + d - 1u) / d; }
fn luminance(v: vec3<f32>) -> f32 { return dot(v, vec3<f32>(0.2126, 0.7152, 0.0722)); }
fn safe_normalize(v: vec3<f32>) -> vec3<f32> { return v * inverseSqrt(max(dot(v, v), 1e-20)); }
fn pow5(x: f32) -> f32 { let x2 = x*x; return x2*x2*x; }
fn hash_u32(value: u32) -> u32 {
    var x = value; x = (x ^ (x >> 16u)) * 0x7feb352du;
    x = (x ^ (x >> 15u)) * 0x846ca68bu; return x ^ (x >> 16u);
}
fn random(state: ptr<function, u32>) -> f32 {
    *state = hash_u32(*state + 0x9e3779b9u);
    return f32(*state >> 8u) * (1.0 / 16777216.0);
}
fn stbn(pixel: vec2<u32>, dimension: u32) -> f32 {
    let p = (pixel + vec2<u32>(37u, 17u) * dimension) % textureDimensions(blue_noise);
    let z = (globals.frame + dimension * 19u) % textureNumLayers(blue_noise);
    // Quantized ranks are bin centers: never return 0 or 1 to the reservoir.
    return (textureLoad(blue_noise, vec2<i32>(p), i32(z), 0).r * 255.0 + 0.5) / 256.0;
}
fn sample_pixel(p: vec2<u32>, frame: u32) -> vec2<u32> {
    // Native shading has no four-rooks offset. Avoid phase and modulo work
    // in the sampler and in every temporal neighborhood load.
    if globals.sample_scale == 1u { return min(p, globals.screen_size - 1u); }
    // Four-rooks over the 2x2 block. Half resolution visits every full-res pixel.
    let phase = (frame + (p.x & 1u) + 2u * (p.y & 1u)) & 3u;
    let offset = vec2<u32>(phase & 1u, phase >> 1u) % globals.sample_scale;
    return min(p * globals.sample_scale + offset, globals.screen_size - 1u);
}
fn sample_position_from_uv(uv: vec2<f32>) -> vec2<f32> {
    // Sample blocks have a fixed full-resolution size. Ceil-rounded texture
    // extents must not stretch reprojection when an output dimension is odd.
    return uv*vec2<f32>(globals.screen_size)/f32(globals.sample_scale);
}
fn world_position(pixel: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc=pixel/vec2<f32>(globals.screen_size)*vec2<f32>(2.0,-2.0)+vec2<f32>(-1.0,1.0);
    let p=cameras[0].proj;
    var view_position: vec3<f32>;
    // For a standard perspective matrix, solve depth before applying the view
    // transform. Subtracting depth from its projection coefficient avoids the
    // cancellation in inverse_projection * clip (two large reciprocal terms).
    // Jitter/off-centre perspective is supported through p[2].xy.
    if abs(p[2].w)==1.0 && p[3].w==0.0 && p[0].y==0.0 && p[1].x==0.0
        && p[0].z==0.0 && p[1].z==0.0 && p[0].w==0.0 && p[1].w==0.0
        && p[3].x==0.0 && p[3].y==0.0 {
        var z=p[3].z/(p[2].w*depth-p[2].z);
        if globals.has_velocity!=0u {
            let motion=textureLoad(gbuf_velocity,vec2<i32>(pixel),0);
            if motion.w==2.0 { z+=motion.z; }
        }
        view_position=vec3<f32>((p[2].w*ndc-p[2].xy)*z/vec2<f32>(p[0].x,p[1].y),z);
    } else {
        let h=globals.inverse_projection*vec4<f32>(ndc,depth,1.0);
        view_position=h.xyz/h.w;
    }
    let world=globals.inverse_view*vec4<f32>(view_position,1.0);
    return world.xyz/world.w;
}
fn previous_uv(pixel: vec2<u32>, position: vec3<f32>) -> vec2<f32> {
    if globals.has_velocity != 0u {
        return (vec2<f32>(pixel) + 0.5 - textureLoad(gbuf_velocity, vec2<i32>(pixel), 0).xy) / vec2<f32>(globals.screen_size);
    }
    let h = cameras[0].prev_view_proj * vec4<f32>(position, 1.0);
    if h.w <= 0.0 { return vec2<f32>(-1.0); }
    return h.xy / h.w * vec2<f32>(0.5, -0.5) + 0.5;
}
fn oct_encode(normal: vec3<f32>) -> vec2<f32> {
    let n = normal / max(dot(abs(normal), vec3<f32>(1.0)), 1e-8);
    let s = select(vec2<f32>(-1.0), vec2<f32>(1.0), n.xy >= vec2<f32>(0.0));
    return select((1.0 - abs(n.yx)) * s, n.xy, n.z >= 0.0);
}
fn oct_decode(e: vec2<f32>) -> vec3<f32> {
    var n = vec3<f32>(e, 1.0 - abs(e.x) - abs(e.y));
    let t = max(-n.z, 0.0);
    n.x += select(t, -t, n.x >= 0.0); n.y += select(t, -t, n.y >= 0.0);
    return safe_normalize(n);
}
fn geometry_matches(g: vec4<f32>, normal: vec3<f32>, view_depth: f32) -> bool {
    return g.w > 0.0 && dot(oct_decode(g.xy), normal) > 0.9 && abs(exp2(g.z) - view_depth) < max(0.02, abs(view_depth) * 0.01);
}
fn finite_color(v: vec3<f32>) -> vec3<f32> {
    // Keep pre-exposed lighting finite within the packed HDR representation.
    return min(select(vec3<f32>(0.0), v, v >= vec3<f32>(0.0)), vec3<f32>(60000.0));
}
fn normal_weight(value: f32) -> f32 {
    let x=max(value,0.0);
    let p2=x*x; let p4=p2*p2; let p8=p4*p4; let p16=p8*p8;
    return p16*p16;
}

// Unsigned floats use five exponent bits and five or six mantissa bits.
// Stochastic rounding preserves the expectation between adjacent values,
// including the subnormal interval, instead of accumulating truncation bias.
fn pack_ufloat(value: f32, mantissa_bits: u32, noise: f32) -> u32 {
    let v=clamp(value,0.0,60000.0);
    let step=exp2(max(floor(log2(max(v,exp2(-14.0)))),-14.0)-f32(mantissa_bits));
    let rounded=floor(v/step+noise)*step;
    if rounded<exp2(-14.0) { return u32(rounded*exp2(14.0+f32(mantissa_bits))); }
    return (bitcast<u32>(rounded)>>(23u-mantissa_bits))-(112u<<mantissa_bits);
}
fn unpack_ufloat(bits: u32, mantissa_bits: u32) -> f32 {
    let exponent=bits>>mantissa_bits;
    let mantissa=bits&((1u<<mantissa_bits)-1u);
    if exponent==0u { return f32(mantissa)*exp2(-14.0-f32(mantissa_bits)); }
    return bitcast<f32>((bits+(112u<<mantissa_bits))<<(23u-mantissa_bits));
}
fn pack_radiance(color: vec3<f32>, pixel: vec2<u32>, dimension: u32) -> vec4<u32> {
    let c=finite_color(color);
    return vec4<u32>(pack_ufloat(c.r,6u,stbn(pixel,dimension)) |
        (pack_ufloat(c.g,6u,stbn(pixel,dimension+1u))<<11u) |
        (pack_ufloat(c.b,5u,stbn(pixel,dimension+2u))<<22u),0u,0u,0u);
}
fn load_radiance(signal: texture_2d<u32>, pixel: vec2<i32>, component: u32) -> vec3<f32> {
    let bits=textureLoad(signal,pixel,0)[component];
    return vec3<f32>(unpack_ufloat(bits&2047u,6u),unpack_ufloat((bits>>11u)&2047u,6u),unpack_ufloat(bits>>22u,5u));
}
// Squared luminance needs six exponent bits. E6M5 covers the full squared
// radiance range while retaining small moments without a shared scale factor.
fn pack_moment(value: f32, noise: f32) -> u32 {
    let v=clamp(value,0.0,3600000000.0);
    let step=exp2(max(floor(log2(max(v,exp2(-30.0)))),-30.0)-5.0);
    let rounded=floor(v/step+noise)*step;
    if rounded<exp2(-30.0) { return u32(rounded*exp2(35.0)); }
    return (bitcast<u32>(rounded)>>18u)-(96u<<5u);
}
fn unpack_moment(bits: u32) -> f32 {
    if (bits>>5u)==0u { return f32(bits&31u)*exp2(-35.0); }
    return bitcast<f32>((bits+(96u<<5u))<<18u);
}
// Two words hold an octahedral normal, logarithmic depth, both luminance
// second moments and age. The first moments are derived from signal RGB.
fn pack_geometry(geometry: vec4<f32>, moments: vec2<f32>, pixel: vec2<u32>, confidence: bool) -> vec4<u32> {
    let normal=pack4x8snorm(vec4<f32>(geometry.xy,0.0,0.0))&65535u;
    let depth=pack2x16float(vec2<f32>(geometry.z,0.0))&65535u;
    let variance=pack_moment(moments.x,stbn(pixel,17u)) |
        (pack_moment(moments.y,stbn(pixel,18u))<<11u);
    return vec4<u32>(normal|(depth<<16u),variance|(u32(geometry.w)<<22u)|select(0u,1u<<28u,confidence),0u,0u);
}
fn load_geometry(signal: texture_2d<u32>, pixel: vec2<i32>) -> vec4<f32> {
    let bits=textureLoad(signal,pixel,0).rg;
    return vec4<f32>(unpack4x8snorm(bits.x&65535u).xy,unpack2x16float(bits.x>>16u).x,f32((bits.y>>22u)&63u));
}
fn load_moments(signal: texture_2d<u32>, pixel: vec2<i32>) -> vec2<f32> {
    let bits=textureLoad(signal,pixel,0).g;
    return vec2<f32>(unpack_moment(bits&2047u),unpack_moment((bits>>11u)&2047u));
}

struct LightProposal {
    id: u32, inverse_probability: f32, alias_index: u32, alias_probability: f32,
    total_weight: f32, key_light: u32,
    // The final array element is a control record. There, id and alias_index
    // are the light-set stamp and inverse_probability stores active light count.
    // Exact weights for the four IDs in this 256-wide stratum when the light
    // population is at most 1,024. The alias table selects a stratum, then
    // sampling chooses one of its IDs with its actual conditional probability.
    weights: array<f32,4>,
}
fn proposal_weight(light: GpuLight, center: vec3<f32>) -> f32 {
    let power=luminance(max(light.color_intensity.rgb*light.color_intensity.w,vec3<f32>(0.0)));
    if power<=0.0 { return 0.0; }
    let delta=light.position_range.xyz-center;
    let attenuation=select(1.0/max(dot(delta,delta),0.0001),1.0,light.light_type==0u);
    return clamp(power*attenuation,1e-20,1e20);
}
