//!use material_row
//! GPU-driven object sort/group/batch pipeline.
//!
//! Turns every live `StaticObjectComponent` SceneDB row into the exact same
//! shape `Scene::rebuild_instance_buffers`'s CPU implementation used to
//! produce: a sorted-by-`(material_class, graph_hash, mesh_id, material_id)`
//! `instances`/`aabbs` array, one `draw_calls` entry per contiguous
//! same-`(mesh_id, material_id)` run, and `(class, graph_hash, start, count)`
//! ranges over those draw-call groups (split into opaque/transparent/
//! forward), plus the static/movable shadow-partitioned indirect lists —
//! with no per-frame CPU iteration over scene objects at all.
//!
//! # Pipeline stages (one `RenderPass::execute()` per frame)
//!
//! 1. `cs_gather` — one thread per `StaticObjectComponent` row capacity slot.
//!    Skips rows nobody has ever written (`mesh_generation == 0u`, the
//!    `Zeroable` default -- never a real generation, see `mesh_generation`'s
//!    field doc in `helio_mesh_api::components`). Live rows compute this
//!    migration's packed sort key and atomically compact directly into
//!    `keys_a`/`indices_a` -- the same buffers the sort's bit-0 pass reads
//!    (same technique as the sprites plugin's sprite cull's `cs_cull`: a single
//!    global atomic counter, order doesn't matter yet).
//! 2. `cs_prepare` — turns the gather's live count into `frame_uniform`
//!    (`num_blocks`) and `dispatch_args` (indirect dispatch size for every
//!    later stage) — same technique as the sprites plugin's sprite cull's
//!    `sprite_sort.wgsl::cs_prepare`.
//! 3. `helio_core::radix_sort` — a stable LSD radix sort of `(key, index)`
//!    pairs, 4-bit digits, eight passes; ends in `keys_a`/`indices_a`.
//! 4. `cs_final_gather` — re-reads `static_objects[indices_a[i]]` in final
//!    sorted order and writes `instances[i]`/`aabbs[i]`. (`keys_a`/
//!    `indices_a` only ever carry the sort key + row index, not the full
//!    ~236-byte row, to keep the sort's per-element payload small.)
//! 5. `cs_group_local_scan` / `cs_group_block_scan` / `cs_group_write` —
//!    detect draw-call group boundaries (`sorted_keys_a[i] !=
//!    sorted_keys_a[i-1]`) and turn them into *stable* (position-ordered,
//!    not execution-ordered) group indices via the same two-level
//!    Hillis-Steele-per-block + single-thread-cross-block-prefix-sum scan
//!    technique `cs_scatter`'s rank calculation already uses at the
//!    single-block scale -- generalized here to however many blocks the
//!    live object count spans. Writes `group_starts[]` (one entry per draw
//!    group, plus a trailing sentinel `group_starts[group_count] =
//!    live_count`) and per-group `(material_class, graph_hash, is_
//!    transparent, is_forward)` metadata.
//! 6. `cs_build_draw_calls` — one thread per group: assembles `draw_calls[g]`
//!    from `group_starts[g]`/`group_starts[g+1]` and the mesh's static draw
//!    parameters (read from the group's first sorted instance).
//! 7. `cs_range_local_scan` / `cs_range_block_scan` / `cs_range_write` — same
//!    two-level scan technique again, this time detecting boundaries between
//!    *groups* that no longer share `(material_class, graph_hash)`, to build
//!    the `(class, graph_hash, start, count)` range tuples PSO selection
//!    needs -- `start`/`count` here are draw-*group* indices, matching what
//!    `helio-pass-gbuffer`'s `multi_draw_indexed_indirect(indirect, start *
//!    20, count)` call expects (`start`/`count` index the `draw_calls`/
//!    `indirect` array directly, not `instances`). Split into three parallel
//!    output arrays (opaque/transparent/forward). A shading-category change
//!    also splits the run: materials sharing a class and graph can still
//!    require different passes.
//! 8. `cs_shadow_partition` — one thread per LIVE sorted instance: atomically
//!    appends a one-instance `DrawIndexedIndirectArgs` into either
//!    `shadow_static_indirect` or `shadow_movable_indirect` depending on
//!    `libhelio::INSTANCE_FLAG_MOVABLE` in the instance's `flags` (default
//!    unset = static/stationary, the common case for baked level geometry).
//!
//! # Capacity, not live count, sizes every buffer
//!
//! `StaticObjectComponent`'s SceneDB buffer auto-registers at
//! `DEFAULT_AUTO_REGISTER_CAPACITY` (64) and grows unbounded as more entities
//! get the component (`pulsar_scenedb::gpu::world_mirror`'s own doc: "small
//! initial capacity, unbounded growth"), unlike the small, genuinely-fixed-
//! ceiling domains elsewhere in this migration (lights/decals/post-process
//! volumes, capped at `DEFAULT_AUTO_REGISTER_CAPACITY` on purpose). Object
//! counts routinely exceed 64 in any real scene, so `capacity` here is
//! computed fresh each frame from the *live* SceneDB buffer's actual byte
//! size (`buffer.size() / size_of::<StaticObjectRow>()`). Every scratch
//! buffer this pipeline owns (`keys_a`/`keys_b`/`instances_out`/
//! `group_starts`/the range tables/the shadow-partition buffers, all of it)
//! is grown to match -- there is no fixed object-count ceiling anywhere in
//! this pipeline. See `src/lib.rs`'s `ObjectBatchPass::ensure_capacity` for
//! the growth policy (next-power-of-two, rebuilding every affected bind
//! group, exactly the same "grow on demand" seam `GrowableBuffer`/`corona`'s
//! `upload_sort_steps` already use elsewhere in this codebase). `max_objects`/
//! `max_groups`/`max_ranges` above are that grown scratch capacity, not a
//! hardcoded constant -- `max_groups`/`max_ranges` both reuse the SAME grown
//! capacity as `max_objects` because a draw-call group needs at least one
//! live object and a range needs at least one group, so `#ranges <= #groups
//! <= #objects` always holds without a second, independently-guessed ceiling.

