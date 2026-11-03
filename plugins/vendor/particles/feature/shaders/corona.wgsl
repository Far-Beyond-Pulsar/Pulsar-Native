// ── Corona GPU Particle System ────────────────────────────────────────────────
//
// Compute pipeline (per frame):
//   0. cs_layout          — per-emitter restarts; the pool's used range → indirect dispatch args
//   1. cs_simulate        — physics, aging, kill expired and orphaned particles
//   2. cs_emit            — ring-buffer spawn per emitter (tags velocity.w with emitter + epoch)
//   3. cs_scan_local      — Hillis-Steele inclusive prefix scan per 256-block + sort-key reset
//   4. cs_scan_blocks     — sequential cumulative sum per emitter; writes emitter_alive totals
//   5. cs_scatter         — scatter alive indices into compact_buf + write view-depth to sort_key_buf
//   6. cs_build_multi     — write one DrawArgs per emitter
//   copy draw_args_staging → draw_args_buf
//   7. cs_sort_local      — bitonic sort within 256-element blocks (shared memory)
//   8. cs_sort_global     — one compare-swap step for large j (global memory, per dispatch)
//   9. cs_sort_local_merge— j < 256 steps for large-k stages (shared memory, per k-stage)

const PI:          f32 = 3.14159265359;
const INV_MAX_U32: f32 = 1.0 / 4294967295.0;
// Stay just below the largest finite f32. The previously rounded decimal was
// above the representable range in Dawn's strict WebGPU WGSL parser.
const F32_MAX:     f32 = 3.4028234e38;
const WG:          u32 = 256u;
// A particle's tag (`velocity.w`, an integer held exactly in an f32): its
// emitter's row plus `TAG_EMITTERS` times that row's epoch (mod `TAG_EPOCHS`).
// `TAG_EMITTERS` is `CORONA_MAX_EMITTERS`.
const TAG_EMITTERS: u32 = 64u;
const TAG_EPOCHS:   u32 = 16384u;

// ── Structs ───────────────────────────────────────────────────────────────────

struct GpuCoronaUniforms {
    delta_time:      f32,
    total_particles: u32,
    emitter_count:   u32,
    frame_count:     u32,
    // Sort params: written per-dispatch via partial copy into this uniform buffer.
    // Only valid inside cs_sort_* entry points.
    sort_k:          u32,
    sort_j:          u32,
    sort_lo:         u32,
    sort_n:          u32,
    // How far the frame's world origin moved since the last simulation
    // step (xyz); live particles subtract it to stay where they were.
    origin_shift:    vec4<f32>,
}

struct Particle {
    pos_and_alive:     vec4<f32>,  // xyz=position, w=alive (0 or 1)
    // velocity xyz + w=emitter_index (f32-encoded u32, written by cs_emit)
    velocity:          vec4<f32>,
    color:             vec4<f32>,
    size_lifetime_age: vec4<f32>,  // x=size, y=unused, z=lifetime, w=age
}

struct EmitterDef {
    transform:          mat4x4<f32>,
    emit_params:        vec4<f32>,        // emit_rate, lifetime, lifetime_var, gravity
    size_params:        vec4<f32>,        // start_size.xy, end_size.xy
    start_color:        vec4<f32>,
    end_color:          vec4<f32>,
    velocity:           vec4<f32>,
    velocity_variation: vec4<f32>,
    extras:             vec4<f32>,        // shape_type, radius, pad, active
    texture_index:      i32,
    particle_offset:    u32,
    particle_count:     u32,
    // Read as the emitter's identity (the environment join writes its source
    // row + 1): when it or the range changes, the row's particles restart.
    spawn_cursor:       u32,
    _pad:               array<f32, 12>,
}

// Pass-owned state per emitter row (see `cs_layout`).
struct EmitterState {
    cursor:   u32,
    identity: u32,
    // The range last frame: a change restarts the row's particles.
    offset:   u32,
    count:    u32,
    // Bumped on every restart; a particle of another epoch is dead.
    epoch:    u32,
    _pad0:    u32,
    _pad1:    u32,
    _pad2:    u32,
}

struct DrawArgs {
    vertex_count:   u32,
    instance_count: u32,
    first_vertex:   u32,
    first_instance: u32,
}

