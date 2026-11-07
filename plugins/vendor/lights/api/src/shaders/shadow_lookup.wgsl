// ── Cascades and projection ─────────────────────────────────────────────────
// The math of a shadow lookup outside the lights' own passes (a medium's
// froxels, a lens's occlusion test); the texture sample is the budgeted
// atlas's (the `shadow_atlas` snippet). Pure functions, so the bindings stay the
// caller's. Over the prelude (`helio_ndc_to_uv`).

/// Which cascade(s) a view distance falls in, and how much to blend between them.
struct ShadowCsmSelection {
    /// Primary cascade index (0..3).
    cascade_a: u32,
    /// Cascade to blend toward; equals `cascade_a` outside a blend zone.
    cascade_b: u32,
    /// 0 = use `cascade_a` alone, >0 = mix toward `cascade_b`.
    blend: f32,
}

/// Fraction of a split distance spent cross-fading between adjacent cascades.
/// Without it, a cascade boundary is a hard line across the floor.
const SHADOW_CSM_BLEND_ZONE: f32 = 0.1;

/// Pick CSM cascades for a view distance.
///
/// `splits` is `Globals.csm_splits`: xyz are the three cascade boundaries
/// (cascade 3 runs from splits.z to the far plane). `view_dist` is the distance
/// from the camera to the shaded point, not its depth-buffer value.
fn shadow_csm_select(view_dist: f32, splits: vec4<f32>) -> ShadowCsmSelection {
    var s: ShadowCsmSelection;
    s.cascade_a = 3u;
    s.cascade_b = 3u;
    s.blend = 0.0;

    let half_zone = SHADOW_CSM_BLEND_ZONE * 0.5;

    if view_dist < splits.x * (1.0 - half_zone) {
        s.cascade_a = 0u;
    } else if view_dist < splits.x * (1.0 + half_zone) {
        s.cascade_a = 0u;
        s.cascade_b = 1u;
        s.blend = smoothstep(splits.x * (1.0 - half_zone), splits.x * (1.0 + half_zone), view_dist);
    } else if view_dist < splits.y * (1.0 - half_zone) {
        s.cascade_a = 1u;
    } else if view_dist < splits.y * (1.0 + half_zone) {
        s.cascade_a = 1u;
        s.cascade_b = 2u;
        s.blend = smoothstep(splits.y * (1.0 - half_zone), splits.y * (1.0 + half_zone), view_dist);
    } else if view_dist < splits.z * (1.0 - half_zone) {
        s.cascade_a = 2u;
    } else if view_dist < splits.z * (1.0 + half_zone) {
        s.cascade_a = 2u;
        s.cascade_b = 3u;
        s.blend = smoothstep(splits.z * (1.0 - half_zone), splits.z * (1.0 + half_zone), view_dist);
    }

    return s;
}

/// A world position projected into a shadow map.
struct ShadowProjection {
    /// Atlas UV (y down), only meaningful when `valid`.
    uv: vec2<f32>,
    /// [0,1] depth to compare against, only meaningful when `valid`.
    depth: f32,
    /// False when the point is behind the light or outside the map — callers
    /// must treat that as *lit*, not shadowed, or geometry outside the cascade
    /// goes black.
    valid: bool,
}

/// Project a world position into a shadow map's UV + depth.
///
/// `light_view_proj` is `shadow_matrices[layer].mat`. The y-flip goes through
/// `helio_ndc_to_uv` for the same reason it does everywhere else — shadow UV is
/// framebuffer-oriented (y down) while NDC is y up.
fn shadow_project(light_view_proj: mat4x4<f32>, world_pos: vec3<f32>) -> ShadowProjection {
    var s: ShadowProjection;
    s.uv = vec2<f32>(0.0);
    s.depth = 0.0;
    s.valid = false;

    let clip = light_view_proj * vec4<f32>(world_pos, 1.0);
    // Behind the light's near plane: the perspective divide would mirror the
    // point back into frame and shadow it against unrelated depths.
    if clip.w <= 0.0 { return s; }

    let ndc = clip.xyz / clip.w;
    let uv = helio_ndc_to_uv(ndc.xy);
    if any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || ndc.z < 0.0 || ndc.z > 1.0 {
        return s;
    }

    s.uv = uv;
    s.depth = ndc.z;
    s.valid = true;
    return s;
}