const WG: u32 = 256u;

// ── Uniforms ─────────────────────────────────────────────────────────────────

/// Per-frame constants, written once by `prepare()`.
struct BatchUniforms {
    /// `static_objects` buffer's current row capacity (see module doc).
    capacity: u32,
    /// Scratch buffer capacity this frame's dispatches were sized against
    /// (>= capacity; only changes when scratch buffers grow).
    max_objects: u32,
    max_groups: u32,
    max_ranges: u32,
}
@group(0) @binding(0) var<uniform> batch: BatchUniforms;

/// SceneDB's `StaticObjectComponent` row -- byte-for-byte identical to
/// `helio_mesh_api::rows::StaticObjectComponent`'s `#[repr(C)]`
/// Rust layout (plain fixed-size arrays throughout, not `mat4x4<f32>`/
/// `vec4<f32>`, for exactly the reason every other SceneDB-mirrored struct
/// in this codebase avoids those: WGSL's struct-member alignment for those
/// types doesn't match Rust's packed layout). Keep in lock-step with that
/// struct; `helio_pass_object_batch`'s own test suite asserts the byte size
/// matches so a drift fails loudly at `cargo test` time, not at draw time.
struct StaticObjectRow {
    mesh_slot: u32,
    mesh_generation: u32,
    material_slot: u32,
    material_generation: u32,
    transform: array<array<f32, 4>, 4>,
    prev_transform: array<array<f32, 4>, 4>,
    normal_mat: array<array<f32, 4>, 3>,
    bounds: array<f32, 4>,
    index_count: u32,
    first_index: u32,
    vertex_offset: i32,
    material_class: u32,
    graph_hash_lo: u32,
    graph_hash_hi: u32,
    flags: u32,
    skin_vertex_offset: u32,
    bone_palette_offset: u32,
    bone_palette_count: u32,
    _skin_pad: u32,
}
@group(0) @binding(1) var<storage, read> static_objects: array<StaticObjectRow>;

// ── Stage 1: cs_gather ──────────────────────────────────────────────────────

// Writes DIRECTLY into `keys_a`/`indices_a` -- the same buffers the radix
// sort's bit-0 pass reads as its source (see stage 3's `src_keys_h`
// binding doc). There is no separate "raw" staging pair: gather's output
// and the sort's input are the same buffer by construction, so there is no
// copy step to forget between them.
@group(0) @binding(2) var<storage, read_write> gathered_keys: array<u32>;
@group(0) @binding(3) var<storage, read_write> gathered_indices: array<u32>;
/// `[_, live_count, _, _]` as a `DrawIndexedIndirectArgs`-shaped buffer so
/// `cs_gather`'s atomic bump doubles as this frame's authoritative live
/// count with no separate readback -- mirrors the sprites plugin's sprite cull's
/// `indirect_args[1]` reuse exactly.
@group(0) @binding(4) var<storage, read_write> gather_count: array<atomic<u32>>;

/// Each static_objects row's transform as of the last frame it was drawn,
/// with the identity it was drawn under. Supplies per-frame motion vectors:
/// a row's authored `prev_transform` is the transform before its last edit,
/// which stays different from `transform` after the object stops moving.
struct PrevRow {
    transform: array<array<f32, 4>, 4>,
    /// (mesh_slot, mesh_generation, material_slot, material_generation);
    /// all zero once the row is seen dead.
    identity: vec4<u32>,
}
@group(0) @binding(5) var<storage, read_write> prev_rows: array<PrevRow>;