struct CameraUniforms {
    view:           mat4x4<f32>,   // offset   0
    proj:           mat4x4<f32>,   // offset  64
    view_proj:      mat4x4<f32>,   // offset 128
    inv_view_proj:  mat4x4<f32>,   // offset 192
    position_near:  vec4<f32>,
    forward_far:    vec4<f32>,
    jitter_frame:   vec4<f32>,
    prev_view_proj: mat4x4<f32>,
}

// ── Compute bindings ──────────────────────────────────────────────────────────

@group(0) @binding(0)  var<uniform>             uniforms:          GpuCoronaUniforms;
@group(0) @binding(1)  var<storage, read_write>  particles:         array<Particle>;
@group(0) @binding(2)  var<storage, read_write>  emitters:          array<EmitterDef>;
@group(0) @binding(3)  var<storage, read_write>  compact_buf:       array<u32>;
// Non-atomic now; written by cs_scan_blocks, read by cs_build_multi.
@group(0) @binding(4)  var<storage, read_write>  emitter_alive:     array<u32>;
@group(0) @binding(5)  var<storage, read_write>  draw_args_staging: array<DrawArgs>;
@group(0) @binding(6)  var<storage, read> cameras: array<CameraUniforms, 2>;
// exclusive prefix within each 256-block (written by cs_scan_local, read by cs_scatter)
@group(0) @binding(7)  var<storage, read_write>  prefix_buf:        array<u32>;
// per-block alive totals (cs_scan_local) → per-block cumulative offsets (cs_scan_blocks)
@group(0) @binding(8)  var<storage, read_write>  block_sums_buf:    array<u32>;
// view-space depth key per compact_buf slot; reset to -F32_MAX, then written by cs_scatter
@group(0) @binding(9)  var<storage, read_write>  sort_key_buf:      array<f32>;
// Per-emitter-row spawn cursor and restart state -- purely transient,
// pass-owned GPU state, deliberately NOT part of `EmitterDef`/`emitters`
// (see the Rust side's `spawn_cursor_buf` doc for why: those rows are
// re-written whenever their source changes, which would stomp an in-place
// cursor advance living in the same row).
@group(0) @binding(12) var<storage, read_write>  emitter_state:     array<EmitterState>;
// Written by cs_layout: x = workgroups covering the pool's used range (then
// y = z = 1, copied into the indirect dispatch args of the particle-wide
// passes), w = the end of that range.
@group(0) @binding(13) var<storage, read_write>  pool_layout:       vec4<u32>;

// ── Workgroup shared memory ───────────────────────────────────────────────────

// Used by cs_scan_local (prefix scan) and cs_sort_local / cs_sort_local_merge.
// These entry points never run concurrently, so it's safe to share the arrays.
var<workgroup> wg_scratch: array<u32, 256>;  // scratch for prefix scan
var<workgroup> sh_keys:    array<f32, 256>;  // sort keys (depths)
var<workgroup> sh_vals:    array<u32, 256>;  // sort values (compact_buf indices)

// ── RNG ───────────────────────────────────────────────────────────────────────

fn hash(x: u32) -> u32 {
    var h = x;
    h = (h ^ (h >> 16u)) * 0x85ebca6bu;
    h = (h ^ (h >> 13u)) * 0xc2b2ae35u;
    return h ^ (h >> 16u);
}
fn rng_f32(seed: u32) -> f32 { return f32(hash(seed)) * INV_MAX_U32; }
fn rng_range(seed: u32, lo: f32, hi: f32) -> f32 { return lo + rng_f32(seed) * (hi - lo); }

// ── Pool ranges ───────────────────────────────────────────────────────────────

/// Emitter `e`'s particle range in the shared pool: (first particle, count).
/// Its row carries it (`particle_offset`, `particle_count`; the environment
/// join allocates them). A range must start on a `WG` boundary, so a scan
/// block never holds two emitters' particles; one that does not, or that
/// starts past the pool, is empty, and one that runs past the pool's end is
/// clamped to it.
fn emitter_range(e: u32) -> vec2<u32> {
    if e >= uniforms.emitter_count { return vec2<u32>(0u, 0u); }
    let offset = emitters[e].particle_offset;
    if offset % WG != 0u || offset >= uniforms.total_particles { return vec2<u32>(offset, 0u); }
    return vec2<u32>(offset, min(emitters[e].particle_count, uniforms.total_particles - offset));
}

