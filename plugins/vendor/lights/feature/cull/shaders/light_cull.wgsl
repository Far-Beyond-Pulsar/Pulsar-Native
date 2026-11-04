//! Tiled light culling (Forward+ style).
//!
//! Divides the screen into TILE_SIZE×TILE_SIZE tiles.  Each workgroup thread
//! is responsible for one tile: it iterates all lights, tests whether each
//! light's sphere of influence intersects the tile frustum (constructed from
//! the tile's four corner NDC coordinates), and writes matching light indices
//! into a flat storage array.
//!
//! CPU cost: O(1) — one dispatch with ceil(num_tiles / 256) workgroups.
//! GPU cost: O(sparse_capacity) when light data changes, plus
//! O(num_tiles × active_lights). Empty entity slots are never scanned per tile.
//!
//! Tile data layout (per tile slot of MAX_LIGHTS_PER_TILE entries):
//!   tile_light_lists[tile_idx * MAX_LIGHTS_PER_TILE + i]  = light_index_i
//!   tile_light_counts[tile_idx]                           = number of lights

const TILE_SIZE: u32 = 16u;
const MAX_LIGHTS_PER_TILE: u32 = 64u;

// ─────────────────────────────────────────────────────────────────────────────
// Bind group 0 — uniforms & scene data
// ─────────────────────────────────────────────────────────────────────────────

struct Camera {
    view:          mat4x4<f32>,
    proj:          mat4x4<f32>,
    view_proj:     mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    position_near: vec4<f32>,
    direction_far: vec4<f32>,
}
@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;

struct LightCullParams {
    num_tiles_x:   u32,
    num_tiles_y:   u32,
    num_lights:    u32,
    screen_width:  u32,
    screen_height: u32,
    // 1 when `lights[i]`/`transforms[i]` share the same raw entity index
    // (SceneDB-direct); 0 when `lights` is `ctx.scene.lights`, a freshly
    // rebuilt dense array needing `light_entity_indices[i]`. See that
    // binding's doc.
    light_mode_direct_index: u32,
    _pad1:         u32,
    _pad2:         u32,
}
@group(0) @binding(1) var<uniform> params: LightCullParams;

