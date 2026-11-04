/// GPU shadow matrix computation
///
/// Computes shadow light-space matrices entirely on GPU to eliminate CPU overhead.
/// One thread per light; each light can output 1-6 matrices depending on type:
///   - Point lights: 6 cube-face matrices (±X, ±Y, ±Z)
///   - Directional lights: 4 CSM cascade matrices
///   - Spot lights: 1 perspective matrix
///
/// Integrates with GPU indirect dispatch system - runs before shadow pass.

// ── Constants matching shadow_math.rs ─────────────────────────────────────────

const FACES_PER_LIGHT: u32 = 6u;
const CSM_SPLITS: vec4f = vec4f(16.0, 80.0, 300.0, 1400.0);
const SCENE_DEPTH: f32 = 4000.0;

// Light types (must match LightType in libhelio/src/light.rs)
const LIGHT_TYPE_DIRECTIONAL: u32 = 0u;
const LIGHT_TYPE_POINT: u32 = 1u;
const LIGHT_TYPE_SPOT: u32 = 2u;

// ── Input/Output structs ──────────────────────────────────────────────────────

/// Must match GpuLight in libhelio/src/light.rs (64 bytes)
struct GpuLight {
    position_range:   vec4f,  // xyz = position, w = range
    direction_outer:  vec4f,  // xyz = direction, w = cos(outer_angle)
    color_intensity:  vec4f,  // xyz = color, w = intensity
    shadow_index:     u32,    // u32::MAX = no shadow, otherwise shadow matrix base index
    light_type:       u32,    // 0=Directional, 1=Point, 2=Spot
    inner_angle:      f32,    // cos(inner_angle) for spot lights
    _pad:             u32,
    // Light shafts — consumed by helio-pass-volumetric-fog. Padding is three
    // scalars, not vec3<u32>, to keep the struct at 96 bytes (vec3 aligns to 16).
    god_rays_enabled:  u32,
    god_rays_density:  f32,
    god_rays_weight:   f32,
    god_rays_decay:    f32,
    god_rays_exposure: f32,
    flare_enabled:      u32,
    flare_type:         u32,
    flare_intensity:    f32,
    flare_scale:        f32,
    flare_tint_r:       f32,
    flare_tint_g:       f32,
    flare_tint_b:       f32,
    ies_profile_index:    i32,
    light_function_index: i32,
    ies_angle_scale:      f32,
    ies_angle_offset:     f32,
}

/// Must match GpuShadowMatrix in uniforms.rs (64 bytes)
struct GpuShadowMatrix {
    mat: mat4x4f,
    atlas: vec4f,
    policy: vec4u,
}

/// Camera data for CSM cascade computation.
/// Layout must match GpuCameraUniforms in libhelio/src/camera.rs (256 bytes).
struct CameraUniforms {
    view:           mat4x4f,   // offset   0
    proj:           mat4x4f,   // offset  64
    view_proj:      mat4x4f,   // offset 128
    inv_view_proj:  mat4x4f,   // offset 192
    position_near:  vec4f,     // offset 256 — xyz = world pos, w = near plane
    forward_far:    vec4f,     // offset 272
    jitter_frame:   vec4f,     // offset 288
    prev_view_proj: mat4x4f,   // offset 304
}

struct ShadowMatrixParams {
    light_count: u32,
    shadow_atlas_size: u32,
    _pad0: u32,
    _pad1: u32,
}

// ── Bindings ──────────────────────────────────────────────────────────────────

@group(0) @binding(0) var<storage, read>       lights:         array<GpuLight>;
@group(0) @binding(1) var<storage, read_write> shadow_mats:    array<GpuShadowMatrix>;
@group(0) @binding(2) var<storage, read> cameras: array<CameraUniforms, 2>;
@group(0) @binding(3) var<uniform>             params:         ShadowMatrixParams;
@group(0) @binding(4) var<storage, read_write> shadow_dirty:   array<atomic<u32>>;  // Atomic dirty flags per caster slot
@group(0) @binding(5) var<storage, read_write> shadow_hashes:  array<u32>;  // FNV hashes to detect changes

// ── Matrix math helpers ───────────────────────────────────────────────────────

const PI: f32 = 3.14159265359;
const FRAC_PI_2: f32 = 1.57079632679;