/// The tag of a particle emitter row `e` spawns now.
fn particle_tag(e: u32) -> f32 {
    return f32(e + TAG_EMITTERS * (emitter_state[e].epoch % TAG_EPOCHS));
}

/// The emitter particle `idx` was spawned by (its tag), if that row has not
/// restarted since and its range still holds `idx`;
/// `uniforms.emitter_count` otherwise (its emitter was removed or replaced,
/// or the ranges were re-allocated).
fn owner_of(idx: u32, p: Particle) -> u32 {
    let tag = u32(max(p.velocity.w, 0.0) + 0.5);
    let e = tag % TAG_EMITTERS;
    let range = emitter_range(e);
    if range.y == 0u || idx < range.x || idx >= range.x + range.y
        || tag / TAG_EMITTERS != emitter_state[e].epoch % TAG_EPOCHS {
        return uniforms.emitter_count;
    }
    return e;
}

// ── cs_layout ─────────────────────────────────────────────────────────────────
// A row whose emitter (its identity) or range changed restarts: its epoch
// moves on, so the particles it had die in `cs_simulate`, and its cursor
// starts over. The particle-wide passes cover only the pool's used range:
// its end is the furthest emitter range's end, computed here every frame
// (no CPU readback), and their dispatch is indirect from `pool_layout`.

@compute @workgroup_size(1)
fn cs_layout() {
    var end = 0u;
    for (var e = 0u; e < uniforms.emitter_count; e++) {
        let range = emitter_range(e);
        let identity = emitters[e].spawn_cursor;
        var state = emitter_state[e];
        if state.identity != identity || state.offset != range.x || state.count != range.y {
            state.identity = identity;
            state.offset = range.x;
            state.count = range.y;
            state.cursor = 0u;
            state.epoch += 1u;
            emitter_state[e] = state;
        }
        if range.y > 0u {
            end = max(end, range.x + range.y);
        }
    }
    pool_layout = vec4<u32>((end + WG - 1u) / WG, 1u, 1u, end);
}

// ── cs_simulate ───────────────────────────────────────────────────────────────

@compute @workgroup_size(256)
fn cs_simulate(@builtin(global_invocation_id) id: vec3<u32>) {
    let idx = id.x;
    if idx >= uniforms.total_particles { return; }

    var p = particles[idx];
    if p.pos_and_alive.w < 0.5 { return; }

    // A particle whose emitter no longer owns its slot dies.
    let e = owner_of(idx, p);
    p.size_lifetime_age.w += uniforms.delta_time;
    if e >= uniforms.emitter_count || p.size_lifetime_age.w >= p.size_lifetime_age.z {
        p.pos_and_alive.w = 0.0;
        particles[idx] = p;
        return;
    }

    let shift = uniforms.origin_shift.xyz;
    let px = p.pos_and_alive.x - shift.x + p.velocity.x * uniforms.delta_time;
    let py = p.pos_and_alive.y - shift.y + p.velocity.y * uniforms.delta_time;
    let pz = p.pos_and_alive.z - shift.z + p.velocity.z * uniforms.delta_time;
    p.pos_and_alive = vec4<f32>(px, py, pz, 1.0);

    let em        = emitters[e];
    let grav      = em.emit_params.w;
    let start_col = em.start_color;
    let end_col   = em.end_color;
    let start_sz  = em.size_params.xy;
    let end_sz    = em.size_params.zw;

    p.velocity.y += grav * uniforms.delta_time;

    let t  = p.size_lifetime_age.w / max(p.size_lifetime_age.z, 0.001);
    p.color = mix(start_col, end_col, vec4<f32>(t));
    let sz  = mix(start_sz, end_sz, vec2<f32>(t));
    p.size_lifetime_age.x = sz.x;
    p.size_lifetime_age.y = sz.y;

    particles[idx] = p;
}

// ── cs_emit ───────────────────────────────────────────────────────────────────