/// A hierarchical per-row sort key matching the CPU reference's tuple sort
/// PRIORITY ORDER `(material_class, graph_hash, mesh_id, material_id)`, not
/// just its highest field: `material_class` occupies the top 8 bits
/// (exact, no hashing -- assumes <=256 material classes, matching every
/// other `& 0xFFu`-masked use of `material_class` in this codebase), a
/// 12-bit hash of `graph_hash` occupies the next 12 bits, and a 12-bit hash
/// of `(mesh_slot, material_slot)` occupies the low 12 bits.
///
/// The field SEPARATION (not just concatenation into one combined hash) is
/// the whole point: two rows sharing the same `(class, graph_hash)` but
/// different mesh/material get the same top 20 bits and therefore always
/// sort into one CONTIGUOUS run (differing only in the low, mesh/material-
/// derived bits) -- exactly what `cs_range_local_scan`/`cs_range_write`
/// need to find every draw-call group belonging to one range without
/// re-scanning the whole array. An earlier version of this function XORed
/// all three fields into a single 24-bit hash instead, which does NOT
/// preserve this property (`graph_hash`'s contribution gets scrambled by
/// whichever mesh/material bits happened to be set) and produced
/// fragmented, non-contiguous ranges for the same `(class, graph_hash)` --
/// caught by `tests/gpu_object_batch_validation.rs`, not by inspection.
///
/// Both 12-bit hashes CAN still collide (two different `graph_hash` values
/// landing in the same 4096-slot bucket, say) -- exactly as before, this
/// never causes a wrong MERGE: `cs_group_local_scan`/`cs_group_write`
/// compare rows' REAL `mesh_slot`/`material_slot` (`same_draw_group`), and
/// `cs_range_local_scan`/`cs_range_write` compare groups' REAL
/// `(material_class, graph_hash)`, never the hash. A collision only costs a
/// slightly less-tidy internal sort order within the affected bucket.
fn hash12(x: u32) -> u32 {
    var h = x;
    h = h ^ (h >> 16u);
    h = h * 0x45D9F3Bu;
    h = h ^ (h >> 13u);
    return h & 0xFFFu;
}

fn compute_sort_key(row: StaticObjectRow) -> u32 {
    let graph_hash_bucket = hash12(row.graph_hash_lo ^ (row.graph_hash_hi * 0x9E3779B1u));
    let mesh_material_bucket = hash12(row.mesh_slot ^ (row.material_slot * 0x85EBCA6Bu));
    return ((row.material_class & 0xFFu) << 24u) | (graph_hash_bucket << 12u) | mesh_material_bucket;
}

@compute @workgroup_size(WG)
fn cs_gather(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i >= batch.capacity {
        return;
    }
    let row = static_objects[i];
    if row.mesh_generation == 0u {
        // Never written, or retired: a later object in this row must not
        // inherit the dead one's last transform.
        prev_rows[i].identity = vec4<u32>(0u);
        return; // Never written -- Zeroable default, not a live entity.
    }
    let slot = atomicAdd(&gather_count[1], 1u);
    if slot < batch.max_objects {
        gathered_keys[slot] = compute_sort_key(row);
        gathered_indices[slot] = i;
    }
}

// ── Stage 2: cs_prepare ──────────────────────────────────────────────────────

struct FrameUniform {
    count: u32,
    num_blocks: u32,
}
@group(0) @binding(0) var<storage, read> prep_gather_count: array<u32>;
@group(0) @binding(1) var<storage, read_write> frame_uniform_rw: FrameUniform;
@group(0) @binding(2) var<storage, read_write> dispatch_args: array<u32>;

@compute @workgroup_size(1)
fn cs_prepare() {
    let count = prep_gather_count[1]; // gather_count[1] -- see cs_gather's atomicAdd target
    frame_uniform_rw.count = count;
    let num_blocks = max((count + WG - 1u) / WG, 1u);
    frame_uniform_rw.num_blocks = num_blocks;
    // `dispatch_workgroups_indirect` reads all three dimensions -- a y or z
    // of 0 dispatches ZERO total workgroups (x*y*z), not "1 workgroup in x
    // only". Must write all three every time, not just x.
    dispatch_args[0] = num_blocks;
    dispatch_args[1] = 1u;
    dispatch_args[2] = 1u;
}

// ── Stage 3: the sort ────────────────────────────────────────────────────────
//
// `helio_core::radix_sort` (its own shader), over `keys_a`/`indices_a` with
// `keys_b`/`indices_b` as scratch, sized by `frame_uniform`/`dispatch_args`.

// ── Stage 4: cs_final_gather ─────────────────────────────────────────────────

@group(0) @binding(0) var<uniform> fu_fg: FrameUniform;
@group(0) @binding(1) var<storage, read> sorted_indices_fg: array<u32>;
@group(0) @binding(2) var<storage, read> static_objects_fg: array<StaticObjectRow>;

struct GpuInstanceDataOut {
    model: array<array<f32, 4>, 4>,
    normal_mat: array<array<f32, 4>, 3>,
    bounds: array<f32, 4>,
    prev_model: array<array<f32, 4>, 4>,
    mesh_id: u32,
    material_id: u32,
    flags: u32,
    lightmap_index: u32,
    skin_vertex_offset: u32,
    bone_palette_offset: u32,
    bone_palette_count: u32,
    _skin_pad: u32,
}
// Layout mirrors `GpuAabb` in indirect_dispatch.wgsl (32 bytes: min, pad, max, pad).
struct GpuInstanceAabbOut {
    min: array<f32, 3>,
    _pad0: f32,
    max: array<f32, 3>,
    _pad1: f32,
}
@group(0) @binding(3) var<storage, read_write> instances_out: array<GpuInstanceDataOut>;
@group(0) @binding(4) var<storage, read_write> aabbs_out: array<GpuInstanceAabbOut>;
@group(0) @binding(5) var<storage, read_write> prev_rows_fg: array<PrevRow>;

