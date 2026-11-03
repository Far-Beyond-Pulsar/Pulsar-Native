//!use helio_prelude

// ── Participating-medium composite ────────────────────────────────────────────
//
// Applies VolumetricFogPass's integrated froxel grid to scene-linear opaque HDR:
//
//     L_camera = T(0,d) * L_surface + S(0,d)
//
// where the grid stores S (in-scattered radiance) in rgb and T (transmittance)
// in alpha, cumulative from the camera to each slice's far face. This runs
// before transparency, anti-aliasing, exposure metering, bloom and lens
// response, so all of them see the shafts' energy exactly once. It is
// ungated: a disabled or empty medium is (0,0,0,1) and the result is the input.

// Byte-identical to helio_pass_postprocess::GpuFogUniforms. Only the resolved
// integration range (byte 20) is consumed; the enabled/density fields describe
// the legacy adapter and must not gate compositing of native media.
struct FogParameters {
    enabled:        u32,
    mode:           u32,
    density:        f32,
    height_falloff: f32,
    start_distance: f32,
    max_distance:   f32,
    height:         f32,
    anisotropy:     f32,
    color:          vec3<f32>,
    _pad_color:     f32,
    emissive:       vec3<f32>,
    _pad_emissive:  f32,
}

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<uniform> fog_parameters: FogParameters;
@group(0) @binding(2) var scene_color: texture_2d<f32>;
@group(0) @binding(3) var scene_depth: texture_depth_2d;
@group(0) @binding(4) var integrated_medium: texture_3d<f32>;
@group(0) @binding(5) var medium_sampler: sampler;

struct VOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
}

@vertex
fn vs_fullscreen(@builtin(vertex_index) vi: u32) -> VOut {
    let x = f32((vi << 1u) & 2u);
    let y = f32(vi & 2u);
    var out: VOut;
    out.pos = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

// Cubic B-spline across the screen axes of the froxel grid, as four trilinear
// taps. Froxels span many pixels at high resolutions; linear reconstruction
// then shows the grid as diamonds and creases along shafts. Depth stays
// linear: a wider depth kernel would pull fog from behind the surface forward.
fn sample_medium(uv: vec2<f32>, slice: f32) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(integrated_medium).xy);
    let t = uv * dims - 0.5;
    let base = floor(t);
    let f = t - base;
    let f2 = f * f;
    let f3 = f2 * f;
    let w0 = (1.0 - f) * (1.0 - f) * (1.0 - f) / 6.0;
    let w1 = (4.0 - 6.0 * f2 + 3.0 * f3) / 6.0;
    let w3 = f3 / 6.0;
    let g0 = w0 + w1;
    let g1 = 1.0 - g0;
    let c0 = (base - 1.0 + w1 / g0 + 0.5) / dims;
    let c1 = (base + 1.0 + w3 / max(g1, vec2<f32>(1e-6)) + 0.5) / dims;
    return g0.y * (g0.x * textureSampleLevel(integrated_medium, medium_sampler, vec3<f32>(c0.x, c0.y, slice), 0.0)
                 + g1.x * textureSampleLevel(integrated_medium, medium_sampler, vec3<f32>(c1.x, c0.y, slice), 0.0))
         + g1.y * (g0.x * textureSampleLevel(integrated_medium, medium_sampler, vec3<f32>(c0.x, c1.y, slice), 0.0)
                 + g1.x * textureSampleLevel(integrated_medium, medium_sampler, vec3<f32>(c1.x, c1.y, slice), 0.0));
}

@fragment
fn fs_composite(in: VOut) -> @location(0) vec4<f32> {
    let pixel = vec2<i32>(in.pos.xy);
    let source = textureLoad(scene_color, pixel, 0);
    let color = source.rgb;
    let far = fog_parameters.max_distance;
    // Zero range: no fog producer (neutral fallback buffer). Pass through.
    if !(far > HELIO_FROXEL_NEAR) {
        return vec4<f32>(color, source.a);
    }
    let depth_dims = vec2<f32>(textureDimensions(scene_depth));
    let depth_pixel = vec2<i32>(in.uv * depth_dims);
    let raw_depth = textureLoad(scene_depth, depth_pixel, 0);
    // Slices are planes of constant view depth: convert the buffer value, do
    // not use radial distance.
    let view_depth = helio_view_depth(cameras[0].proj, raw_depth);
    // Integration stores each slice's cumulative value at its far face, not
    // its centre; offset by half a texel for texel-centre sampling.
    let slice = clamp(
        helio_froxel_slice_from_view_depth(view_depth, far)
            - 0.5 / f32(textureDimensions(integrated_medium).z),
        0.0,
        1.0,
    );
    // The froxel grid is unjittered; this pixel's ray is the jittered one.
    let grid_uv = in.uv - cameras[0].jitter_frame.xy * vec2<f32>(0.5, -0.5);
    let medium = sample_medium(grid_uv, slice);
    // Beer-Lambert attenuation of the surface plus scattered radiance. HDR is
    // preserved; exposure and tone mapping happen once, later.
    return vec4<f32>(color * clamp(medium.a, 0.0, 1.0) + max(medium.rgb, vec3<f32>(0.0)), source.a);
}