@compute @workgroup_size(1)
fn cs_emit(@builtin(workgroup_id) id: vec3<u32>) {
    let eidx = id.x;
    if eidx >= uniforms.emitter_count { return; }

    let em = emitters[eidx];
    if em.extras.w < 0.5 { return; }

    let count = u32(em.emit_params.x * uniforms.delta_time);
    if count == 0u { return; }

    let pool_range = emitter_range(eidx);
    if pool_range.y == 0u { return; }
    let base   = pool_range.x;
    let range  = pool_range.y;
    let origin = em.transform[3].xyz;
    let etype  = u32(em.extras.x);
    let radius = em.extras.y;
    let seed   = eidx * 997u + uniforms.frame_count * 7919u;

    var cursor = emitter_state[eidx].cursor % range;
    let tag = particle_tag(eidx);
    for (var i = 0u; i < count; i++) {
        let this_cursor = cursor;
        cursor = (cursor + 1u) % range;

        let pidx = base + this_cursor;
        let s    = seed + i * 1013u;

        var spawn_pos: vec3<f32>;
        if etype >= 1u {
            let theta = rng_f32(s + 1u) * 2.0 * PI;
            let phi   = rng_f32(s + 3u) * PI;
            let r     = rng_f32(s + 5u) * radius;
            spawn_pos = origin + vec3<f32>(
                r * sin(phi) * cos(theta),
                r * cos(phi),
                r * sin(phi) * sin(theta),
            );
        } else {
            spawn_pos = origin;
        }

        let vv  = em.velocity_variation.xyz;
        let vel = em.velocity.xyz + vec3<f32>(
            rng_range(s + 7u,  -vv.x, vv.x),
            rng_range(s + 11u, -vv.y, vv.y),
            rng_range(s + 13u, -vv.z, vv.z),
        );
        let life = em.emit_params.y + rng_range(s + 17u, -em.emit_params.z, em.emit_params.z);

        var p: Particle;
        p.pos_and_alive     = vec4<f32>(spawn_pos, 1.0);
        // Tag the particle with its emitter (atlas lookup in vs_main) and epoch.
        p.velocity          = vec4<f32>(vel, tag);
        p.color             = em.start_color;
        p.size_lifetime_age = vec4<f32>(em.size_params.x, em.size_params.y, max(life, 0.01), 0.0);
        particles[pidx] = p;
    }

    emitter_state[eidx].cursor = cursor;
}

// ── cs_scan_local ─────────────────────────────────────────────────────────────
// Hillis-Steele inclusive prefix scan over alive flags within each 256-element
// block. Writes:
//   - prefix_buf[idx]      = exclusive prefix (alive count before this slot in block)
//   - block_sums_buf[wid]  = total alive in this block
//   - sort_key_buf[idx]    = -F32_MAX (sentinel for dead slots; overwritten by cs_scatter)

@compute @workgroup_size(256)
fn cs_scan_local(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_id)  lid: vec3<u32>,
    @builtin(workgroup_id)         wid: vec3<u32>,
) {
    let idx = gid.x;
    let alive_flag = select(0u, 1u,
        idx < uniforms.total_particles && particles[idx].pos_and_alive.w >= 0.5);

    // Reset sort key sentinel so dead/stale compact_buf slots sort to the end.
    if idx < uniforms.total_particles {
        sort_key_buf[idx] = -F32_MAX;
    }

    wg_scratch[lid.x] = alive_flag;
    workgroupBarrier();

    // Hillis-Steele inclusive scan: O(log N) steps, O(N log N) work.
    var step = 1u;
    loop {
        if step >= WG { break; }
        let val = select(0u, wg_scratch[lid.x - step], lid.x >= step);
        workgroupBarrier();
        wg_scratch[lid.x] += val;
        workgroupBarrier();
        step <<= 1u;
    }
    // wg_scratch[lid.x] = inclusive prefix sum for this thread.

    if lid.x == WG - 1u {
        block_sums_buf[wid.x] = wg_scratch[WG - 1u];
    }

    // Convert inclusive to exclusive: shift right by 1.
    let excl = select(wg_scratch[lid.x - 1u], 0u, lid.x == 0u);
    if idx < uniforms.total_particles {
        prefix_buf[idx] = excl;
    }
}

// ── cs_scan_blocks ────────────────────────────────────────────────────────────
// One workgroup per emitter. Sequentially scans the block_sums within the
// emitter's block range to produce cumulative emitter-relative offsets.
// After this pass:
//   block_sums_buf[b] = how many alive particles are in blocks [block_lo .. b)
//                       within this emitter's range.
//   emitter_alive[eidx] = total alive particles for this emitter.