@group(0) @binding(6) var<storage, read_write> sorted_position_fg: array<u32>;
// Per `static_objects` row: non-zero = another renderer (virtual geometry) draws
// it in the main view. The row stays in the batch (shadows keep drawing it).
@group(0) @binding(7) var<storage, read> classic_suppressed_fg: array<u32>;
const INSTANCE_FLAG_CLASSIC_SUPPRESSED: u32 = 1u << 4u; // mirrors helio_pass_object_batch

fn gather_one(i: u32, src: u32) {
    let row = static_objects_fg[src];
    // Previous-frame transform for motion vectors: last frame's transform
    // of this same object, or this frame's (zero motion) when the row is
    // new or was reused by another object. `sorted_indices_fg` is a
    // permutation, so each row is read and written by one invocation.
    let identity = vec4<u32>(row.mesh_slot, row.mesh_generation, row.material_slot, row.material_generation);
    let last = prev_rows_fg[src];
    var row_flags = row.flags;
    if src < arrayLength(&classic_suppressed_fg) && classic_suppressed_fg[src] != 0u {
        row_flags = row_flags | INSTANCE_FLAG_CLASSIC_SUPPRESSED;
    }
    var prev_transform = row.transform;
    if all(last.identity == identity) {
        prev_transform = last.transform;
    }
    prev_rows_fg[src] = PrevRow(row.transform, identity);
    instances_out[i] = GpuInstanceDataOut(
        row.transform,
        row.normal_mat,
        row.bounds,
        prev_transform,
        row.mesh_slot,
        row.material_slot,
        row_flags,
        0xFFFFFFFFu,
        row.skin_vertex_offset,
        row.bone_palette_offset,
        row.bone_palette_count,
        0u,
    );
    // Conservative world AABB: the bounding sphere's box.
    let r = row.bounds[3];
    aabbs_out[i] = GpuInstanceAabbOut(
        array<f32, 3>(row.bounds[0] - r, row.bounds[1] - r, row.bounds[2] - r),
        0.0,
        array<f32, 3>(row.bounds[0] + r, row.bounds[1] + r, row.bounds[2] + r),
        0.0,
    );
}

@compute @workgroup_size(WG)
fn cs_final_gather(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i >= fu_fg.count {
        return;
    }
    let src = sorted_indices_fg[i];
    sorted_position_fg[src] = i;
    gather_one(i, src);
}

// ── Stage 4b: placements of only the moved objects ──────────────────────────
//
// The scene join lists the object rows that moved (`[total, fresh, slot, ..]`,
// this frame's and the previous frame's, which settles their motion vectors).
// Each is re-gathered into its sorted slot through the position the last full
// run kept; everything else in the sorted arrays already stands.

@group(1) @binding(0) var<storage, read> moved_list: array<u32>;
@group(1) @binding(1) var<storage, read_write> moved_args: array<u32>;

@compute @workgroup_size(1)
fn cs_moved_args() {
    moved_args[0] = (moved_list[0] + WG - 1u) / WG;
    moved_args[1] = 1u;
    moved_args[2] = 1u;
}

@compute @workgroup_size(WG)
fn cs_final_gather_moved(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= moved_list[0] {
        return;
    }
    let src = moved_list[2u + gid.x];
    if src >= arrayLength(&sorted_position_fg) {
        return;
    }
    let i = sorted_position_fg[src];
    // Rows the last full run did not sort (or stale entries) have no slot.
    if i >= fu_fg.count || sorted_indices_fg[i] != src {
        return;
    }
    gather_one(i, src);
}

// ── Stage 5: group boundary detection (two-level scan) ──────────────────────
//
// Boundaries here MUST compare the real `(mesh_slot, material_slot)` pair
// (via `static_objects[sorted_indices[i]]`), never just `sorted_keys[i]`.
// The sort key is a 24-bit HASH (see `compute_sort_key`'s doc) -- two
// genuinely different `(mesh, material)` pairs can collide onto the same
// key, and the radix sort would then place them adjacently. Treating equal
// KEYS as equal GROUPS would silently merge those two different objects
// into one draw call, using only the first one's mesh/material for the
// whole group -- a real (if rare) rendering-correctness bug, not just a
// missed-optimization one. Comparing the real fields makes a collision cost
// only a slightly less-tidy sort order (two unrelated groups sharing a
// key-driven sort position), never a wrong merge.

@group(0) @binding(0) var<uniform> fu_gls: FrameUniform;
@group(0) @binding(1) var<storage, read> sorted_indices_gls: array<u32>;
@group(0) @binding(2) var<storage, read> static_objects_gls: array<StaticObjectRow>;
@group(0) @binding(3) var<storage, read_write> local_group_rank: array<u32>;
@group(0) @binding(4) var<storage, read_write> block_group_totals: array<u32>;

var<workgroup> group_scan_buf: array<u32, 256>;

fn same_draw_group(a: StaticObjectRow, b: StaticObjectRow) -> bool {
    return a.mesh_slot == b.mesh_slot && a.material_slot == b.material_slot;
}

// Downstream frustum/occlusion kernels assign one workgroup to each draw.
// An unbounded run of identical mesh/material rows otherwise serializes
// millions of instances onto 64 lanes. Split sorted runs at fixed block
// boundaries; material ranges still coalesce the resulting adjacent draws.
const MAX_INSTANCES_PER_DRAW: u32 = 4096u;

