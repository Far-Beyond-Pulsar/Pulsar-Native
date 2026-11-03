// hitbox.frag.wgsl — AABB-based water displacement (replaces sphere.frag).
//
// For each hitbox we compute how much water the AABB *was* displacing (old bounds)
// and how much it *now* displaces (new bounds).  The difference drives a height
// change:  rise where the box vacated, fall where it now sits.
//
// Hitboxes are world-space boxes. Each draw simulates one volume (the
// instance index) and maps them into that volume's sim space: cascade 0
// tiles world XZ every `PATCH_SIZE` metres, so a texel stands for the world
// point of its tile nearest the box, at the volume's rest height, and a box
// outside the volume's footprint leaves it untouched.
//
// "Was" is where the box was the last time it was applied: the pass keeps the
// previous frame's rows (`previous`), and while a row holds the same body
// (`params.z`, its identity) the old bounds are the previous row's new ones,
// so a body that stops moving stops displacing water. A row whose body
// changed starts from its own `old_*` bounds.
//
// Texture layout (Rgba16Float):
//   R = height  (read-write)
//   G = velocity (read-only, pass through)
//   B = normal.x (read-only, pass through)
//   A = normal.z (read-only, pass through)

@group(0) @binding(0) var water_texture: texture_2d<f32>;
@group(0) @binding(1) var water_sampler: sampler;

/// One AABB hitbox (80 bytes = 5 × vec4<f32>)
struct GpuWaterHitbox {
    old_min:  vec4<f32>,   // xyz = old AABB min
    old_max:  vec4<f32>,   // xyz = old AABB max
    new_min:  vec4<f32>,   // xyz = new AABB min
    new_max:  vec4<f32>,   // xyz = new AABB max
    params:   vec4<f32>,   // x = edge_softness, y = strength, z = identity
}

struct HitboxUniforms {
    /// Number of active hitboxes
    count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    /// How far a camera-relative frame's world origin moved since
    /// `previous` was captured (xyz): its bounds are relative to the old one.
    previous_shift: vec4<f32>,
}
@group(0) @binding(2) var<uniform> u: HitboxUniforms;

@group(0) @binding(3) var<storage, read> hitboxes: array<GpuWaterHitbox>;
@group(0) @binding(4) var<storage, read> previous: array<GpuWaterHitbox>;

/// `GpuWaterVolume` (16 vec4s); only the bounds and world phase are read
/// here.
struct WaterVolume {
    bounds_min: vec4<f32>,
    bounds_max: vec4<f32>,  // w = rest height of the surface
    _rest: array<vec4<f32>, 13>,
    world_phase: vec4<f32>, // xy = the frame's world origin XZ mod 270 m
}
@group(0) @binding(5) var<storage, read> volumes: array<WaterVolume>;

/// Cascade 0's tile, in metres (`CASCADE_PATCH_SIZES[0]`).
const PATCH_SIZE: f32 = 30.0;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Smooth 3-D Gaussian falloff inside an AABB, times the depth of the box
/// below the surface.
///
/// The weight is 1.0 inside the box's footprint, smoothly tapering to 0
/// beyond its edges; `softness` (metres) scales how far the falloff extends.
/// The texel stands for the point of its tile nearest the box, at the rest
/// height; outside the volume's footprint it is 0.
fn volume_in_box(box_min: vec3<f32>, box_max: vec3<f32>, uv: vec2<f32>, softness: f32, vol: WaterVolume) -> f32 {
    let box_center  = (box_min + box_max) * 0.5;
    let box_half    = (box_max - box_min) * 0.5;
    let rest_y      = vol.bounds_max.w;

    // The world XZ this texel stands for, in the tile nearest the box. The
    // waves tile the world, so in a camera-relative frame the texel's
    // tiling is offset by the volume's world phase (as in `surface.wgsl`).
    let tile = uv * PATCH_SIZE - vol.world_phase.xy;
    let world_xz = tile + PATCH_SIZE * round((box_center.xz - tile) / PATCH_SIZE);
    if any(world_xz < vol.bounds_min.xz) || any(world_xz > vol.bounds_max.xz) {
        return 0.0;
    }

    // Per-axis distance from the box's footprint (negative inside, positive outside)
    let d = abs(world_xz - box_center.xz) - box_half.xz;

    // Smooth falloff: exp(-clamp(d/softness, 0, 4)^2) per axis, multiplied together
    let soft = max(d, vec2<f32>(0.0)) / max(softness, 0.001);
    let weight = exp(-dot(soft * soft, vec2<f32>(1.0)));

    // Only the part of the box below the surface displaces water.
    let submerged_depth = clamp(rest_y - box_min.y, 0.0, max(box_max.y - box_min.y, 0.0));

    return weight * submerged_depth * 0.1;
}

// ── Entry point ──────────────────────────────────────────────────────────────

@fragment
fn fs_main(
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) volume: u32,
) -> @location(0) vec4<f32> {
    var info = textureSample(water_texture, water_sampler, uv);
    if volume >= arrayLength(&volumes) {
        return info;
    }
    let vol = volumes[volume];
    if any(vol.bounds_max.xz <= vol.bounds_min.xz) {
        return info;
    }

    let count = min(u.count, min(arrayLength(&hitboxes), arrayLength(&previous)));
    for (var i: u32 = 0u; i < count; i = i + 1u) {
        let hb = hitboxes[i];
        let softness = hb.params.x;
        let strength = hb.params.y;
        if strength == 0.0 {
            continue;
        }
        var old_min = hb.old_min.xyz;
        var old_max = hb.old_max.xyz;
        let last = previous[i];
        if last.params.z == hb.params.z && last.params.y != 0.0 {
            old_min = last.new_min.xyz - u.previous_shift.xyz;
            old_max = last.new_max.xyz - u.previous_shift.xyz;
        }

        // Water rises where the box *was* (old position)
        info.r += volume_in_box(old_min, old_max, uv, softness, vol) * strength;
        // Water falls where the box *is* (new position)
        info.r -= volume_in_box(hb.new_min.xyz, hb.new_max.xyz, uv, softness, vol) * strength;
    }

    return info;
}