@compute @workgroup_size(1)
fn cs_scan_blocks(@builtin(workgroup_id) wid: vec3<u32>) {
    let eidx = wid.x;
    if eidx >= uniforms.emitter_count { return; }

    let range    = emitter_range(eidx);
    let block_lo = range.x / WG;
    let block_hi = select(block_lo, (range.x + range.y + WG - 1u) / WG, range.y > 0u);

    var cumsum = 0u;
    for (var b = block_lo; b < block_hi; b++) {
        let block_total   = block_sums_buf[b];
        block_sums_buf[b] = cumsum;   // replace total with emitter-relative offset
        cumsum            += block_total;
    }

    emitter_alive[eidx] = cumsum;
}

// ── cs_scatter ────────────────────────────────────────────────────────────────
// For each alive particle, computes its position in compact_buf using the
// block cumulative offset + local exclusive prefix (no global atomics).
// Also writes the particle's negated view-space z to sort_key_buf at that
// position, so bitonic sort (descending) produces back-to-front order.

@compute @workgroup_size(256)
fn cs_scatter(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(workgroup_id)         wid: vec3<u32>,
) {
    let idx = gid.x;
    if idx >= uniforms.total_particles { return; }

    let p = particles[idx];
    if p.pos_and_alive.w < 0.5 { return; }

    let e = owner_of(idx, p);
    if e >= uniforms.emitter_count { return; }
    // Position within this emitter's compact sub-range:
    //   block_sums_buf[wid.x] = alive count in emitter blocks before this one.
    //   prefix_buf[idx]       = alive count before idx in this block.
    let pos_in_emitter = block_sums_buf[wid.x] + prefix_buf[idx];
    let compact_pos    = emitter_range(e).x + pos_in_emitter;

    compact_buf[compact_pos] = idx;

    // Negated view-space z: positive = far from the camera.
    // Bitonic descending sort puts max (furthest) at position 0.
    let view_pos = cameras[0].view * vec4<f32>(p.pos_and_alive.xyz, 1.0);
    sort_key_buf[compact_pos] = -view_pos.z;
}

// ── cs_build_multi ────────────────────────────────────────────────────────────

@compute @workgroup_size(1)
fn cs_build_multi(@builtin(workgroup_id) wid: vec3<u32>) {
    let eidx = wid.x;
    if eidx >= uniforms.emitter_count { return; }
    let alive = emitter_alive[eidx];
    draw_args_staging[eidx].vertex_count   = 6u;
    draw_args_staging[eidx].instance_count = alive;
    draw_args_staging[eidx].first_vertex   = 0u;
    draw_args_staging[eidx].first_instance = emitter_range(eidx).x;
}

// ── cs_sort_local ─────────────────────────────────────────────────────────────
// Bitonic sort within each 256-element block (all stages k=2..256 in shared
// memory). After this pass, every 256-element sub-range is independently sorted
// descending (max depth at position 0 within the block).
// sort_params: sort_lo = particle_offset, sort_n = particle_count (from uniforms).

@compute @workgroup_size(256)
fn cs_sort_local(
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(workgroup_id)        wid: vec3<u32>,
) {
    let lo     = uniforms.sort_lo;
    let n      = uniforms.sort_n;
    let base_t = wid.x * WG;          // emitter-relative index of this block's first element
    let t      = base_t + lid.x;      // emitter-relative index for this thread
    let gi     = lo + t;              // global compact_buf / sort_key_buf index

    if t < n {
        sh_keys[lid.x] = sort_key_buf[gi];
        sh_vals[lid.x] = compact_buf[gi];
    } else {
        sh_keys[lid.x] = -F32_MAX;
        sh_vals[lid.x] = 0u;
    }
    workgroupBarrier();

    // All stages for k = 2 .. 256 using shared memory.
    // Descending sort: position 0 = maximum.
    // Direction at (k, t): if (t & k) == 0 → descending sub-sequence.
    //   → swap if sh_keys[lid] < sh_keys[buddy]
    // Direction at (k, t): if (t & k) != 0 → ascending sub-sequence.
    //   → swap if sh_keys[lid] > sh_keys[buddy]
    var k = 2u;
    loop {
        if k > WG { break; }
        var j = k >> 1u;
        loop {
            if j == 0u { break; }
            let buddy_lid = lid.x ^ j;
            if lid.x < buddy_lid {
                let desc   = (t & k) == 0u;
                let a = sh_keys[lid.x];
                let b_key = sh_keys[buddy_lid];
                let swap   = select(a > b_key, a < b_key, desc);
                if swap {
                    sh_keys[lid.x]   = b_key;
                    sh_keys[buddy_lid] = a;
                    let va = sh_vals[lid.x];
                    sh_vals[lid.x]   = sh_vals[buddy_lid];
                    sh_vals[buddy_lid] = va;
                }
            }
            workgroupBarrier();
            j >>= 1u;
        }
        k <<= 1u;
    }

    if t < n {
        sort_key_buf[gi] = sh_keys[lid.x];
        compact_buf[gi]  = sh_vals[lid.x];
    }
}