@compute @workgroup_size(WG)
fn cs_group_local_scan(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(workgroup_id) wgid: vec3<u32>,
) {
    let has_elem = gid.x < fu_gls.count;
    var is_start = 0u;
    if has_elem {
        if gid.x % MAX_INSTANCES_PER_DRAW == 0u {
            is_start = 1u;
        } else {
            let cur = static_objects_gls[sorted_indices_gls[gid.x]];
            let prev = static_objects_gls[sorted_indices_gls[gid.x - 1u]];
            if !same_draw_group(cur, prev) {
                is_start = 1u;
            }
        }
    }
    group_scan_buf[lid.x] = is_start;
    workgroupBarrier();

    var offset = 1u;
    loop {
        if offset >= WG {
            break;
        }
        var v = 0u;
        if lid.x >= offset {
            v = group_scan_buf[lid.x - offset];
        }
        workgroupBarrier();
        group_scan_buf[lid.x] += v;
        workgroupBarrier();
        offset = offset * 2u;
    }

    if has_elem {
        local_group_rank[gid.x] = group_scan_buf[lid.x];
    }
    if lid.x == WG - 1u {
        block_group_totals[wgid.x] = group_scan_buf[lid.x];
    }
}

@group(0) @binding(0) var<uniform> fu_gbs: FrameUniform;
@group(0) @binding(1) var<storage, read_write> block_group_totals_s: array<u32>;
/// `[0]` becomes `group_count` after this runs -- turned into an indirect
/// dispatch size for stages 6/7 by `cs_prepare_groups` right below (fully
/// GPU-resident; also read back to the CPU afterward, small and bounded,
/// alongside the range tables -- see `src/lib.rs`).
@group(0) @binding(2) var<storage, read_write> group_count_out: array<u32>;

@compute @workgroup_size(1)
fn cs_group_block_scan() {
    var running = 0u;
    for (var blk = 0u; blk < fu_gbs.num_blocks; blk++) {
        let total = block_group_totals_s[blk];
        block_group_totals_s[blk] = running; // becomes each block's base
        running += total;
    }
    group_count_out[0] = running;
}

@group(0) @binding(0) var<uniform> fu_gw: FrameUniform;
@group(0) @binding(1) var<storage, read> sorted_indices_gw: array<u32>;
@group(0) @binding(2) var<storage, read> static_objects_gw: array<StaticObjectRow>;
@group(0) @binding(3) var<storage, read> local_group_rank_gw: array<u32>;
@group(0) @binding(4) var<storage, read> block_group_base_gw: array<u32>;
@group(0) @binding(5) var<storage, read_write> group_starts: array<u32>;

@compute @workgroup_size(WG)
fn cs_group_write(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(workgroup_id) wgid: vec3<u32>,
) {
    let i = gid.x;
    if i >= fu_gw.count {
        return;
    }
    var is_start = i % MAX_INSTANCES_PER_DRAW == 0u;
    if !is_start {
        let cur = static_objects_gw[sorted_indices_gw[i]];
        let prev = static_objects_gw[sorted_indices_gw[i - 1u]];
        is_start = !same_draw_group(cur, prev);
    }
    if !is_start {
        return;
    }
    let global_group_index = block_group_base_gw[wgid.x] + local_group_rank_gw[i] - 1u;
    group_starts[global_group_index] = i;
}

@group(0) @binding(0) var<storage, read> final_count: array<u32>;
@group(0) @binding(1) var<storage, read_write> group_starts_sentinel: array<u32>;
@group(0) @binding(2) var<storage, read> group_count_sentinel: array<u32>;

@compute @workgroup_size(1)
fn cs_group_write_sentinel() {
    let gc = group_count_sentinel[0];
    group_starts_sentinel[gc] = final_count[0];
}

/// Turns the just-computed `group_count` into an indirect dispatch size for
/// every group-count-indexed stage below (`cs_build_draw_calls`, `cs_range_
/// local_scan`, `cs_range_write`) -- same technique as `cs_prepare`, just
/// keyed off `group_count` instead of the gather's live-object count.
///
/// Words 3..6 are a second dispatch size, one workgroup per group, for the
/// culling passes downstream (`ObjectBatchFrameData::group_dispatch`), so
/// they cull exactly this frame's groups without a CPU readback. Capped at
/// WebGPU's 65535 workgroups per dimension.
@group(0) @binding(0) var<storage, read> group_count_pg: array<u32>;
@group(0) @binding(1) var<storage, read_write> dispatch_args_groups: array<u32>;

@compute @workgroup_size(1)
fn cs_prepare_groups() {
    dispatch_args_groups[0] = max((group_count_pg[0] + WG - 1u) / WG, 1u);
    dispatch_args_groups[1] = 1u;
    dispatch_args_groups[2] = 1u;
    dispatch_args_groups[3] = min(group_count_pg[0], 65535u);
    dispatch_args_groups[4] = 1u;
    dispatch_args_groups[5] = 1u;
}

// ── Stage 6: cs_build_draw_calls ────────────────────────────────────────────

struct GpuDrawCallOut {
    index_count: u32,
    first_index: u32,
    vertex_offset: i32,
    first_instance: u32,
    instance_count: u32,
}