/// Build perspective projection matrix (RH, depth [0,1])
fn mat4_perspective_rh(fovy: f32, aspect: f32, near: f32, far: f32) -> mat4x4f {
    let f = 1.0 / tan(fovy * 0.5);
    let nf = 1.0 / (near - far);
    return mat4x4f(
        vec4f(f / aspect, 0.0, 0.0, 0.0),
        vec4f(0.0, f, 0.0, 0.0),
        vec4f(0.0, 0.0, far * nf, -1.0),
        vec4f(0.0, 0.0, near * far * nf, 0.0),
    );
}

/// Build orthographic projection matrix (RH, depth [0,1])
fn mat4_orthographic_rh(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> mat4x4f {
    let rml = 1.0 / (right - left);
    let tmb = 1.0 / (top - bottom);
    let fmn = 1.0 / (far - near);
    return mat4x4f(
        vec4f(2.0 * rml, 0.0, 0.0, 0.0),
        vec4f(0.0, 2.0 * tmb, 0.0, 0.0),
        // RH view space looks down -z: depth = (-z - near) / (far - near), so
        // the z scale is 1 / (near - far). A positive scale put every point
        // at depth ~ -0.5, outside [0,1], and no directional shadow rendered.
        vec4f(0.0, 0.0, -fmn, 0.0),
        vec4f(-(right + left) * rml, -(top + bottom) * tmb, -near * fmn, 1.0),
    );
}

/// Build look-at view matrix (RH)
fn mat4_look_at_rh(eye: vec3f, center: vec3f, up: vec3f) -> mat4x4f {
    let f = normalize(center - eye);
    let s = normalize(cross(f, up));
    let u = cross(s, f);
    return mat4x4f(
        vec4f(s.x, u.x, -f.x, 0.0),
        vec4f(s.y, u.y, -f.y, 0.0),
        vec4f(s.z, u.z, -f.z, 0.0),
        vec4f(-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0),
    );
}

// ── Point light matrices (6 cube faces) ───────────────────────────────────────

fn compute_point_light_matrices(light_idx: u32, position: vec3f, range: f32) {
    let base = lights[light_idx].shadow_index;
    // Extend far plane to ensure full spherical coverage
    // With 90° FOV, worst case is corners at sqrt(3) * range from light center
    let far_plane = max(range, 0.1) * 2.5;  // 2.5x provides full coverage with margin
    let proj = mat4_perspective_rh(FRAC_PI_2, 1.0, 0.05, far_plane);

    let views = array<mat4x4f, 6>(
        mat4_look_at_rh(position, position + vec3f(1.0, 0.0, 0.0),  vec3f(0.0, -1.0, 0.0)),  // +X
        mat4_look_at_rh(position, position + vec3f(-1.0, 0.0, 0.0), vec3f(0.0, -1.0, 0.0)),  // -X
        mat4_look_at_rh(position, position + vec3f(0.0, 1.0, 0.0),  vec3f(0.0, 0.0, 1.0)),   // +Y
        mat4_look_at_rh(position, position + vec3f(0.0, -1.0, 0.0), vec3f(0.0, 0.0, -1.0)),  // -Y
        mat4_look_at_rh(position, position + vec3f(0.0, 0.0, 1.0),  vec3f(0.0, -1.0, 0.0)),  // +Z
        mat4_look_at_rh(position, position + vec3f(0.0, 0.0, -1.0), vec3f(0.0, -1.0, 0.0)),  // -Z
    );

    for (var i = 0u; i < 6u; i++) {
        shadow_mats[base + i].mat = proj * views[i];
    }
}

// ── Spot light matrix (single perspective) ────────────────────────────────────

fn compute_spot_matrix(light_idx: u32, position: vec3f, direction: vec3f, range: f32, cos_outer: f32) {
    let base = lights[light_idx].shadow_index;
    let dir = normalize(direction);

    // Outer angle from cos(outer) → fov = 2 * acos(cos_outer), clamped to [45°, 179°]
    let outer_angle = acos(cos_outer);
    let fov = clamp(outer_angle * 2.0, PI * 0.25, PI - 0.01);

    let up = select(vec3f(0.0, 0.0, 1.0), vec3f(0.0, 1.0, 0.0), abs(dot(dir, vec3f(0.0, 1.0, 0.0))) < 0.99);
    let view = mat4_look_at_rh(position, position + dir, up);
    let proj = mat4_perspective_rh(fov, 1.0, 0.05, max(range, 0.1));

    shadow_mats[base].mat = proj * view;
}

// ── Directional light cascades (CSM with sphere-fit + texel snap) ─────────────

fn compute_directional_cascades(light_idx: u32, direction: vec3f) {
    let base = lights[light_idx].shadow_index;
    let dir = normalize(direction);
    let up = select(vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0), abs(dot(dir, vec3f(0.0, 1.0, 0.0))) > 0.99);

    // Unproject 8 NDC corners to world space: near plane first (reversed
    // depth 1), then the far end (depth 0, infinitely far).
    let ndc = array<vec4f, 8>(
        vec4f(-1.0, -1.0, 1.0, 1.0), vec4f(1.0, -1.0, 1.0, 1.0),
        vec4f(-1.0,  1.0, 1.0, 1.0), vec4f(1.0,  1.0, 1.0, 1.0),
        vec4f(-1.0, -1.0, 0.0, 1.0), vec4f(1.0, -1.0, 0.0, 1.0),
        vec4f(-1.0,  1.0, 0.0, 1.0), vec4f(1.0,  1.0, 0.0, 1.0),
    );

    // Near corners are finite. Far corners are only needed out to the last
    // split: walk each corner ray (the homogeneous far point minus the eye,
    // exact for any w) to that distance. The camera projection has no far
    // plane, so its far corners are points at infinity (w == 0) and dividing
    // by w would give inf/NaN. A finite far plane inside the last split, from
    // a caller's own projection, is still honoured.
    let eye = cameras[0].position_near.xyz;
    var world: array<vec3f, 8>;
    for (var i = 0u; i < 8u; i++) {
        let v = cameras[0].inv_view_proj * ndc[i];
        if i < 4u {
            world[i] = v.xyz / v.w;
        } else {
            let far_point = v.xyz / v.w;
            let capped = eye + normalize(v.xyz - v.w * eye) * CSM_SPLITS.w;
            let finite_far = v.w > 0.0 && length(far_point - eye) < CSM_SPLITS.w;
            world[i] = select(capped, far_point, finite_far);
        }
    }

    // Compute camera distances for near/far planes
    var near_dist = 0.0;
    var far_dist = 0.0;
    for (var i = 0u; i < 4u; i++) {
        near_dist += length(world[i] - cameras[0].position_near.xyz);
        far_dist  += length(world[i + 4u] - cameras[0].position_near.xyz);
    }
    near_dist /= 4.0;
    far_dist  /= 4.0;
    let depth = max(far_dist - near_dist, 1.0);

    let prev_d = array<f32, 4>(0.0, CSM_SPLITS.x, CSM_SPLITS.y, CSM_SPLITS.z);

    // Compute 4 cascade matrices
    for (var cascade_idx = 0u; cascade_idx < 4u; cascade_idx++) {
        let t0 = clamp((prev_d[cascade_idx] - near_dist) / depth, 0.0, 1.0);
        let t1 = clamp((CSM_SPLITS[cascade_idx] - near_dist) / depth, 0.0, 1.0);

        // 8 world-space corners of this frustum slice
        var cc: array<vec3f, 8>;
        for (var j = 0u; j < 4u; j++) {
            cc[j * 2u]       = mix(world[j], world[j + 4u], t0);
            cc[j * 2u + 1u]  = mix(world[j], world[j + 4u], t1);
        }

        // Sphere fit: centroid + radius
        var centroid = vec3f(0.0);
        for (var i = 0u; i < 8u; i++) {
            centroid += cc[i];
        }
        centroid /= 8.0;

        var radius = 0.0;
        for (var i = 0u; i < 8u; i++) {
            radius = max(radius, length(cc[i] - centroid));
        }

        // Snap radius to texel boundaries
        let texel_size = (2.0 * radius) / f32(max(shadow_mats[base + cascade_idx].policy.z, 128u));
        let radius_snap = ceil(radius / texel_size) * texel_size;

        // Texel-snapped light view
        let light_view_raw = mat4_look_at_rh(centroid - dir * SCENE_DEPTH, centroid, up);
        let centroid_ls_v4 = light_view_raw * vec4f(centroid, 1.0);
        let centroid_ls = centroid_ls_v4.xyz / centroid_ls_v4.w;  // Match CPU transform_point3
        let snap = texel_size;
        let snapped_x = round(centroid_ls.x / snap) * snap;
        let snapped_y = round(centroid_ls.y / snap) * snap;

        // Apply snap offset in world space
        let right_ws = normalize(vec3f(light_view_raw[0][0], light_view_raw[1][0], light_view_raw[2][0]));
        let up_ws    = normalize(vec3f(light_view_raw[0][1], light_view_raw[1][1], light_view_raw[2][1]));
        let snap_offset = right_ws * (snapped_x - centroid_ls.x) + up_ws * (snapped_y - centroid_ls.y);
        let stable_centroid = centroid + snap_offset;

        let light_view = mat4_look_at_rh(stable_centroid - dir * SCENE_DEPTH, stable_centroid, up);
        let proj = mat4_orthographic_rh(-radius_snap, radius_snap, -radius_snap, radius_snap, 0.1, SCENE_DEPTH * 2.0);

        shadow_mats[base + cascade_idx].mat = proj * light_view;
    }

    // Fill slots 4-5 with identity (point light faces 4-5 unused for directional)
    for (var i = 4u; i < 6u; i++) {
        shadow_mats[base + i].mat = mat4x4f(
            vec4f(1.0, 0.0, 0.0, 0.0),
            vec4f(0.0, 1.0, 0.0, 0.0),
            vec4f(0.0, 0.0, 1.0, 0.0),
            vec4f(0.0, 0.0, 0.0, 1.0),
        );
    }
}

