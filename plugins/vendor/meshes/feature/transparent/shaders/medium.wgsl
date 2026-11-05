
//!use frame_media
// Pass-owned view effect: each fragment seen through the frame's medium at
// its own depth (the grid's mapping and composite are the frame's
// `frame_media`). Camera/material template remains independent of media.
struct TransparentMediumParameters {
    first: vec4<f32>,
    distances: vec4<f32>,
    albedo: vec4<f32>,
    emission: vec4<f32>,
}
@group(2) @binding(0) var integrated_medium: texture_3d<f32>;
@group(2) @binding(1) var medium_sampler: sampler;
@group(2) @binding(2) var<uniform> medium_parameters: TransparentMediumParameters;

fn apply_medium(surface: vec4<f32>, p: vec3<f32>) -> vec4<f32> {
    let far = medium_parameters.distances.y;
    if far <= MEDIA_NEAR { return surface; }
    let clip = cameras[0].view_proj * vec4<f32>(p, 1.0);
    // view_proj is jittered; the froxel grid is not.
    let uv = (clip.xy / clip.w - cameras[0].jitter_frame.xy) * vec2<f32>(0.5, -0.5) + 0.5;
    let depth = dot(p - cameras[0].position_near.xyz, normalize(cameras[0].forward_far.xyz));
    if depth <= MEDIA_NEAR { return surface; }
    let slice = media_grid_w(depth, far, textureDimensions(integrated_medium).z);
    let fog = textureSampleLevel(integrated_medium, medium_sampler, vec3<f32>(uv, slice), 0.0);
    // SrcAlpha blending with the fogged background counts foreground scattering
    // once: alpha*S + (1-alpha)*S = S, with alpha*T*surface radiance.
    return vec4<f32>(media_seen(surface.rgb, fog), surface.a);
}