/// Same five fields as `GpuDrawCallOut`, reordered to `wgpu`'s hardware
/// indirect-draw ABI (`index_count, instance_count, first_index,
/// base_vertex, first_instance`) -- `multi_draw_indexed_indirect` reads
/// this exact byte layout directly off the GPU, so it needs its own buffer
/// rather than reusing `draw_calls_out`. Matches `libhelio::
/// DrawIndexedIndirectArgs` field-for-field -- also reused by stage 8's
/// shadow-partition buffers below, which need the identical hardware ABI.
struct DrawIndexedIndirectArgsOut {
    index_count: u32,
    instance_count: u32,
    first_index: u32,
    base_vertex: i32,
    first_instance: u32,
}

// `GpuMaterial` and its flags: the materials' row (`//!use material_row`);
// only `.flags` is read here.

@group(0) @binding(0) var<storage, read> group_count_bdc: array<u32>;
@group(0) @binding(1) var<storage, read> group_starts_bdc: array<u32>;
@group(0) @binding(2) var<storage, read> sorted_indices_bdc: array<u32>;
@group(0) @binding(3) var<storage, read> static_objects_bdc: array<StaticObjectRow>;
@group(0) @binding(4) var<storage, read_write> draw_calls_out: array<GpuDrawCallOut>;
@group(0) @binding(5) var<storage, read_write> group_material_class: array<u32>;
@group(0) @binding(6) var<storage, read_write> group_graph_hash_lo: array<u32>;
@group(0) @binding(7) var<storage, read_write> group_graph_hash_hi: array<u32>;
@group(0) @binding(8) var<storage, read_write> group_shading: array<u32>; // bit0=transparent, bit1=forward
@group(0) @binding(9) var<storage, read> materials_bdc: array<GpuMaterial>;
@group(0) @binding(10) var<storage, read_write> indirect_out: array<DrawIndexedIndirectArgsOut>;


@compute @workgroup_size(WG)
fn cs_build_draw_calls(@builtin(global_invocation_id) gid: vec3<u32>) {
    let g = gid.x;
    if g >= group_count_bdc[0] {
        return;
    }
    let start = group_starts_bdc[g];
    let end = group_starts_bdc[g + 1u];
    let row = static_objects_bdc[sorted_indices_bdc[start]];

    let instance_count = end - start;
    draw_calls_out[g] = GpuDrawCallOut(
        row.index_count,
        row.first_index,
        row.vertex_offset,
        start,
        instance_count,
    );
    indirect_out[g] = DrawIndexedIndirectArgsOut(
        row.index_count,
        instance_count,
        row.first_index,
        row.vertex_offset,
        start,
    );
    // Range/segment key: low 16 bits are the template class; upper bits
    // carry raster state. Double-sided cards must not share an opaque PSO.
    group_material_class[g] = row.material_class | ((materials_bdc[row.material_slot].flags & (MATERIAL_FLAG_DOUBLE_SIDED | MATERIAL_FLAG_ALPHA_TEST)) << 16u);
    group_graph_hash_lo[g] = row.graph_hash_lo;
    group_graph_hash_hi[g] = row.graph_hash_hi;

    // Shading category is a MATERIAL property (read straight from the
    // shared `materials` buffer by this group's first instance's material
    // slot) -- fully GPU-resident, no CPU round-trip needed between this
    // stage and stage 7's opaque/transparent/forward split.
    let mat_flags = materials_bdc[row.material_slot].flags;
    var shading = 0u;
    if (mat_flags & MATERIAL_FLAG_TRANSPARENT_ONLY) != 0u {
        shading |= 1u;
    }
    if (mat_flags & MATERIAL_FLAG_FORWARD_SHADING) != 0u {
        shading |= 2u;
    }
    group_shading[g] = shading;
}

// ── Stage 7: range boundary detection (two-level scan, over GROUPS) ────────

@group(0) @binding(0) var<storage, read> group_count_rls: array<u32>;
@group(0) @binding(1) var<storage, read> group_material_class_rls: array<u32>;
@group(0) @binding(2) var<storage, read> group_graph_hash_lo_rls: array<u32>;
@group(0) @binding(3) var<storage, read> group_graph_hash_hi_rls: array<u32>;
@group(0) @binding(4) var<storage, read_write> local_range_rank: array<u32>;
@group(0) @binding(5) var<storage, read_write> block_range_totals: array<u32>;
@group(0) @binding(6) var<storage, read> group_shading_rls: array<u32>;

var<workgroup> opaque_range_scan: array<u32, 256>;
var<workgroup> transparent_range_scan: array<u32, 256>;
var<workgroup> forward_range_scan: array<u32, 256>;

