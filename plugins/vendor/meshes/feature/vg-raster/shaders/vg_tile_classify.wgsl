// Tile overlap classification (plan 5.3).
//
// Compares the per-tile counters written by the software tier to thresholds
// and writes the dense-tile mask for this frame, plus a compact list of dense
// tiles and indirect dispatch args for Tier 3.
//
// A tile is dense when `tris >= tri_thresh` or `hits >= hit_thresh`
// (hits = pixel hits that passed the depth test, i.e. total depth complexity;
// 256 pixels per tile, so hit_thresh = 256 * K means average overdraw K).
// Hysteresis: a tile that was dense in the previous frame stays dense until
// both counters fall below `hysteresis * threshold` (hysteresis in (0, 1]).
//
// Dispatch `classify_main` over ceil(tile_count / 64) workgroups, then
// `dense_args_main` with one workgroup.
//
// Mask format: one u32 per tile, 1 = dense (row-major tile index).
// `dense_args` (u32 x 4): [0..3] = DispatchIndirect (x, y, z) for one workgroup
// per dense tile (2D: x <= 65535), [3] = dense tile count. Clear [3] each frame.

struct ClassifyParams {
    tile_count:  u32,
    tri_thresh:  u32,
    hit_thresh:  u32,
    hysteresis:  f32,
    // Upper bound on dense tiles per frame (the excess stays on Tier 2).
    max_dense:   u32,
    // Frames a tile stays dense after its counters last qualified (>= 1).
    latch:       u32,
    pad0:        u32,
    pad1:        u32,
}

@group(0) @binding(0) var<uniform> cp: ClassifyParams;
@group(0) @binding(1) var<storage, read> tile_counters: array<u32>;
@group(0) @binding(2) var<storage, read> prev_mask: array<u32>;
@group(0) @binding(3) var<storage, read_write> cur_mask: array<u32>;
@group(0) @binding(4) var<storage, read_write> dense_list: array<u32>;
@group(0) @binding(5) var<storage, read_write> dense_args: array<atomic<u32>, 4>;
// Estimated triangles per tile of the clusters the cull diverted to Tier 3
// (`vg_dense_count.wgsl`); they are not rasterised, so `tile_counters` misses them.
@group(0) @binding(6) var<storage, read> dense_est: array<u32>;
// Persistent per-tile latch age (frames of dense status left). Never cleared.
@group(0) @binding(7) var<storage, read_write> dense_age: array<u32>;

@compute @workgroup_size(64)
fn classify_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let t = gid.x;
    if (t >= cp.tile_count) { return; }
    let was_dense = prev_mask[t] != 0u;
    // Feedback-safe: the cull routes the clusters of a dense tile away from the
    // software raster, which empties its counters. A tile that WAS dense also
    // counts the estimate of its diverted clusters (as triangles and, halved, as
    // hits), so it keeps its state while the geometry stays. The estimate never
    // makes a tile dense on its own, so it cannot grow the mask.
    let est = select(0.0, f32(dense_est[t]), was_dense);
    let tris = f32(tile_counters[t * 2u]) + est;
    let hits = f32(tile_counters[t * 2u + 1u]) + 0.5 * est;
    let k = select(1.0, cp.hysteresis, was_dense);
    let qualifies = tris >= f32(cp.tri_thresh) * k || hits >= f32(cp.hit_thresh) * k;
    // Latch with decay: a tile that qualified stays dense for `latch` frames,
    // so an under-estimate of the diverted clusters cannot make it flicker.
    var age = dense_age[t];
    if qualifies {
        age = max(cp.latch, 1u);
    } else if age > 0u {
        age = age - 1u;
    }
    var dense = age > 0u;
    if dense {
        let i = atomicAdd(&dense_args[3], 1u);
        if i >= cp.max_dense {
            atomicSub(&dense_args[3], 1u);
            dense = false;
            age = 0u;
        } else {
            dense_list[i] = t;
        }
    }
    dense_age[t] = age;
    cur_mask[t] = select(0u, 1u, dense);
}

@compute @workgroup_size(1)
fn dense_args_main() {
    let c = atomicLoad(&dense_args[3]);
    atomicStore(&dense_args[0], min(c, 65535u));
    atomicStore(&dense_args[1], max((c + 65534u) / 65535u, 1u));
    atomicStore(&dense_args[2], 1u);
}
