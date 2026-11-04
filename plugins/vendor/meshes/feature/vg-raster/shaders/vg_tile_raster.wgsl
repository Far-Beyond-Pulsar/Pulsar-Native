// Tiled shared-memory raster kernel: one workgroup per 16x16 tile, one thread
// per pixel. REUSABLE: this is both the no-int64 fallback of Tier 2 and the
// kernel Tier 3 (`helio-pass-vg-dense`, T8) builds on.
//
// Contract
//   input   per-tile lists of vis ids: `tile_bin_counts[tile]` entries in
//           `tile_bin_entries[tile * cap ..]`. Anything that fills the lists
//           can drive it (Tier 2 fallback: `vg_sw_bin.wgsl`; Tier 3: the dense
//           tile's cluster list expanded to triangles).
//   work    the list is consumed in batches of 64 triangles. Threads 0..63
//           cooperatively decode + project + set up one triangle each into
//           shared memory (`sh_setup`, ~3.5 KiB); then every pixel thread
//           tests all 64 setups against its own pixel center and keeps the
//           nearest (depth, id) in registers. No atomics at all, hence no
//           contention; depth is also tested against the Tier 1 depth.
//   output  `sw_keys[y * width + x] = (id, depth_bits)` as two u32, which is
//           byte-identical to the 64-bit key of the atomic kernel
//           (`depth << 32 | id`, little endian). Untouched pixels stay 0.
//           `tile_counters[2t] = list length`, `[2t+1] = pixel hits that
//           passed the depth test` (same meaning as the atomic kernel).
//
// Tie order matches `atomicMax` on the 64-bit key: the larger depth wins, an
// equal depth is won by the larger id.
//
// Dispatch (tiles_x, tiles_y, 1). Requires vg_format.wgsl, vg_bindings.wgsl,
// vg_decode.wgsl, vg_raster_common.wgsl.

@group(1) @binding(0) var<storage, read> tile_bin_counts: array<u32>;
@group(1) @binding(1) var<storage, read> tile_bin_entries: array<u32>;
@group(1) @binding(2) var<storage, read_write> sw_keys: array<vec2<u32>>;
@group(1) @binding(3) var hw_depth: texture_depth_2d;
@group(1) @binding(4) var<storage, read_write> tile_counters: array<u32>;

const VG_TILE_BATCH: u32 = 64u;

var<workgroup> sh_setup: array<VgTriSetup, 64>;
var<workgroup> sh_count: u32;
var<workgroup> sh_hits: atomic<u32>;

@compute @workgroup_size(16, 16)
fn tile_raster_main(
    @builtin(workgroup_id) wid: vec3<u32>,
    @builtin(local_invocation_id) lid2: vec3<u32>,
    @builtin(local_invocation_index) lid: u32,
) {
    let tile = wid.y * vg_params.tiles_x + wid.x;
    if (lid == 0u) {
        sh_count = tile_bin_counts[tile];
        atomicStore(&sh_hits, 0u);
    }
    workgroupBarrier();
    let n_total = workgroupUniformLoad(&sh_count);
    let n = min(n_total, vg_params.tile_bin_cap);

    let px = i32(wid.x * 16u + lid2.x);
    let py = i32(wid.y * 16u + lid2.y);
    let inside = u32(px) < vg_params.width && u32(py) < vg_params.height;
    let size_f = vec2<f32>(f32(vg_params.width), f32(vg_params.height));
    let size_i = vec2<i32>(i32(vg_params.width), i32(vg_params.height));

    var best_db = 0u;
    if (inside) {
        best_db = bitcast<u32>(textureLoad(hw_depth, vec2<i32>(px, py), 0));
    }
    let hw_db = best_db;
    // Phase 1 of a two-phase frame resumes from the keys phase 0 left (an
    // untouched pixel is 0, which never beats the Tier 1 depth). Cleared at
    // the start of phase 0, so a single-phase frame sees zeros.
    var best_id = 0u;
    var found = false;
    if (inside) {
        let prior = sw_keys[u32(py) * vg_params.width + u32(px)];
        if (prior.y != 0u && prior.y >= hw_db) {
            best_db = prior.y;
            best_id = prior.x;
            found = true;
        }
    }
    var local_hits = 0u;

    let batches = (n + VG_TILE_BATCH - 1u) / VG_TILE_BATCH;
    for (var b = 0u; b < batches; b++) {
        if (lid < VG_TILE_BATCH) {
            var s = VgTriSetup();
            let e = b * VG_TILE_BATCH + lid;
            if (e < n) {
                let id = tile_bin_entries[tile * vg_params.tile_bin_cap + e];
                let vis = vg_visible[vg_vis_cluster(id)];
                let cs = vis.cluster_slot;
                let cl = vgd_load_cluster(cs);
                let m = vg_object_transform(vis);
                let h = vgd_pos_block(cs, cl, vg_asset_of(vis));
                let ix = vgd_tri(cs, cl, vg_vis_tri(id));
                s = vg_tri_setup(
                    vg_screen_vertex(m, h, ix.x, size_f),
                    vg_screen_vertex(m, h, ix.y, size_f),
                    vg_screen_vertex(m, h, ix.z, size_f),
                    size_i,
                    (vg_params.flags & VG_FLAG_CULL_BACK) != 0u,
                    vg_is_mirrored(m),
                );
            }
            sh_setup[lid] = s;
            // The id is needed per setup in the pixel loop.
        }
        workgroupBarrier();
        if (inside) {
            for (var i = 0u; i < VG_TILE_BATCH; i++) {
                let s = sh_setup[i];
                if (s.kind == VG_TRI_EMPTY || s.kind == VG_TRI_CLIPPED) { continue; }
                let d = vg_tri_depth_at(s, px, py);
                if (d < 0.0) { continue; }
                let db = bitcast<u32>(d);
                if (db < hw_db) { continue; }     // behind the Tier 1 depth
                local_hits++;                     // counted like the atomic kernel: every fragment passing Tier 1
                if (db < best_db) { continue; }   // behind the current best
                let id = tile_bin_entries[tile * vg_params.tile_bin_cap + b * VG_TILE_BATCH + i];
                if (db > best_db || !found || id > best_id) {
                    best_db = db;
                    best_id = id;
                    found = true;
                }
            }
        }
        workgroupBarrier();
    }

    if (inside && found) {
        sw_keys[u32(py) * vg_params.width + u32(px)] = vec2<u32>(best_id, best_db);
    }
    atomicAdd(&sh_hits, local_hits);
    workgroupBarrier();
    if (lid == 0u) {
        tile_counters[tile * 2u] += n_total;
        tile_counters[tile * 2u + 1u] += atomicLoad(&sh_hits);
    }
}