@compute @workgroup_size(WG)
fn cs_range_local_scan(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(workgroup_id) wgid: vec3<u32>,
) {
    let count = group_count_rls[0];
    let has_elem = gid.x < count;
    var is_start = 0u;
    var shading = 0u;
    if has_elem {
        shading = group_shading_rls[gid.x];
        if gid.x == 0u {
            is_start = 1u;
        } else {
            let i = gid.x;
            if group_material_class_rls[i] != group_material_class_rls[i - 1u]
                || group_graph_hash_lo_rls[i] != group_graph_hash_lo_rls[i - 1u]
                || group_graph_hash_hi_rls[i] != group_graph_hash_hi_rls[i - 1u]
                || group_shading_rls[i] != group_shading_rls[i - 1u] {
                is_start = 1u;
            }
        }
    }
    let opaque_start = select(0u, 1u, is_start != 0u && (shading & 3u) == 0u);
    let transparent_start = select(0u, 1u, is_start != 0u && (shading & 3u) == 1u);
    let forward_start = select(0u, 1u, is_start != 0u && (shading & 2u) != 0u);
    opaque_range_scan[lid.x] = opaque_start;
    transparent_range_scan[lid.x] = transparent_start;
    forward_range_scan[lid.x] = forward_start;
    workgroupBarrier();

    var offset = 1u;
    loop {
        if offset >= WG {
            break;
        }
        var opaque_value = 0u;
        var transparent_value = 0u;
        var forward_value = 0u;
        if lid.x >= offset {
            opaque_value = opaque_range_scan[lid.x - offset];
            transparent_value = transparent_range_scan[lid.x - offset];
            forward_value = forward_range_scan[lid.x - offset];
        }
        workgroupBarrier();
        opaque_range_scan[lid.x] += opaque_value;
        transparent_range_scan[lid.x] += transparent_value;
        forward_range_scan[lid.x] += forward_value;
        workgroupBarrier();
        offset = offset * 2u;
    }

    if has_elem {
        local_range_rank[gid.x * 3u] = opaque_range_scan[lid.x];
        local_range_rank[gid.x * 3u + 1u] = transparent_range_scan[lid.x];
        local_range_rank[gid.x * 3u + 2u] = forward_range_scan[lid.x];
    }
    if lid.x == WG - 1u {
        block_range_totals[wgid.x * 3u] = opaque_range_scan[lid.x];
        block_range_totals[wgid.x * 3u + 1u] = transparent_range_scan[lid.x];
        block_range_totals[wgid.x * 3u + 2u] = forward_range_scan[lid.x];
    }
}

struct RangeBlockUniform {
    num_blocks: u32,
}
@group(0) @binding(0) var<uniform> rbu: RangeBlockUniform;
@group(0) @binding(1) var<storage, read_write> block_range_totals_s: array<u32>;
@group(0) @binding(2) var<storage, read_write> range_count_out: array<u32>;

@compute @workgroup_size(1)
fn cs_range_block_scan() {
    var total_ranges = 0u;
    for (var bucket = 0u; bucket < 3u; bucket++) {
        var running = 0u;
        for (var blk = 0u; blk < rbu.num_blocks; blk++) {
            let index = blk * 3u + bucket;
            let total = block_range_totals_s[index];
            block_range_totals_s[index] = running;
            running += total;
        }
        range_count_out[bucket] = running;
        let dispatch_args = 4u + bucket * 3u;
        // Keep each indirect dispatch dimension within WebGPU's 65535
        // workgroup limit. The consumer reconstructs the flattened slot.
        range_count_out[dispatch_args] = min(running, 65535u);
        range_count_out[dispatch_args + 1u] = max(
            1u,
            running / 65535u + select(0u, 1u, running % 65535u != 0u),
        );
        range_count_out[dispatch_args + 2u] = 1u;
        total_ranges += running;
    }
    range_count_out[3] = total_ranges;
}

struct GpuRangeOut {
    material_class: u32,
    graph_hash_lo: u32,
    graph_hash_hi: u32,
    start: u32,
    count: u32,
}