// ── FNV-1a hash for matrix change detection ───────────────────────────────────

fn fnv_hash_mat(m: mat4x4f) -> u32 {
    var hash: u32 = 2166136261u;
    for (var col = 0u; col < 4u; col++) {
        for (var row = 0u; row < 4u; row++) {
            let bits = bitcast<u32>(m[col][row]);
            hash ^= (bits & 0xFFu);
            hash = hash * 16777619u;
            hash ^= ((bits >> 8u) & 0xFFu);
            hash = hash * 16777619u;
            hash ^= ((bits >> 16u) & 0xFFu);
            hash = hash * 16777619u;
            hash ^= ((bits >> 24u) & 0xFFu);
            hash = hash * 16777619u;
        }
    }
    return hash;
}

fn fnv_hash_mats_6(base_idx: u32) -> u32 {
    var hash: u32 = 2166136261u;
    for (var i = 0u; i < 6u; i++) {
        let mat_hash = fnv_hash_mat(shadow_mats[base_idx + i].mat);
        hash ^= (mat_hash & 0xFFu);
        hash = hash * 16777619u;
        hash ^= ((mat_hash >> 8u) & 0xFFu);
        hash = hash * 16777619u;
        hash ^= ((mat_hash >> 16u) & 0xFFu);
        hash = hash * 16777619u;
        hash ^= ((mat_hash >> 24u) & 0xFFu);
        hash = hash * 16777619u;
    }
    return hash;
}