struct GpuLight {
    position_range:  vec4<f32>,  // xyz = world pos, w = range
    direction_outer: vec4<f32>,
    color_intensity: vec4<f32>,
    shadow_index:    u32,
    light_type:      u32,        // 0 = directional, 1 = point, 2 = spot
    inner_angle:     f32,
    _pad:            u32,
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
@group(0) @binding(2) var<storage, read> lights: array<GpuLight>;

// SceneDB's `Transform` component, entity-indexed the same way `lights` is
// (both are `#[gpu(layout = packed)]` SceneDB buffers: row `i` is whichever
// entity has raw index `i`) -- see `helio_pass_forward_lit`'s own `Transform`
// binding doc for the exact byte-layout reasoning and why `light.position_
// range.xyz` itself is a stale/zeroed placeholder, not real position data.
struct Transform {
    position: array<f32, 3>,
    rotation: array<f32, 3>,
    scale:    array<f32, 3>,
}
@group(0) @binding(3) var<storage, read> transforms: array<Transform>;
// Count followed by active sparse light indices. Output indices remain in the
// original SceneDB address space consumed by deferred/forward shading.
@group(0) @binding(4) var<storage, read> light_entity_indices: array<u32>;

struct ActiveLights {
    count: atomic<u32>,
    indices: array<u32>,
}
@group(0) @binding(7) var<storage, read_write> active_lights: ActiveLights;

@compute @workgroup_size(256)
fn compact_lights(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i >= min(params.num_lights, arrayLength(&lights)) { return; }
    if lights[i].color_intensity.w <= 0.0 { return; }
    let slot = atomicAdd(&active_lights.count, 1u);
    active_lights.indices[slot] = i;
}

// Output: flat arrays, one slot per tile
@group(0) @binding(5) var<storage, read_write> tile_light_lists:  array<u32>;
@group(0) @binding(6) var<storage, read_write> tile_light_counts: array<u32>;

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

/// Build a view-space frustum plane from an NDC edge.
/// Each plane is stored as (nx, ny, nz, d) where Ax+By+Cz+D >= 0 is inside.
fn make_plane_from_ndc_edge(p0: vec2<f32>, p1: vec2<f32>) -> vec4<f32> {
    // Unproject the two edge points to view-space rays (z = -1 in view space).
    let r0 = normalize(vec3<f32>(
        p0.x / cameras[0].proj[0][0],
        p0.y / cameras[0].proj[1][1],
        -1.0,
    ));
    let r1 = normalize(vec3<f32>(
        p1.x / cameras[0].proj[0][0],
        p1.y / cameras[0].proj[1][1],
        -1.0,
    ));
    // Plane normal = cross of the two rays (points toward inside of frustum).
    let n = normalize(cross(r0, r1));
    return vec4<f32>(n, 0.0); // d = 0 (plane passes through camera origin)
}

/// Test whether a sphere (view-space center + radius) intersects the four
/// lateral frustum planes of a tile.
/// Returns true if the sphere is NOT culled (i.e. it may be visible).
fn sphere_inside_tile_frustum(
    sphere_center_vs: vec3<f32>,
    sphere_radius: f32,
    planes: array<vec4<f32>, 4>,
) -> bool {
    for (var i = 0u; i < 4u; i++) {
        let dist = dot(planes[i].xyz, sphere_center_vs) + planes[i].w;
        if dist < -sphere_radius {
            return false; // Entirely outside this plane
        }
    }
    return true;
}

// ─────────────────────────────────────────────────────────────────────────────
// Main kernel — one thread per tile
// ─────────────────────────────────────────────────────────────────────────────

@compute @workgroup_size(256, 1, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let tile_idx = gid.x;
    let total_tiles = params.num_tiles_x * params.num_tiles_y;
    if tile_idx >= total_tiles {
        return;
    }

    let tile_x = tile_idx % params.num_tiles_x;
    let tile_y = tile_idx / params.num_tiles_x;

    // Compute the tile's NDC bounding rectangle.
    // NDC x ∈ [-1, +1] left→right, y ∈ [-1, +1] bottom→top.
    let fw = f32(params.screen_width);
    let fh = f32(params.screen_height);
    let ts = f32(TILE_SIZE);
    let ndc_left  = (f32(tile_x) * ts / fw) * 2.0 - 1.0;
    let ndc_right = min((f32(tile_x + 1u) * ts / fw) * 2.0 - 1.0, 1.0);
    // Note: screen y goes top-down, NDC y goes bottom-up.
    let ndc_top    =  1.0 - (f32(tile_y) * ts / fh) * 2.0;
    let ndc_bottom =  max(1.0 - (f32(tile_y + 1u) * ts / fh) * 2.0, -1.0);

    // Build 4 lateral frustum planes from the tile corners.
    // Plane order: left, right, top, bottom.
    var planes: array<vec4<f32>, 4>;
    planes[0] = make_plane_from_ndc_edge(vec2<f32>(ndc_left, ndc_bottom),  vec2<f32>(ndc_left,  ndc_top));    // left
    planes[1] = make_plane_from_ndc_edge(vec2<f32>(ndc_right, ndc_top),    vec2<f32>(ndc_right, ndc_bottom)); // right
    planes[2] = make_plane_from_ndc_edge(vec2<f32>(ndc_left,  ndc_top),    vec2<f32>(ndc_right, ndc_top));    // top
    planes[3] = make_plane_from_ndc_edge(vec2<f32>(ndc_right, ndc_bottom), vec2<f32>(ndc_left,  ndc_bottom)); // bottom

    // Iterate all lights and test each against this tile's frustum.
    var count = 0u;
    for (var active_slot = 0u; active_slot < light_entity_indices[0]; active_slot++) {
        let i = light_entity_indices[active_slot + 1u];
        let light = lights[i];

        // Skip never-written slots. The direct-index path always iterates
        // the SceneDB buffer's full fixed capacity (`params.num_lights` ==
        // `MAX_LIGHTS`), not just the slots an entity has actually claimed
        // -- `LightComponent` derives `bytemuck::Zeroable`, so an unclaimed
        // row reads back as all-zero, which means `light_type == 0u` (the
        // SAME value as a real `LightType::Directional`). Without this
        // check every unclaimed slot gets treated as an always-visible
        // directional light with a zero-length `direction_outer` -- and
        // normalizing a zero vector in the shading pass produces NaN, which
        // then poisons the entire lighting sum for every pixel every tile
        // touches, silently blacking out lighting everywhere regardless of
        // whether the real lights are correctly culled.
        if light.color_intensity.w <= 0.0 {
            continue;
        }

        // Directional lights have infinite range — always visible.
        if light.light_type == 0u {
            if count < MAX_LIGHTS_PER_TILE {
                tile_light_lists[tile_idx * MAX_LIGHTS_PER_TILE + count] = i;
                count += 1u;
            }
            continue;
        }

        // World-space light position. The direct-index (`scene_lights`) path
        // authors `helio_pass_forward_lit::LightComponent` rows with position
        // already baked into `position_range.xyz` at spawn/update time (see
        // that component's module doc: "no per-frame CPU touch at all, not
        // even a live-count read") -- these light entities never get a
        // separate `Transform` component, so indexing `transforms[]` at this
        // light's row for them silently reads an unrelated/never-written
        // (all-zero) row instead of the light's real position, and every
        // point/spot light then culls out of every tile it should actually
        // be visible in. Only the CPU-resolved path (`light_mode_direct_index
        // == 0u`, `helio_component::LightComponent` via `light_entity_
        // indices`) needs the separate Transform combine -- that mirror has
        // no position of its own (see `LightComponentGpuMirror::
        // to_helio_gpu_light`'s doc).
        var world_pos = light.position_range.xyz;
        if params.light_mode_direct_index == 0u {
            let entity_idx = light_entity_indices[i];
            let t = transforms[entity_idx];
            world_pos = vec3<f32>(t.position[0], t.position[1], t.position[2]);
        }
        let pos_vs = (cameras[0].view * vec4<f32>(world_pos, 1.0)).xyz;
        let range  = light.position_range.w;

        if sphere_inside_tile_frustum(pos_vs, range, planes) {
            if count < MAX_LIGHTS_PER_TILE {
                tile_light_lists[tile_idx * MAX_LIGHTS_PER_TILE + count] = i;
                count += 1u;
            }
        }
    }

    tile_light_counts[tile_idx] = count;
}