// ── cs_sort_global ────────────────────────────────────────────────────────────
// One compare-swap step for stages where j >= 256 (global memory reads).
// sort_k and sort_j are written into uniforms before each dispatch.
// sort_lo = particle_offset, sort_n = particle_count.
// Dispatch: ceil(sort_n / WG) workgroups, each thread handles one element.

@compute @workgroup_size(256)
fn cs_sort_global(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;               // emitter-relative index
    let k = uniforms.sort_k;
    let j = uniforms.sort_j;
    let lo = uniforms.sort_lo;
    let n  = uniforms.sort_n;

    if i >= n { return; }
    let buddy = i ^ j;
    if buddy >= n || buddy <= i { return; }

    let ti = lo + i;
    let tb = lo + buddy;

    let ka = sort_key_buf[ti];
    let kb = sort_key_buf[tb];

    // Descending sort: (i & k)==0 → descending sub-seq → swap if a < b.
    let desc = (i & k) == 0u;
    let swap = select(ka > kb, ka < kb, desc);
    if swap {
        sort_key_buf[ti] = kb; sort_key_buf[tb] = ka;
        let va = compact_buf[ti];
        compact_buf[ti] = compact_buf[tb];
        compact_buf[tb] = va;
    }
}

// ── cs_sort_local_merge ───────────────────────────────────────────────────────
// For global k-stages: handles the tail steps (j = 128 down to 1) in shared
// memory. Called once per emitter per k-stage after all global (j>=256) steps.
// sort_k is the current stage; sort_lo and sort_n identify the emitter range.
// Within a 256-element block during a global k-stage (k >= 512), all threads
// in the block have the same (t & k) because k >= 256, so the direction is
// uniform across the entire workgroup.

@compute @workgroup_size(256)
fn cs_sort_local_merge(
    @builtin(local_invocation_id) lid: vec3<u32>,
    @builtin(workgroup_id)        wid: vec3<u32>,
) {
    let k      = uniforms.sort_k;
    let lo     = uniforms.sort_lo;
    let n      = uniforms.sort_n;
    let base_t = wid.x * WG;
    let t      = base_t + lid.x;
    let gi     = lo + t;

    if t < n {
        sh_keys[lid.x] = sort_key_buf[gi];
        sh_vals[lid.x] = compact_buf[gi];
    } else {
        sh_keys[lid.x] = -F32_MAX;
        sh_vals[lid.x] = 0u;
    }
    workgroupBarrier();

    // All threads in this block share the same direction because k >= 512 > 256.
    let desc = (base_t & k) == 0u;

    var j = WG >> 1u;  // start at j=128
    loop {
        if j == 0u { break; }
        let buddy_lid = lid.x ^ j;
        if lid.x < buddy_lid {
            let a     = sh_keys[lid.x];
            let b_key = sh_keys[buddy_lid];
            let swap  = select(a > b_key, a < b_key, desc);
            if swap {
                sh_keys[lid.x]     = b_key;
                sh_keys[buddy_lid] = a;
                let va = sh_vals[lid.x];
                sh_vals[lid.x]     = sh_vals[buddy_lid];
                sh_vals[buddy_lid] = va;
            }
        }
        workgroupBarrier();
        j >>= 1u;
    }

    if t < n {
        sort_key_buf[gi] = sh_keys[lid.x];
        compact_buf[gi]  = sh_vals[lid.x];
    }
}