// ── Main compute entry point ──────────────────────────────────────────────────

@compute @workgroup_size(64)
fn compute_shadow_matrices(@builtin(global_invocation_id) gid: vec3u) {
    let light_idx = gid.x;
    if light_idx >= params.light_count { return; }

    let light = lights[light_idx];

    // SceneDB is entity-indexed. An unused row is all zeros, including
    // shadow_index=0 and light_type=directional: without a liveness check
    // every empty row computes invalid cascades and races on shadow slot 0.
    if light.color_intensity.w <= 0.0 || light.shadow_index == 0xFFFFFFFFu { return; }

    // Compute matrices based on light type
    if light.light_type == LIGHT_TYPE_POINT {
        compute_point_light_matrices(light_idx, light.position_range.xyz, light.position_range.w);
    } else if light.light_type == LIGHT_TYPE_DIRECTIONAL {
        compute_directional_cascades(light_idx, light.direction_outer.xyz);
    } else if light.light_type == LIGHT_TYPE_SPOT {
        compute_spot_matrix(light_idx, light.position_range.xyz, light.direction_outer.xyz, light.position_range.w, light.direction_outer.w);
    }

    // Hash the computed matrices to detect changes
    // This enables shadow atlas caching for static geometry
    let base_idx = light.shadow_index;
    let caster_slot = base_idx / FACES_PER_LIGHT;
    let new_hash = fnv_hash_mats_6(base_idx);
    let old_hash = shadow_hashes[caster_slot];

    // Update hash and mark dirty if changed
    if new_hash != old_hash {
        shadow_hashes[caster_slot] = new_hash;
        // Atomically set dirty flag for this light
        atomicStore(&shadow_dirty[caster_slot], 1u);
    }
}