@group(0) @binding(0) var<storage, read> group_count_rw: array<u32>;
@group(0) @binding(1) var<storage, read> group_material_class_rw: array<u32>;
@group(0) @binding(2) var<storage, read> group_graph_hash_lo_rw: array<u32>;
@group(0) @binding(3) var<storage, read> group_graph_hash_hi_rw: array<u32>;
@group(0) @binding(4) var<storage, read> group_shading_rw: array<u32>;
@group(0) @binding(5) var<storage, read> local_range_rank_rw: array<u32>;
@group(0) @binding(6) var<storage, read> block_range_base_rw: array<u32>;
@group(0) @binding(7) var<storage, read_write> opaque_ranges: array<GpuRangeOut>;
@group(0) @binding(8) var<storage, read_write> transparent_ranges: array<GpuRangeOut>;
@group(0) @binding(9) var<storage, read_write> forward_ranges: array<GpuRangeOut>;
@group(0) @binding(10) var<storage, read_write> draw_counts_out: array<u32>;
@compute @workgroup_size(WG)
fn cs_range_write(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(workgroup_id) wgid: vec3<u32>,
) {
    let count = group_count_rw[0];
    let g = gid.x;
    if g >= count {
        return;
    }
    var is_start = false;
    if g == 0u {
        is_start = true;
    } else if group_material_class_rw[g] != group_material_class_rw[g - 1u]
        || group_graph_hash_lo_rw[g] != group_graph_hash_lo_rw[g - 1u]
        || group_graph_hash_hi_rw[g] != group_graph_hash_hi_rw[g - 1u]
        || group_shading_rw[g] != group_shading_rw[g - 1u] {
        is_start = true;
    }
    if !is_start {
        return;
    }

    // Find this range's end: the next boundary, or `count` for the last one.
    // Cheap linear walk -- ranges are, by construction, contiguous runs of
    // groups sharing one (class, graph_hash); a real scene has tens to low
    // hundreds of groups, so this never approaches a hot loop.
    var end = g + 1u;
    loop {
        if end >= count {
            break;
        }
        if group_material_class_rw[end] != group_material_class_rw[g]
            || group_graph_hash_lo_rw[end] != group_graph_hash_lo_rw[g]
            || group_graph_hash_hi_rw[end] != group_graph_hash_hi_rw[g]
            || group_shading_rw[end] != group_shading_rw[g] {
            break;
        }
        end += 1u;
    }

    let range = GpuRangeOut(
        group_material_class_rw[g],
        group_graph_hash_lo_rw[g],
        group_graph_hash_hi_rw[g],
        g,
        end - g,
    );
    let shading = group_shading_rw[g];
    let is_transparent = (shading & 1u) != 0u;
    let is_forward = (shading & 2u) != 0u;
    let range_capacity = (arrayLength(&draw_counts_out) - 4u) / 3u;
    if is_forward {
        let slot = block_range_base_rw[wgid.x * 3u + 2u]
            + local_range_rank_rw[g * 3u + 2u] - 1u;
        forward_ranges[slot] = range;
        draw_counts_out[4u + 2u * range_capacity + slot] = range.count;
    } else if is_transparent {
        let slot = block_range_base_rw[wgid.x * 3u + 1u]
            + local_range_rank_rw[g * 3u + 1u] - 1u;
        transparent_ranges[slot] = range;
        draw_counts_out[4u + range_capacity + slot] = range.count;
    } else {
        let slot = block_range_base_rw[wgid.x * 3u]
            + local_range_rank_rw[g * 3u] - 1u;
        opaque_ranges[slot] = range;
        draw_counts_out[4u + slot] = range.count;
    }
}

// ── Stage 8: cs_shadow_partition ─────────────────────────────────────────────
//
// Opaque casters split by mobility into the static/movable depth atlases.
// Transmissive (transparent-only) static casters go to their own list: the
// shadow pass renders them into the coloured transmittance layer instead of
// the depth atlas, so light passes through glass tinted rather than blocked.

const INSTANCE_FLAG_MOVABLE: u32 = 1u << 3u; // mirrors libhelio::INSTANCE_FLAG_MOVABLE

@group(0) @binding(0) var<uniform> fu_sp: FrameUniform;
@group(0) @binding(1) var<storage, read> sorted_indices_sp: array<u32>;
@group(0) @binding(2) var<storage, read> static_objects_sp: array<StaticObjectRow>;
@group(0) @binding(3) var<storage, read_write> shadow_static_indirect: array<DrawIndexedIndirectArgsOut>;
@group(0) @binding(4) var<storage, read_write> shadow_movable_indirect: array<DrawIndexedIndirectArgsOut>;
@group(0) @binding(5) var<storage, read_write> shadow_counts: array<atomic<u32>>; // [static, movable, transmissive]
@group(0) @binding(6) var<storage, read> materials_sp: array<GpuMaterial>;
@group(0) @binding(7) var<storage, read_write> shadow_transmissive_indirect: array<DrawIndexedIndirectArgsOut>;

fn shadow_membership_hash(value: u32, salt: u32) -> u32 {
    var x = value ^ salt;
    x = (x ^ (x >> 16u)) * 0x7feb352du;
    x = (x ^ (x >> 15u)) * 0x846ca68bu;
    return x ^ (x >> 16u);
}

@compute @workgroup_size(WG)
fn cs_shadow_partition(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i >= fu_sp.count {
        return;
    }
    let row = static_objects_sp[sorted_indices_sp[i]];
    let entry = DrawIndexedIndirectArgsOut(
        row.index_count,
        1u,
        row.first_index,
        row.vertex_offset,
        i,
    );
    let transmissive = (materials_sp[row.material_slot].flags & MATERIAL_FLAG_TRANSPARENT_ONLY) != 0u;
    if transmissive {
        // Moving glass casts no coloured shadow (the transmittance layer is
        // cached with the static atlas); it never casts an opaque one.
        if (row.flags & INSTANCE_FLAG_MOVABLE) == 0u {
            let identity = sorted_indices_sp[i];
            atomicXor(&shadow_counts[3], shadow_membership_hash(identity, 0x9e3779b9u));
            atomicXor(&shadow_counts[4], shadow_membership_hash(identity, 0x85ebca6bu));
            let slot = atomicAdd(&shadow_counts[2], 1u);
            shadow_transmissive_indirect[slot] = entry;
        }
    } else if (row.flags & INSTANCE_FLAG_MOVABLE) != 0u {
        let slot = atomicAdd(&shadow_counts[1], 1u);
        shadow_movable_indirect[slot] = entry;
    } else {
        let identity = sorted_indices_sp[i];
        atomicXor(&shadow_counts[3], shadow_membership_hash(identity, 0x9e3779b9u));
        atomicXor(&shadow_counts[4], shadow_membership_hash(identity, 0x85ebca6bu));
        let slot = atomicAdd(&shadow_counts[0], 1u);
        shadow_static_indirect[slot] = entry;
    }
}
