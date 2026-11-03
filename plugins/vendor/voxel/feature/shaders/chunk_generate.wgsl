// Chunk octree (docs/chunk-octree.md): generation.
//
// A tile is the 64x64 columns of one level under a stack of chunks; a chunk
// is 64^3 cells of one level, stored as a 64-tree: a root over 4^3 mid
// nodes of 16^3 cells, mid nodes over 4^3 leaves of 4^3 cells, leaves a
// 64-bit occupancy mask. Bit `x + 4 y + 16 z` of a node or leaf mask is its
// child or cell (x, y, z). Uniform children (all solid) are flagged in a
// second mask and store nothing.
//
// 1. `tile_columns`: each column's height, surface word and volume extent,
//    and the footprint's range of layers that may hold a surface, into its
//    slot of the footprint pool (only for footprints not already there);
//    its far forms' canopies over them; `tile_features`: its features'
//    placements there, its range raised over them.
// 2. `chunk_columns`: each column's 64 cells in a chunk as a 64-bit mask
//    (bit z: layer `64 cz + z`), with volume terms, features and brushes,
//    and the material of its highest solid cell.
// 3. `chunk_cells` (chunks with per-voxel materials): each column's exposed
//    cells' materials, where their leaves' pairs would draw them wrong;
//    `chunk_build`: the chunk's 64-tree from its column masks, its leaves
//    in pages of 64 (`LEAF_PAGES` a chunk at most) taken off the free
//    pages' stack.
// 4. `evict`: evicted chunks' pages (and their detail pages) back on their
//    free stacks.

const TILE_COLUMNS: u32 = 4096u;
// Height of a tile column past the face's edge.
const OUTSIDE: i32 = -2147483647 - 1;

// A footprint of the batch: the ones whose columns it computes come first
// (`tile_ranges`' count), then those already in the pool.
struct TileJob {
    face: u32,
    level: u32,
    cx: i32,
    cy: i32,
    // Its place in the footprint pool (`tile_columns`, `tile_caves`): kept
    // across frames, so a footprint's columns are computed once while
    // resident, whichever of its nodes are generated when.
    slot: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

struct ChunkJob {
    tile: u32,  // index into tile_jobs
    cz: i32,
    slot: u32,  // chunk slot of the result
    brushes: u32, // first brush ref; count in `brush_count`
    brush_count: u32,
    // Refs of the large log (they come first), then the recent log's.
    large: u32,
    // Its baked bricks: 8 per brick column (`baked_refs`), or NONE.
    baked: u32,
    // Its slot of `cell_materials` when edits set materials in it (its
    // leaves get per-voxel materials), or NONE.
    detail: u32,
}

struct FaceBrush {
    flags: u32,
    radius_half: u32,
    k_lo: i32,
    k_hi: i32,
    center: vec4<i32>,
    ball: vec4<i32>,
}

struct ChunkMeta {
    root_occ: vec2<u32>,
    root_full: vec2<u32>,
    // Lowest and one past the highest occupied layer of the chunk (cells).
    z: vec2<i32>,
    // Leaves stored for the chunk (0: none) and their block (NONE: none).
    leaves: u32,
    pages: u32,
    // A plane over every solid cell, z = top.z + top.x x + top.y y (cells):
    // a chunk on a slope is far lower than its occupied layers' box.
    top: vec4<f32>,
}

const NONE: u32 = 0xffffffffu;
// Leaves are stored in pages of 64, a chunk's up to `LEAF_PAGES` of them
// in `leaf_pages`: every page serves any chunk, so freed storage is never
// stranded in a size it no longer fits.
const LEAF_PAGES: u32 = 64u;

@group(0) @binding(2) var<storage, read> tile_jobs: array<TileJob>;
@group(0) @binding(3) var<storage, read_write> tile_columns: array<vec4<i32>>;
// Each computed footprint's measured layers, by quarter (32x32 columns:
// x then y, each a child footprint's), four words a quarter: the volume's
// lowest and highest, the surface's lowest and highest (level cells); the
// batch's computed count last.
@group(0) @binding(4) var<storage, read_write> tile_ranges: array<atomic<i32>>;
const RANGE_WORDS: u32 = 16u;
@group(0) @binding(5) var<storage, read> chunk_jobs: array<ChunkJob>;
@group(0) @binding(6) var<storage, read_write> column_masks: array<vec2<u32>>;
@group(0) @binding(7) var<storage, read> brushes: array<FaceBrush>;
@group(0) @binding(8) var<storage, read> brush_refs: array<u32>;
@group(0) @binding(9) var<storage, read_write> chunk_meta: array<ChunkMeta>;
@group(0) @binding(10) var<storage, read_write> mids: array<vec4<u32>>;
@group(0) @binding(11) var<storage, read_write> mid_leaves: array<u32>;
@group(0) @binding(12) var<storage, read_write> leaves: array<vec2<u32>>;
// Free pages (a count of `free_pages`), an overflow flag.
@group(0) @binding(13) var<storage, read_write> leaf_alloc: array<atomic<i32>>;
// The free pages' indices, a stack.
@group(0) @binding(20) var<storage, read_write> free_pages: array<u32>;
// Each slot's pages (`LEAF_PAGES` a slot).
@group(0) @binding(24) var<storage, read_write> leaf_pages: array<u32>;
// Per chunk job: slot, occupied layers [lo, hi), flags (1 built, 2 empty,
// 4 solid throughout, 8 out of leaf storage).
@group(0) @binding(19) var<storage, read_write> job_results: array<vec4<i32>>;
// Slots evicted this frame.
@group(0) @binding(21) var<storage, read> evictions: array<u32>;
// Baked edits (`edit_store`) under the chunk jobs: per job with any, a
// brick per brick column (8x8 of a chunk) and layer (8), NONE where none;
// a brick's 512 cell edits two a word.
@group(0) @binding(22) var<storage, read> baked_refs: array<u32>;
@group(0) @binding(23) var<storage, read> baked_cells: array<u32>;
// Per-voxel materials where edits are (`docs/chunk-octree.md`, "Per-voxel
// materials"). A chunk job under edits writes each solid cell's edited
// material (0: none set) into its slot of `cell_materials`: CELL_WORDS a
// column, cell z in byte z & 3 of word z >> 2, then the column's natural
// top, then its leaves with cells left (bit lz). `chunk_cells_main` gives
// each exposed cell without one its natural material (`Planet::material`)
// and clears the cells their leaf's pair draws as they are;
// `chunk_build_main` gives every leaf page holding a leaf with cells left
// a detail page (`detail_pages`, by leaf page; NONE: none) of
// `DETAIL_WORDS` words: 16 a leaf, by the leaf's place in its page, word
// x + 4 y its column's four cells (byte z). Other leaves read zero there.
// Pages come off the stack `detail_free` of pages given back, else past
// the pages ever taken, up to the capacity (`detail_alloc`: the stack's
// count, the overflow flag, the pages ever taken, the capacity), and go
// back on the stack when the chunk is evicted. The world grows the pool
// (and raises the capacity) when it overflows.
@group(0) @binding(28) var<storage, read_write> cell_materials: array<u32>;
@group(0) @binding(29) var<storage, read_write> detail_pages: array<u32>;
@group(0) @binding(30) var<storage, read_write> detail_cells: array<u32>;
@group(0) @binding(31) var<storage, read_write> detail_alloc: array<atomic<i32>>;
@group(0) @binding(32) var<storage, read_write> detail_free: array<u32>;
// Per slot: 1 when any of its leaf pages has a detail page (drawing skips
// the lookup for the others).
@group(0) @binding(33) var<storage, read_write> slot_details: array<u32>;
// Each footprint's material slopes (`Planet::material_slope`), kept with
// its columns in the pool: for levels up to MATERIAL_SLOPE_LEVEL - 1, the
// block slopes of the level-4 cells its columns interpolate between, rows
// of `slope_blocks(level)` from level-4 cell `slope_origin`.
@group(0) @binding(34) var<storage, read_write> tile_slopes: array<i32>;
const SLOPE_STRIDE: u32 = 1156u;
const MATERIAL_SLOPE_LEVEL: u32 = 4u;

// Level-4 block slopes a footprint of `level` keeps along a side.
fn slope_blocks(level: u32) -> i32 {
    return (4 << level) + 2;
}

// The first of them: level-4 cell (4 c << level) - 1 along each axis.
fn slope_origin(job: TileJob) -> vec2<i32> {
    return vec2<i32>((4 * job.cx) << job.level, (4 * job.cy) << job.level) - 1;
}
const DETAIL_WORDS: u32 = 1024u;
const CELL_WORDS: u32 = 18u;

// A detail page off the free stack, else a page never taken below the
// capacity (NONE: none free; the overflow flag is set).
fn take_detail() -> u32 {
    var free = atomicLoad(&detail_alloc[0]);
    for (var tries = 0u; tries < 4096u; tries++) {
        if free < 1 { break; }
        let r = atomicCompareExchangeWeak(&detail_alloc[0], free, free - 1);
        if r.exchanged { return detail_free[u32(free - 1)]; }
        free = r.old_value;
    }
    let capacity = atomicLoad(&detail_alloc[3]);
    var fresh = atomicLoad(&detail_alloc[2]);
    for (var tries = 0u; tries < 4096u; tries++) {
        if fresh >= capacity { break; }
        let r = atomicCompareExchangeWeak(&detail_alloc[2], fresh, fresh + 1);
        if r.exchanged { return u32(fresh); }
        fresh = r.old_value;
    }
    atomicStore(&detail_alloc[1], 1);
    return NONE;
}

// `n` pages off the free stack: the first's place in it (NONE: fewer are
// free; the overflow flag is set).
fn take_pages(n: u32) -> u32 {
    var free = atomicLoad(&leaf_alloc[0]);
    // Each failed exchange is another chunk's take: at most a batch of them.
    for (var tries = 0u; tries < 4096u; tries++) {
        if free < i32(n) { break; }
        let r = atomicCompareExchangeWeak(&leaf_alloc[0], free, free - i32(n));
        if r.exchanged { return u32(free - i32(n)); }
        free = r.old_value;
    }
    atomicStore(&leaf_alloc[1], 1);
    return NONE;
}

// Evicted slots' pages back on the free stack (a dispatch of its own: no
// page is taken meanwhile).
@compute @workgroup_size(64)
fn evict(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= arrayLength(&evictions) { return; }
    let slot = evictions[id.x];
    let n = chunk_meta[slot].pages;
    if n == 0u { return; }
    let at = u32(atomicAdd(&leaf_alloc[0], i32(n)));
    for (var p = 0u; p < n; p++) {
        let page = leaf_pages[slot * LEAF_PAGES + p];
        free_pages[at + p] = page;
        let detail = detail_pages[page];
        if detail != NONE {
            detail_free[u32(atomicAdd(&detail_alloc[0], 1))] = detail;
            detail_pages[page] = NONE;
        }
    }
    chunk_meta[slot].pages = 0u;
    chunk_meta[slot].leaves = 0u;
}
// Scratch per chunk job: each column's materials (`chunk_columns_main`):
// its top cell's (bit 7: a single-voxel speck), the soil's and the rock's
// under it (bytes 0, 1, 2), and
// the lowest of the terrain's tops around it (its own and its neighbours')
// relative to the chunk, + 128 (byte 3): cells above it are a natural
// riser's, surface.
@group(0) @binding(14) var<storage, read_write> column_materials: array<u32>;
// Two material bytes per stored leaf (two leaves a word) and per mid node:
// the surface's, shown by its cells with air above, and the subsurface's
// (soil or rock by the leaf's depth), shown by the others.
@group(0) @binding(15) var<storage, read_write> leaf_materials: array<atomic<u32>>;
@group(0) @binding(17) var<storage, read_write> mid_materials: array<u32>;
// Each stored leaf's ground normal (two leaves a word, as
// `leaf_materials`; `encode_slope` x | y << 8): the heightfield's slope
// over the leaf's columns from their exact heights, where the leaf holds
// their natural tops (0: none: an edit or an overhang made them).
@group(0) @binding(27) var<storage, read_write> leaf_normals: array<atomic<u32>>;

// A slope (cells per cell) in a byte, by its angle: 2 to 254, 0 for none.
fn encode_slope(g: f32) -> u32 {
    return u32(i32(clamp(round(atan(g) * (252.0 / 3.14159265)), -126.0, 126.0)) + 128);
}

var<workgroup> w_range: array<atomic<i32>, 4>;

// The level-4 column tops (base layers) of each computed footprint's
// lattice (`tile_slope_heights_main`): SLOPE_LATTICE a footprint of the
// batch.
@group(0) @binding(35) var<storage, read_write> slope_lattice: array<i32>;
const SLOPE_LATTICE: u32 = 1444u;

// A footprint's lattice: `side` level-4 columns a side from `first`, its
// macro octaves from `nodes`^2 nodes from node-aligned `origin` (3x3 at
// levels 0 and 1, up to 5x5 at level 3).
struct SlopeLattice {
    first: vec2<i32>,
    origin: vec2<i32>,
    side: i32,
    nodes: u32,
}

fn slope_lattice_of(job: TileJob) -> SlopeLattice {
    let side = slope_blocks(job.level) + 4;
    let first = slope_origin(job) - 2;
    let origin = first & vec2<i32>(~(NODE_COLUMNS - 1));
    let reach = (((first + side - 1) & vec2<i32>(~(NODE_COLUMNS - 1))) - origin) / NODE_COLUMNS;
    return SlopeLattice(first, origin, side, u32(max(reach.x, reach.y)) + 2u);
}

// The material slopes of each computed footprint below
// MATERIAL_SLOPE_LEVEL (`Planet::material_slope`), in three dispatches:
// the lattice's nodes (tiles), its level-4 heights two cells each way of
// each block (23 x tiles, 64 a workgroup), then the block slopes (19 x
// tiles).
@compute @workgroup_size(64)
fn tile_slope_nodes_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let job = tile_jobs[wg.x];
    if job.level >= MATERIAL_SLOPE_LEVEL { return; }
    let lattice = slope_lattice_of(job);
    generation_grid_nodes(LATTICE_AT + wg.x * TILE_NODES * TILE_NODES, TILE_NODES, job.face, lattice.origin, lattice.nodes, MATERIAL_SLOPE_LEVEL, li);
}

@compute @workgroup_size(64)
fn tile_slope_heights_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let job = tile_jobs[wg.y];
    if job.level >= MATERIAL_SLOPE_LEVEL { return; }
    let lattice = slope_lattice_of(job);
    let n = i32(wg.x * 64u + li);
    if n >= lattice.side * lattice.side { return; }
    let c = lattice.first + vec2<i32>(n % lattice.side, n / lattice.side);
    slope_lattice[wg.y * SLOPE_LATTICE + u32(n)] = div_floor(generation_grid_column(LATTICE_AT + wg.y * TILE_NODES * TILE_NODES, TILE_NODES, lattice.origin, job.face, c.x, c.y, MATERIAL_SLOPE_LEVEL, false).x, world.grid.y);
}

@compute @workgroup_size(64)
fn tile_slopes_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let job = tile_jobs[wg.y];
    if job.level >= MATERIAL_SLOPE_LEVEL { return; }
    let blocks = slope_blocks(job.level);
    let n = i32(wg.x * 64u + li);
    if n >= blocks * blocks { return; }
    let side = blocks + 4;
    let at = wg.y * SLOPE_LATTICE;
    let b = vec2<i32>(n % blocks, n / blocks) + 2;
    let di = abs(slope_lattice[at + u32(b.y * side + b.x + 2)] - slope_lattice[at + u32(b.y * side + b.x - 2)]);
    let dj = abs(slope_lattice[at + u32((b.y + 2) * side + b.x)] - slope_lattice[at + u32((b.y - 2) * side + b.x)]);
    tile_slopes[job.slot * SLOPE_STRIDE + u32(n)] = (gradient_length(di, dj) << (16u - MATERIAL_SLOPE_LEVEL)) / 32768;
}

// The length of a gradient's differences `di`, `dj` (base layers, each
// held to 46340 so their squares sum in a u32), rounded down exactly, as
// `Planet::material_slope` takes it. (The larger difference alone measured
// slopes across the axes short, by up to 29% on a diagonal: a material
// threshold on it traced squares on the face's grid, the Moon's rock specks
// stopping at the straight edges of a crater wall's level ground.)
fn gradient_length(di: i32, dj: i32) -> i32 {
    let a = u32(min(abs(di), 46340));
    let b = u32(min(abs(dj), 46340));
    let x = a * a + b * b;
    // f32's root is within a step of the exact one below 2^32.
    var r = min(u32(sqrt(f32(x))), 65535u);
    for (var n = 0u; n < 2u && r * r > x; n++) { r -= 1u; }
    for (var n = 0u; n < 2u && r < 65535u && (r + 1u) * (r + 1u) <= x; n++) { r += 1u; }
    return i32(r);
}

// The material slope of level column (i, j) of footprint `job` (below
// MATERIAL_SLOPE_LEVEL): `Planet::material_slope` of the base column at
// its centre, its blocks' slopes interpolated in 32nds.
fn material_slope(job: TileJob, i: i32, j: i32) -> i32 {
    let base = (vec2<i32>(i, j) << vec2<u32>(job.level)) + ((1 << job.level) >> 1u);
    let r = base * 2 + 1 - 16;
    let a = r >> vec2<u32>(5u);
    let w = r - (a << vec2<u32>(5u));
    let blocks = slope_blocks(job.level);
    let b = a - slope_origin(job);
    let at = job.slot * SLOPE_STRIDE + u32(b.y * blocks + b.x);
    let s00 = tile_slopes[at];
    let s10 = tile_slopes[at + 1u];
    let s01 = tile_slopes[at + u32(blocks)];
    let s11 = tile_slopes[at + u32(blocks) + 1u];
    return ((32 - w.x) * (32 - w.y) * s00 + w.x * (32 - w.y) * s10 + (32 - w.x) * w.y * s01 + w.x * w.y * s11) >> 10u;
}

// A two-rate field's nodes of each tile (`generation_tile_nodes`), before
// `tile_columns_main`: dispatch (tiles).
@compute @workgroup_size(64)
fn tile_nodes_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let job = tile_jobs[wg.x];
    generation_tile_nodes(wg.x, job.face, job.cx, job.cy, job.level, li);
}

// One workgroup per 8x8 columns of a tile: dispatch (64, tiles).
@compute @workgroup_size(64)
fn tile_columns_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let job = tile_jobs[wg.y];
    let x = (wg.x & 7u) * 8u + (li & 7u);
    let y = (wg.x >> 3u) * 8u + (li >> 3u);
    let i = job.cx * 64 + i32(x);
    let j = job.cy * 64 + i32(y);
    if li == 0u {
        atomicStore(&w_range[0], 0x7fffffff);
        atomicStore(&w_range[1], -0x7fffffff);
        atomicStore(&w_range[2], 0x7fffffff);
        atomicStore(&w_range[3], -0x7fffffff);
    }
    workgroupBarrier();
    // Tiles at a face's edge reach past it: those columns are empty.
    let cells = world.grid.z >> job.level;
    if i < cells && j < cells {
        // Coarse levels keep the mean of the ridges they cannot resolve
        // (the display field), so they sit where finer levels' terrain sits.
        let field = generation_tile_column(wg.y, job.face, i, j, job.level, job.level >= 1u);
        let p = domain_point(job.face, i, j, job.level);
        let top = div_floor(field.x, world.grid.y) >> job.level;
        // The far form over the column (`far_spawn`), once a footprint: its
        // spawn kept over the surface word, the volume's range raised over
        // its canopy.
        var far = 0u;
        if features[0] > 0u {
            far = far_spawn(job, i, j, p, top, field.x, u32(field.y));
            if far != 0u {
                let canopy = column_far(vec4<i32>(field.x, i32(far << 8u), 0, 0));
                atomicMax(&w_range[1], ((canopy.z - 1) >> job.level) + 1);
            }
        }
        // The volume's extent comes later (`tile_volumes_main`).
        tile_columns[job.slot * TILE_COLUMNS + y * 64u + x] = vec4<i32>(field.x, field.y | i32(far << 8u), 0, 0);
        // The cell under the top holds the surface. Ranges: the volume's
        // (`tile_volumes_main` widens it), the surface's.
        atomicMin(&w_range[0], top - 1);
        atomicMax(&w_range[1], top);
        atomicMin(&w_range[2], top - 1);
        atomicMax(&w_range[3], top);
    } else {
        tile_columns[job.slot * TILE_COLUMNS + y * 64u + x] = vec4<i32>(OUTSIDE, 0, 0, 0);
    }
    workgroupBarrier();
    // The quarter its 8x8 columns lie in.
    let at = wg.y * RANGE_WORDS + (((wg.x & 7u) >> 2u) | ((wg.x >> 5u) << 1u)) * 4u;
    if li == 0u {
        atomicMin(&tile_ranges[at], atomicLoad(&w_range[0]));
        atomicMax(&tile_ranges[at + 1u], atomicLoad(&w_range[1]));
        atomicMin(&tile_ranges[at + 2u], atomicLoad(&w_range[2]));
        atomicMax(&tile_ranges[at + 3u], atomicLoad(&w_range[3]));
    }
}

fn set_bits(m: vec2<u32>, lo: i32, hi: i32, on: bool) -> vec2<u32> {
    // Bits [lo, hi) of a 64-bit mask set or cleared.
    let a = clamp(lo, 0, 64);
    let b = clamp(hi, 0, 64);
    if a >= b { return m; }
    var r = m;
    let lo_a = u32(min(a, 32));
    let lo_b = u32(min(b, 32));
    if lo_b > lo_a {
        let bits = select((1u << (lo_b - lo_a)) - 1u, 0xffffffffu, lo_b - lo_a == 32u) << lo_a;
        r.x = select(r.x & ~bits, r.x | bits, on);
    }
    let hi_a = u32(max(a, 32) - 32);
    let hi_b = u32(max(b, 32) - 32);
    if hi_b > hi_a {
        let bits = select((1u << (hi_b - hi_a)) - 1u, 0xffffffffu, hi_b - hi_a == 32u) << hi_a;
        r.y = select(r.y & ~bits, r.y | bits, on);
    }
    return r;
}

fn bit_at(m: vec2<u32>, b: u32) -> bool {
    return ((select(m.x, m.y, b >= 32u) >> (b & 31u)) & 1u) != 0u;
}

// Where brush `b` crosses the column over domain point `p` (half-cell
// centre `ch`): its first and last level cells, how far either may be off
// (cells; 0: exact), and whether it reaches the column at all. A box's
// cells are exact. A ball in volume space holds one interval of the column
// (its volume points are linear in the layer, `volume_point_half`): solved
// in f32 around the column's exact point at the ball's height, along the
// exact slope, with the error of the f32 integers and products (relative
// 2^-22) and of the volume points' rounding; near tangency the root moves
// by the square root of the discriminant's error. Cells within the margin
// of either end are tested exactly (`brush_holds`).
fn brush_cells(b: FaceBrush, p: vec3<i32>, ch: vec2<i32>, level: u32) -> vec4<i32> {
    let none = vec4<i32>(0, -1, 0, 0);
    if b.radius_half < (1u << level) { return none; }
    let shift = level + 1u;
    let half = 1 << level;
    if ((b.flags >> 6u) & 3u) == 1u {
        // A box: the columns inside its footprint, between its faces (cell
        // centres `(k << shift) + half` from k_lo to k_hi).
        if any(vec2<u32>(abs(ch - b.center.xy)) > vec2<u32>(b.radius_half)) { return none; }
        return vec4<i32>(-((half - b.k_lo) >> shift), (b.k_hi - half) >> shift, 0, 1);
    }
    let h0 = (b.k_lo >> 1u) + (b.k_hi >> 1u);
    let q0 = volume_point_half(p, h0);
    var v = vec3<f32>(0.0, f32(world.scale.w) / 65536.0, 0.0);
    if !is_plane() { v = vec3<f32>(p) * (f32(world.scale.y) * exp2(-f32(world.scale.z) - 30.0)); }
    let a = dot(v, v);
    if a <= 0.0 { return none; }
    let d = vec3<f32>(q0 - b.ball.xyz);
    let r = f32(abs(b.ball.w));
    let dl = length(d);
    let bb = dot(d, v);
    let c = (dl - r) * (dl + r);
    let disc = bb * bb - a * c;
    let e = (dl + r) * 4.8e-7 + 4.0;
    let dd = a * 2.0 * (dl + r) * e;
    if disc < -dd { return none; }
    let sq = sqrt(max(disc, 0.0));
    let root_err = select(dd / (2.0 * sq), sqrt(dd), sq * sq < dd);
    let t_err = (root_err + e * sqrt(a)) / a;
    // Cells relative to the cell holding h0, so f32 never holds the
    // planet-scale height itself.
    let k0 = h0 >> shift;
    let rem = f32(h0 - (k0 << shift) - half);
    let cell = f32(1 << shift);
    let lo = k0 + i32(floor((rem + (-bb - sq) / a) / cell + 0.5));
    let hi = k0 + i32(floor((rem + (-bb + sq) / a) / cell + 0.5));
    return vec4<i32>(lo, hi, i32(ceil(t_err / cell)) + 2, 1);
}

// Exact containment of level cell `k` of the column (`FaceBrush::contains`:
// exact 64-bit squares), given its `brush_cells`.
fn brush_holds(b: FaceBrush, p: vec3<i32>, cells: vec4<i32>, level: u32, k: i32) -> bool {
    if cells.w == 0 || k < cells.x - cells.z || k > cells.y + cells.z { return false; }
    if k > cells.x + cells.z && k < cells.y - cells.z { return true; }
    if cells.z == 0 { return true; }
    let q = volume_point_half(p, center_half(k, level));
    let rr = u32(abs(b.ball.w));
    let dq = vec3<u32>(abs(q - b.ball.xyz));
    if any(dq > vec3<u32>(rr)) { return false; }
    let sum = add_wide(add_wide(mul_wide(dq.x, dq.x), mul_wide(dq.y, dq.y)), mul_wide(dq.z, dq.z));
    let r2 = mul_wide(rr, rr);
    return sum.y < r2.y || (sum.y == r2.y && sum.x <= r2.x);
}

// Brush refs [first, end) of `job` applied in order to a column's cells
// from layer `k0`.
fn apply_brushes(mask: vec2<u32>, job: ChunkJob, first: u32, end: u32, p: vec3<i32>, ch: vec2<i32>, level: u32, k0: i32) -> vec2<u32> {
    var m = mask;
    for (var e = first; e < end; e++) {
        let b = brushes[brush_refs[job.brushes + e]];
        // Ops: 0 remove, 1 add; paint changes no cell (`edits::BrushOp`).
        let op = (b.flags >> 4u) & 3u;
        if op >= 2u { continue; }
        let cells = brush_cells(b, p, ch, level);
        if cells.w == 0 || cells.x - cells.z > cells.y + cells.z { continue; }
        // Cells surely inside at once, those near either end exactly.
        let add = op == 1u;
        if cells.z == 0 {
            m = set_bits(m, cells.x - k0, cells.y - k0 + 1, add);
        } else {
            m = set_bits(m, cells.x + cells.z + 1 - k0, cells.y - cells.z - k0, add);
            let edges = array<vec2<i32>, 2>(vec2<i32>(cells.x - cells.z, cells.x + cells.z), vec2<i32>(cells.y - cells.z, cells.y + cells.z));
            for (var n = 0u; n < 2u; n++) {
                for (var k = max(edges[n].x, k0); k <= min(edges[n].y, k0 + 63); k++) {
                    if brush_holds(b, p, cells, level, k) { m = set_bits(m, k - k0, k - k0 + 1, add); }
                }
            }
        }
    }
    return m;
}

// The baked edit of cell `z` of the job's column `li` in brick column
// `column` (`edit_store::CellEdit`: state in bits 0..2, 1 air, 2 solid,
// 3 paint; material in 8..16).
// A brick's table entry: NONE (none), UNIFORM_BRICK | the edit of all its
// cells, or its cells' first word / 256 in `baked_cells`.
const UNIFORM_BRICK: u32 = 0x40000000u;

fn baked_edit(job: ChunkJob, column: u32, li: u32, z: u32) -> u32 {
    let b = baked_refs[job.baked + column * 8u + (z >> 3u)];
    if b == NONE { return 0u; }
    if (b & UNIFORM_BRICK) != 0u { return b & 0xffffu; }
    let n = (li & 7u) + 8u * ((li >> 3u) + 8u * (z & 7u));
    return (baked_cells[b * 256u + (n >> 1u)] >> ((n & 1u) * 16u)) & 0xffffu;
}

fn apply_baked(mask: vec2<u32>, job: ChunkJob, column: u32, li: u32) -> vec2<u32> {
    if job.baked == NONE { return mask; }
    var m = mask;
    for (var bz = 0u; bz < 8u; bz++) {
        if baked_refs[job.baked + column * 8u + bz] == NONE { continue; }
        for (var z = bz * 8u; z < bz * 8u + 8u; z++) {
            let state = baked_edit(job, column, li, z) & 3u;
            if state == 1u || state == 2u { m = set_bits(m, i32(z), i32(z) + 1, state == 2u); }
        }
    }
    return m;
}

// Solid cell `k`'s material after the edits over the terrain's
// `material`: the last brush that added or painted it, or its baked edit
// (material 0: the terrain's).
fn edited_material(material: u32, job: ChunkJob, column: u32, li: u32, p: vec3<i32>, ch: vec2<i32>, level: u32, k: i32, k0: i32) -> u32 {
    var m = material;
    for (var e = 0u; e <= job.brush_count; e++) {
        if e == job.large && job.baked != NONE {
            let edit = baked_edit(job, column, li, u32(k - k0));
            if (edit & 3u) >= 2u && (edit >> 8u) != 0u { m = edit >> 8u; }
        }
        if e == job.brush_count { break; }
        let b = brushes[brush_refs[job.brushes + e]];
        let brush_material = (b.flags >> 8u) & 0xffu;
        if ((b.flags >> 4u) & 3u) == 0u || brush_material == 0u { continue; }
        if brush_holds(b, p, brush_cells(b, p, ch, level), level, k) { m = brush_material; }
    }
    return m;
}

// Whether the chunk is air throughout (`chunk_columns_main`).
var<workgroup> w_air: u32;

// Each footprint's cave primitives (`volumes::Primitive::words`), kept with
// its columns in the pool: a count, then CAVE_SLOT primitives of 8 words,
// horizontal coordinates from the footprint's first base cell
// (`tile_caves_main`, docs/volumes.md).
@group(0) @binding(26) var<storage, read_write> tile_caves: array<i32>;
const CAVE_SLOT: u32 = 48u;
const CAVE_SLOT_WORDS: u32 = 392u;

// The first base cell of a footprint.
fn footprint_origin(job: TileJob) -> vec2<i32> {
    return vec2<i32>(job.cx * 64, job.cy * 64) << vec2<u32>(job.level);
}

// Owner `o` of a footprint's columns (`VolumeLayer::owners`): the regions
// under it, each row with its `-i` neighbour, then the `-j` row; z: 0 past
// the last.
fn cave_owner(job: TileJob, o: u32) -> vec3<i32> {
    let origin = footprint_origin(job);
    let r0 = origin >> vec2<u32>(volume_region_shift());
    let r1 = (origin + (64 << job.level) - 1) >> vec2<u32>(volume_region_shift());
    let n = r1 - r0 + 1;
    let row = u32(n.x + 1);
    if o < row * u32(n.y) { return vec3<i32>(r0.x - 1 + i32(o % row), r0.y + i32(o / row), 1); }
    let rest = o - row * u32(n.y);
    if rest < u32(n.x) { return vec3<i32>(r0.x + i32(rest), r0.y - 1, 1); }
    return vec3<i32>(0);
}

var<workgroup> w_cave_ground: array<vec2<i32>, 48>;
var<workgroup> w_cave_count: atomic<u32>;
var<workgroup> w_cave_tiles: u32;

// Tiles `wg.x`, `wg.x + HALO_GROUPS`, ..: each footprint's primitives at
// its level, from its owners' networks (8 at most where caves are
// resolved). A lane a site reads its reference ground (scratch node
// `wg.x * 64 + li`), then a lane an owner builds its network and keeps the
// primitives over the footprint.
@compute @workgroup_size(64)
fn tile_caves_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    if li == 0u { w_cave_tiles = u32(atomicLoad(&tile_ranges[arrayLength(&tile_ranges) - 1u])); }
    let tiles = workgroupUniformLoad(&w_cave_tiles);
    for (var t = wg.x; t < tiles; t += HALO_GROUPS) {
        let job = tile_jobs[t];
        let slot = job.slot * CAVE_SLOT_WORDS;
        let caves_on = volumes_caves() && i32(job.level) <= cave_deepest_level(true);
        if li == 0u { atomicStore(&w_cave_count, 0u); }
        if li < 48u {
            var g = vec2<i32>(0);
            let owner = cave_owner(job, li / 6u);
            if caves_on && owner.z != 0 {
                let site = volume_site(job.face, owner.x, owner.y, li % 6u);
                if site.used {
                    let reference = u32(volumes.head.z);
                    let c = generation_halo_column(t, job.face, site.x >> reference, site.y >> reference, reference, reference >= 1u, wg.x * 64u + li);
                    g = vec2<i32>(c.x, terrain_cave_gate(u32(c.y) & 0xffu, c.x));
                }
            }
            w_cave_ground[li] = g;
        }
        workgroupBarrier();
        let owner = cave_owner(job, li);
        if caves_on && li < 8u && owner.z != 0 {
            var ground: array<vec2<i32>, 6>;
            for (var s = 0u; s < VOLUME_SITES; s++) { ground[s] = w_cave_ground[li * 6u + s]; }
            let net = volume_network(job.face, owner.x, owner.y, ground, footprint_origin(job));
            let side = 4 * (64 << job.level);
            for (var n = 0u; n < net.count; n++) {
                let p = net.prims[n];
                let h = cave_reach(p).x;
                if !cave_resolved(p, job.level) || max(p.a.x, p.b.x) + h < 0 || min(p.a.x, p.b.x) - h >= side
                    || max(p.a.y, p.b.y) + h < 0 || min(p.a.y, p.b.y) - h >= side { continue; }
                let at = atomicAdd(&w_cave_count, 1u);
                if at < CAVE_SLOT {
                    var w = cave_primitive_words(p);
                    for (var k = 0u; k < 8u; k++) { tile_caves[slot + 8u + at * 8u + k] = w[k]; }
                }
            }
        }
        workgroupBarrier();
        if li == 0u { tile_caves[slot] = i32(min(atomicLoad(&w_cave_count), CAVE_SLOT)); }
        workgroupBarrier();
    }
}

// A brick column's primitives (`cull_caves`).
var<workgroup> w_prims: array<array<i32, 8>, 48>;
var<workgroup> w_prim_count: atomic<u32>;
var<workgroup> w_prim_total: u32;

// The primitives of the footprint in pool slot `slot` over the 8x8
// columns of `level` from local column `first` and reaching Q2 layers
// from `z_lo` up, into `w_prims` (every lane calls it; `enabled` false:
// none). Returns their count.
fn cull_caves(slot: u32, first: vec2<i32>, level: u32, z_lo: i32, li: u32, enabled: bool) -> u32 {
    if li == 0u { atomicStore(&w_prim_count, 0u); }
    workgroupBarrier();
    if enabled {
        let count = u32(tile_caves[slot * CAVE_SLOT_WORDS]);
        if li < count {
            var w: array<i32, 8>;
            for (var k = 0u; k < 8u; k++) { w[k] = tile_caves[slot * CAVE_SLOT_WORDS + 8u + li * 8u + k]; }
            let p = cave_primitive_of(w);
            let lo = (4 * first + 2) << vec2<u32>(level);
            let hi = (4 * (first + 7) + 2) << vec2<u32>(level);
            let h = cave_reach(p).x;
            if max(p.a.x, p.b.x) + h >= lo.x && min(p.a.x, p.b.x) - h <= hi.x
                && max(p.a.y, p.b.y) + h >= lo.y && min(p.a.y, p.b.y) - h <= hi.y && cave_z_range(p).y >= z_lo {
                w_prims[atomicAdd(&w_prim_count, 1u)] = w;
            }
        }
    }
    workgroupBarrier();
    if li == 0u { w_prim_total = atomicLoad(&w_prim_count); }
    return workgroupUniformLoad(&w_prim_total);
}

// Whether bit `b` of a 64-bit mask is set.
fn mask_bit(m: vec2<u32>, b: i32) -> bool {
    if b < 32 { return ((m.x >> u32(b)) & 1u) != 0u; }
    return ((m.y >> u32(b - 32)) & 1u) != 0u;
}

// Each column's volume extent (`Planet::volume_column`): the cells below
// and above its top its cliff band and the cave primitives over it may
// change, into `tile_columns.zw`, and the footprint's measured volume
// range. One workgroup per 8x8 columns: dispatch (64, tiles).
@compute @workgroup_size(64)
fn tile_volumes_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let job = tile_jobs[wg.y];
    let x = (wg.x & 7u) * 8u + (li & 7u);
    let y = (wg.x >> 3u) * 8u + (li >> 3u);
    let i = job.cx * 64 + i32(x);
    let j = job.cy * 64 + i32(y);
    if li == 0u {
        atomicStore(&w_range[0], 0x7fffffff);
        atomicStore(&w_range[1], -0x7fffffff);
    }
    let first = vec2<i32>(i32((wg.x & 7u) * 8u), i32((wg.x >> 3u) * 8u));
    let caves = volumes_caves() && i32(job.level) <= cave_deepest_level(true);
    let count = cull_caves(job.slot, first, job.level, -0x7fffffff, li, caves);
    let at = job.slot * TILE_COLUMNS + y * 64u + x;
    let column = tile_columns[at];
    if column.x != OUTSIDE {
        let level = job.level;
        let top = div_floor(column.x, world.grid.y) >> level;
        var below = 0;
        var above = 0;
        if volumes_cliffs() && level < CLIFF_LEVELS {
            let cliff = volume_cliff(domain_point(job.face, i, j, level), material_slope(job, i, j), level);
            below = cliff.cells;
            above = cliff.cells;
        }
        let q = (4 * vec2<i32>(i32(x), i32(y)) + 2) << vec2<u32>(level);
        let deepest = cave_max_depth_cells(level);
        for (var c = 0u; c < count; c++) {
            let p = cave_primitive_of(w_prims[c]);
            if cave_covers(p, q.x, q.y) {
                below = max(below, clamp(top - div_floor(cave_z_range(p).x, 4 << level), 0, deepest));
            }
        }
        if below > 0 || above > 0 {
            tile_columns[at] = vec4<i32>(column.xy, below, above);
            atomicMin(&w_range[0], top - 1 - below);
            atomicMax(&w_range[1], top + above);
        }
    }
    workgroupBarrier();
    // The quarter its 8x8 columns lie in (`tile_columns_main`).
    let quarter = wg.y * RANGE_WORDS + (((wg.x & 7u) >> 2u) | ((wg.x >> 5u) << 1u)) * 4u;
    if li == 0u {
        atomicMin(&tile_ranges[quarter], atomicLoad(&w_range[0]));
        atomicMax(&tile_ranges[quarter + 1u], atomicLoad(&w_range[1]));
    }
}

// One thread per column, one workgroup per 8x8 columns (a brick column):
// dispatch (64, chunks).
@compute @workgroup_size(64)
fn chunk_columns_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let job = chunk_jobs[wg.y];
    let tile = tile_jobs[job.tile];
    let face = tile.face;
    let level = tile.level;
    let x = (wg.x & 7u) * 8u + (li & 7u);
    let y = (wg.x >> 3u) * 8u + (li >> 3u);
    let i = tile.cx * 64 + i32(x);
    let j = tile.cy * 64 + i32(y);
    let column = tile_columns[tile.slot * TILE_COLUMNS + y * 64u + x];
    let at = wg.y * TILE_COLUMNS + y * 64u + x;
    // A chunk over the volume its footprint measured in this batch
    // (`tile_ranges`: the field's, its features' and far canopies' cells),
    // under no edits, holds no cell: air throughout, as past the face's
    // edge. (Chunks are asked for before their footprints are measured.)
    if li == 0u {
        let computed = u32(atomicLoad(&tile_ranges[arrayLength(&tile_ranges) - 1u]));
        var high = atomicLoad(&tile_ranges[job.tile * RANGE_WORDS + 1u]);
        for (var q = 1u; q < 4u; q++) { high = max(high, atomicLoad(&tile_ranges[job.tile * RANGE_WORDS + q * 4u + 1u])); }
        w_air = u32(job.tile < computed && job.brush_count == 0u && job.baked == NONE && job.cz * 64 >= high);
    }
    if workgroupUniformLoad(&w_air) != 0u {
        column_masks[at] = vec2<u32>(0u);
        column_materials[at] = M_AIR;
        if job.detail != NONE {
            for (var w = 0u; w < 16u; w++) { cell_materials[(job.detail * TILE_COLUMNS + y * 64u + x) * CELL_WORDS + w] = 0u; }
        }
        return;
    }
    // The footprint's cave primitives over the brick column, reaching the
    // chunk's layers or above (the natural top over it reads them too).
    let caves_here = volumes_caves() && i32(level) <= cave_deepest_level(true);
    let brick_first = vec2<i32>(i32((wg.x & 7u) * 8u), i32((wg.x >> 3u) * 8u));
    let prim_count = cull_caves(tile.slot, brick_first, level, (4 * job.cz * 64) << level, li, caves_here);
    if li == 0u {
        atomicStore(&w_feature_bits[0], 0u);
        atomicStore(&w_feature_bits[1], 0u);
        atomicStore(&w_feature_parts, 0u);
    }
    workgroupBarrier();
    let height = column.x;
    let top = div_floor(height, world.grid.y) >> level;
    let k0 = job.cz * 64;
    let p = domain_point(face, i, j, level);
    // Features over the terrain, under the edits (`Planet::sample_kind`):
    // the far forms' canopies, the near stamps over them. Every lane takes
    // part (the workgroup culls the placements together).
    var fc: FeatureColumn;
    if features[0] > 0u {
        let brick = vec2<i32>(tile.cx * 64 + i32((wg.x & 7u) * 8u), tile.cy * 64 + i32((wg.x >> 3u) * 8u));
        fc = feature_column(tile, brick, li, column.x != OUTSIDE, i, j, k0, column);
    }
    if column.x == OUTSIDE {
        column_masks[at] = vec2<u32>(0u);
        column_materials[at] = M_AIR;
        if job.detail != NONE {
            for (var w = 0u; w < 16u; w++) { cell_materials[(job.detail * TILE_COLUMNS + y * 64u + x) * CELL_WORDS + w] = 0u; }
        }
        return;
    }
    var mask = set_bits(vec2<u32>(0u), 0, top - k0, true);
    // Under edits, the natural top when the volume puts solid cells over
    // the chunk (`Planet::generated_top`: the first air over the highest).
    var above_top = OUTSIDE;
    if column.z > 0 || column.w > 0 {
        // The volume (`Planet::column_kind`): the cliff band, then the
        // caves over the column, within its extent.
        let k_lo = max(top - column.z, k0);
        let k_hi = min(top + column.w, k0 + 64);
        let half = 1 << level;
        var cliff = Cliff(0, 0, false, false, 0);
        if volumes_cliffs() && level < CLIFF_LEVELS {
            cliff = volume_cliff(p, material_slope(tile, i, j), level);
        }
        for (var k = max(top - cliff.cells, k_lo); k < min(top + cliff.cells, k_hi); k++) {
            let solid = volume_cliff_solid(cliff, volume_point_half(p, (k << (level + 1u)) + half), height, k, level);
            mask = set_bits(mask, k - k0, k - k0 + 1, solid);
        }
        let q = (4 * vec2<i32>(i32(x), i32(y)) + 2) << vec2<u32>(level);
        let origin = footprint_origin(tile);
        for (var c = 0u; c < prim_count; c++) {
            let prim = cave_primitive_of(w_prims[c]);
            if !cave_covers(prim, q.x, q.y) { continue; }
            let range = cave_z_range(prim);
            let lo = max(div_floor(range.x, 4 << level), k_lo);
            let hi = min(div_floor(range.y, 4 << level) + 1, k_hi);
            for (var k = lo; k < hi; k++) {
                if mask_bit(mask, k - k0) && volume_carves(prim, q.x, q.y, origin, k, level, height) {
                    mask = set_bits(mask, k - k0, k - k0 + 1, false);
                }
            }
        }
        if job.detail != NONE {
            for (var s = top + column.w - 1; s >= k0 + 64; s--) {
                var solid = s < top;
                if s >= top - cliff.cells && s < top + cliff.cells {
                    solid = volume_cliff_solid(cliff, volume_point_half(p, (s << (level + 1u)) + half), height, s, level);
                }
                for (var c = 0u; c < prim_count && solid; c++) {
                    let prim = cave_primitive_of(w_prims[c]);
                    if volume_carves(prim, q.x, q.y, origin, s, level, height) { solid = false; }
                }
                if solid {
                    above_top = s + 1;
                    break;
                }
            }
        }
    }
    // Edits in their order (`planet::Edits`): the large brushes, the baked
    // cells, the recent brushes.
    let ch = vec2<i32>(center_half(i, level), center_half(j, level));
    let natural = mask;
    if features[0] > 0u {
        mask = (mask & ~fc.air) | fc.solid;
        if (fc.solid.x | fc.solid.y | fc.air.x | fc.air.y) != 0u {
            let c = y * 64u + x;
            atomicOr(&job_features[wg.y * FEATURE_JOB_WORDS + (c >> 5u)], 1u << (c & 31u));
        }
    }
    let featured = fc.solid | fc.air;
    mask = apply_brushes(mask, job, 0u, job.large, p, ch, level, k0);
    mask = apply_baked(mask, job, wg.x, li);
    mask = apply_brushes(mask, job, job.large, job.brush_count, p, ch, level, k0);
    column_masks[at] = mask;
    // Materials of the highest solid cell and under it.
    var material = M_AIR;
    let high = select(i32(firstLeadingBit(mask.x)), 32 + i32(firstLeadingBit(mask.y)), mask.y != 0u);
    if mask.x != 0u || mask.y != 0u {
        let k = k0 + high;
        let t = vec4<i32>(
            tile_top(tile.slot, max(i32(x) - 1, 0), i32(y), level),
            tile_top(tile.slot, min(i32(x) + 1, 63), i32(y), level),
            tile_top(tile.slot, i32(x), max(i32(y) - 1, 0), level),
            tile_top(tile.slot, i32(x), min(i32(y) + 1, 63), level));
        let slope = column_slope(tile, x, y);
        // The terrain's own top (where its volume put it, under the height
        // field's at caves and overhangs) is surface; a top edits moved
        // lies as deep as it is under the terrain's.
        let natural_high = select(i32(firstLeadingBit(natural.x)), 32 + i32(firstLeadingBit(natural.y)), natural.y != 0u);
        let moved = natural.x == 0u && natural.y == 0u || natural_high != high;
        let depth = select(0, max(top - 1 - k, 0) << level, moved);
        // Each material at its own cell's volume point and with the
        // surface word's byte, as `Planet::material` evaluates it.
        let cell_half = 1 << level;
        let surface_word = u32(column.y) & 0xffu;
        let rules = ground_rules(surface_word, height, slope);
        let ground = column_ground_material(rules, volume_point_half(p, (k << (level + 1u)) + cell_half), surface_word, height, depth, slope, k << level);
        // A feature's cell takes its stamp's material; the terrain's top
        // under a far form shows it (a forest's colour where its canopy is
        // thinner than half a cell).
        var terrain_material = ground & M_ID;
        var speckled = (ground & M_SPECK) != 0u;
        var own = 0u;
        if bit_at(featured, u32(high)) {
            own = feature_material(fc, u32(high));
        } else if fc.far != 0u && !moved {
            own = fc.far;
        }
        if own != 0u {
            terrain_material = own;
            speckled = false;
        }
        let surface = edited_material(terrain_material, job, wg.x, li, p, ch, level, k, k0);
        let speck = select(0u, 0x80u, speckled && surface == terrain_material);
        // Under it: soil a cell below the terrain's top, rock below the
        // soil depth; an edited top's material throughout.
        var soil = surface;
        var rock = surface;
        if surface == terrain_material {
            soil = column_ground_material(rules, volume_point_half(p, ((top - 2) << (level + 1u)) + cell_half), surface_word, height, 1 << level, slope, (top - 2) << level) & M_ID;
            let deep = (terrain.header.z >> level) + 1;
            rock = column_ground_material(rules, volume_point_half(p, ((top - 1 - deep) << (level + 1u)) + cell_half), surface_word, height, deep << level, slope, (top - 1 - deep) << level) & M_ID;
        }
        let lowest = min(top, min(min(t.x, t.y), min(t.z, t.w)));
        material = (surface & 0x7fu) | speck | ((soil & 0xffu) << 8u) | ((rock & 0xffu) << 16u) | (u32(clamp(lowest - k0 + 128, 0, 255)) << 24u);
    }
    column_materials[at] = material;
    // Each solid cell's own material where the edits set one, then the
    // column's natural top (`Planet::material`'s depth counts from it): over
    // the chunk, else its highest natural cell's in it, else the
    // heightfield's.
    if job.detail != NONE {
        let cells = (job.detail * TILE_COLUMNS + y * 64u + x) * CELL_WORDS;
        // A natural column's cells set none (`column_cells` starts its
        // words from zero); a feature's take its stamp's material.
        let featured_column = (featured.x | featured.y) != 0u;
        for (var w = 0u; w < 16u && (job.brush_count > 0u || job.baked != NONE || featured_column); w++) {
            var word = 0u;
            for (var b = 0u; b < 4u; b++) {
                let z = w * 4u + b;
                if !bit_at(mask, z) { continue; }
                var own = 0u;
                if bit_at(featured, z) { own = feature_material(fc, z); }
                word |= edited_material(own, job, wg.x, li, p, ch, level, k0 + i32(z), k0) << (b * 8u);
            }
            cell_materials[cells + w] = word;
        }
        var natural_top = top;
        if above_top != OUTSIDE {
            natural_top = above_top;
        } else if natural.x != 0u || natural.y != 0u {
            let high = select(i32(firstLeadingBit(natural.x)), 32 + i32(firstLeadingBit(natural.y)), natural.y != 0u);
            natural_top = select(k0 + high + 1, select(top, k0 + 64, column.z > 0 || column.w > 0), high == 63);
        }
        cell_materials[cells + 16u] = bitcast<u32>(natural_top);
    }
}

// The materials' slope of column (x, y) of footprint `tile`:
// `Planet::material_slope` (one scale, level-4 relief, whatever level draws
// it) below MATERIAL_SLOPE_LEVEL. A coarser column spans many base columns:
// the gradient's length over 64 base cells (in the tile; eighths of a cell
// per cell).
fn column_slope(tile: TileJob, x: u32, y: u32) -> i32 {
    if tile.level < MATERIAL_SLOPE_LEVEL {
        return material_slope(tile, tile.cx * 64 + i32(x), tile.cy * 64 + i32(y));
    }
    let level = tile.level;
    let n = max(32 >> level, 1);
    let xa = min(i32(x) + n, 63);
    let xb = max(i32(x) - n, 0);
    let ya = min(i32(y) + n, 63);
    let yb = max(i32(y) - n, 0);
    let gx = f32(tile_top(tile.slot, xa, i32(y), level) - tile_top(tile.slot, xb, i32(y), level)) / f32(max(xa - xb, 1));
    let gy = f32(tile_top(tile.slot, i32(x), ya, level) - tile_top(tile.slot, i32(x), yb, level)) / f32(max(ya - yb, 1));
    return i32(round(length(vec2<f32>(gx, gy)) * 8.0));
}

// The top (level cells) of column (x, y) of the footprint in pool slot `slot`.
fn tile_top(slot: u32, x: i32, y: i32, level: u32) -> i32 {
    let h = tile_columns[slot * TILE_COLUMNS + u32(y) * 64u + u32(x)].x;
    return div_floor(select(h, 0, h == OUTSIDE), world.grid.y) >> level;
}

// A leaf's two material bytes from its columns' materials
// (`chunk_columns_main`): the surface, the top material of a column whose
// top cell is in the leaf (`has_top`, `top`), else the subsurface's; the
// subsurface, by the column whose risers it holds (`exposed_column`) at
// the leaf's centre (layer `lz` of a chunk of `level`): above the lowest
// terrain top around it (a natural riser) its surface, within the soil
// depth under it soil, else rock.
fn leaf_materials_of(column: u32, top: u32, has_top: u32, lz: u32, level: u32) -> u32 {
    let below = (i32(column >> 24u) - 128) - 1 - i32(lz * 4u + 2u);
    var sub = select((column >> 16u) & 0xffu, (column >> 8u) & 0xffu, (below << level) < terrain.header.z);
    if below < 0 { sub = column & 0x7fu; }
    return select(sub, top & 0x7fu, has_top != 0u) | (sub << 8u);
}

// Of two columns with cells in a leaf (`chunk_columns_main`'s words; 0:
// none yet), the one whose subsurface the leaf draws: of the lowest terrain
// top around it, the first of those. Its cells the leaf holds are the
// likeliest to be exposed (a riser over a lower neighbour), and those are
// the leaf's cells under others that show. (The first column the leaf met
// drew a slope's risers in an uphill corner's soil or rock: a forest's far
// canopy streaked with the terrain under it.)
fn exposed_column(held: u32, column: u32) -> u32 {
    return select(held, column, held == 0u || (column >> 24u) < (held >> 24u));
}

// One column's top material voting for the surface of the leaf its top
// cell is in (`chunk_build_main`): the leaf's top material and votes after
// it. With per-cell materials (`detailed`: a chunk of `DETAIL_LEVELS`) the
// pair only decides which cells need a byte of their own: a majority vote
// of all but single-voxel specks, which a leaf of 4x4 columns would spread,
// leaves the fewest. Without, the pair is what the leaf draws: one of its
// columns' tops, the one of least `pick` (a hash of the column), so over
// many leaves each material covers the share of the columns it tops, as
// the finer levels' cells average to. A majority drew the ground's
// minority materials (lunar ejecta and rock, specks) at no coarse level:
// the ground changed colour where a detail level met a coarser one.
fn vote_top(top: u32, votes: u32, column_material: u32, detailed: bool, pick: u32) -> vec2<u32> {
    if !detailed {
        let key = pick | 1u;
        if votes == 0u || key < votes { return vec2<u32>(column_material, key); }
        return vec2<u32>(top, votes);
    }
    if (column_material & 0x80u) != 0u {
        // A speck: the surface only while nothing else votes.
        return vec2<u32>(select(top, column_material, votes == 0u), votes);
    }
    if votes == 0u { return vec2<u32>(column_material, 1u); }
    if (top & 0x7fu) == (column_material & 0x7fu) { return vec2<u32>(top, votes + 1u); }
    return vec2<u32>(top, votes - 1u);
}

// Column (cx, cy) of chunk job `n` (`job`, footprint `tile`), its leaf
// column's leaves in `occupied` (bit lz), their material pairs at
// `w_cells_pairs[pairs + lz]`: each exposed solid cell's material (its
// edit's, else `Planet::material`'s natural one), cleared where its leaf's
// pair draws it (the surface's under air, the subsurface's under ground;
// both at the chunk's top layer; never in a leaf that would be implicit,
// bit lz of `implicit`, drawn with its mid's material), written back to
// the job's `cell_materials`. The leaves with any of its cells left (bit
// lz). A natural column's words are written only where cells are left: the
// rest are zero without being stored.
//
// In a chunk no edits are over, a heightfield column (no volume terms and
// no features) is solid up to its top, so its cells on the chunk's faces are exposed only
// where they are: under air at its top, beside a lower column (past the
// footprint's sides, a column like it); other columns and edited chunks
// count the chunk's faces as air. A natural top takes the material
// `chunk_columns_main` gave its column's top: the same evaluation.
fn column_cells(job: ChunkJob, n: u32, tile: TileJob, cx: u32, cy: u32, pairs: u32, occupied: u32, implicit: u32) -> u32 {
    let natural_job = job.brush_count == 0u && job.baked == NONE;
    // A column features changed stores its cells as an edited one.
    let natural = natural_job && !column_featured(n, cx, cy);
    let m = column_masks[n * TILE_COLUMNS + cy * 64u + cx];
    let cells = (job.detail * TILE_COLUMNS + cy * 64u + cx) * CELL_WORDS;
    if m.x == 0u && m.y == 0u {
        return 0u;
    }
    let natural_top = bitcast<i32>(cell_materials[cells + 16u]);
    let high = select(firstLeadingBit(m.x), 32u + firstLeadingBit(m.y), m.y != 0u);
    let top_material = column_materials[n * TILE_COLUMNS + cy * 64u + cx] & 0x7fu;
    let column = tile_columns[tile.slot * TILE_COLUMNS + cy * 64u + cx];
    let exact = natural && column.z == 0 && column.w == 0;
    // The columns beside it (past the chunk's sides: air, or for an
    // exact column one like it).
    let outside = select(vec2<u32>(0u), m, exact);
    var west = outside;
    var east = outside;
    var south = outside;
    var north = outside;
    if cx > 0u { west = column_masks[n * TILE_COLUMNS + cy * 64u + cx - 1u]; }
    if cx < 63u { east = column_masks[n * TILE_COLUMNS + cy * 64u + cx + 1u]; }
    if cy > 0u { south = column_masks[n * TILE_COLUMNS + (cy - 1u) * 64u + cx]; }
    if cy < 63u { north = column_masks[n * TILE_COLUMNS + (cy + 1u) * 64u + cx]; }
    // Its exposed cells: air over, under or beside them (the chunk's
    // top and bottom faces as above).
    let top_open = !exact || job.cz * 64 + 63 == natural_top - 1;
    let above = vec2<u32>((m.x >> 1u) | (m.y << 31u), (m.y >> 1u) | select(0x80000000u, 0u, top_open));
    let below = vec2<u32>((m.x << 1u) | select(0u, 1u, exact), (m.y << 1u) | (m.x >> 31u));
    let exposed = m & (~above | ~below | ~west | ~east | ~south | ~north);
    // The column's domain point, material slope and rules, once it has a
    // cell to evaluate.
    var p = vec3<i32>(0);
    var slope = 0;
    var rules = 0u;
    var located = false;
    var left = 0u;
    for (var lz = 0u; lz < 16u; lz++) {
        if ((occupied >> lz) & 1u) == 0u {
            continue;
        }
        // A natural leaf's cells are none but its exposed ones.
        let cells_in = select(m, exposed, natural);
        if ((select(cells_in.x, cells_in.y, lz >= 8u) >> ((lz & 7u) * 4u)) & 15u) == 0u {
            continue;
        }
        let pair = w_cells_pairs[pairs + lz];
        let surface = pair & 0xffu;
        let sub = (pair >> 8u) & 0xffu;
        let full = ((implicit >> lz) & 1u) != 0u;
        // Edits' materials as the columns wrote them (none: a natural
        // chunk's columns write only their natural tops).
        var word = 0u;
        if !natural { word = cell_materials[cells + lz]; }
        for (var b = 0u; b < 4u; b++) {
            let z = lz * 4u + b;
            var own = (word >> (b * 8u)) & 0xffu;
            if !bit_at(m, z) {
                continue;
            }
            let under_air = z < 63u && !bit_at(m, z + 1u);
            let k = job.cz * 64 + i32(z);
            let open = bit_at(exposed, z);
            if own == 0u && open && natural && z == high && k == natural_top - 1 {
                own = top_material;
            } else if own == 0u && open {
                let level = tile.level;
                if !located {
                    p = domain_point(tile.face, tile.cx * 64 + i32(cx), tile.cy * 64 + i32(cy), level);
                    slope = column_slope(tile, cx, cy);
                    rules = ground_rules(u32(column.y) & 0xffu, column.x, slope);
                    located = true;
                }
                let depth = max(natural_top - 1 - k, 0) << level;
                let q = volume_point_half(p, (k << (level + 1u)) + (1 << level));
                own = column_ground_material(rules, q, u32(column.y) & 0xffu, column.x, depth, slope, k << level) & M_ID;
            }
            let drawn = !full && ((z == 63u && own == surface && own == sub) || (z < 63u && own == select(sub, surface, under_air)));
            if own == 0u || drawn {
                own = 0u;
            }
            word = (word & ~(0xffu << (b * 8u))) | (own << (b * 8u));
        }
        if !natural || word != 0u { cell_materials[cells + lz] = word; }
        if word != 0u { left |= 1u << lz; }
    }
    return left;
}

// The brick column's columns (masks, materials) and its four leaf
// columns' (2x2 of 4x4 columns) leaf pairs (16 a leaf column) and leaves
// (occupied in bits 0..16, implicit in bits 16..32).
var<workgroup> w_cells_mask: array<vec2<u32>, 64>;
var<workgroup> w_cells_material: array<u32, 64>;
var<workgroup> w_cells_pairs: array<u32, 64>;
var<workgroup> w_cells_leaves: array<atomic<u32>, 4>;

// Each detail job's exposed cells' materials (`column_cells`), a lane a
// column, a workgroup a brick column (as `chunk_columns_main`): dispatch
// (64, chunks), before `chunk_build_main`, which reads each column's leaves
// with cells left (word 17 of its `cell_materials`). A lane a leaf of the
// brick column's leaf columns first finds the leaves' pairs, occupancy and
// implicit leaves as `chunk_build_main` does.
@compute @workgroup_size(64)
fn chunk_cells_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let job = chunk_jobs[wg.y];
    if job.detail == NONE { return; }
    let tile = tile_jobs[job.tile];
    let x = (wg.x & 7u) * 8u + (li & 7u);
    let y = (wg.x >> 3u) * 8u + (li >> 3u);
    let m = column_masks[wg.y * TILE_COLUMNS + y * 64u + x];
    w_cells_mask[li] = m;
    w_cells_material[li] = column_materials[wg.y * TILE_COLUMNS + y * 64u + x];
    if li < 4u { atomicStore(&w_cells_leaves[li], 0u); }
    workgroupBarrier();
    // Leaf lz of leaf column lc: its columns in `chunk_build_main`'s order.
    let lc = li >> 4u;
    let lz = li & 15u;
    var first = 0u;
    var top = 0u;
    var votes = 0u;
    var topped = false;
    var occupied = false;
    var full = true;
    for (var c = 0u; c < 16u; c++) {
        let b = ((lc >> 1u) * 4u + (c >> 2u)) * 8u + (lc & 1u) * 4u + (c & 3u);
        let cm = w_cells_mask[b];
        let nib = (select(cm.x, cm.y, lz >= 8u) >> ((lz & 7u) * 4u)) & 15u;
        full = full && nib == 15u;
        if cm.x == 0u && cm.y == 0u { continue; }
        let material = w_cells_material[b];
        let high = select(firstLeadingBit(cm.x), 32u + firstLeadingBit(cm.y), cm.y != 0u);
        if (high >> 2u) == lz {
            topped = true;
            let v = vote_top(top, votes, material, true, 0u);
            top = v.x;
            votes = v.y;
        }
        if nib != 0u { first = exposed_column(first, material); }
        occupied = occupied || nib != 0u;
    }
    w_cells_pairs[li] = select(0u, leaf_materials_of(first, top, u32(topped), lz, tile.level), occupied);
    atomicOr(&w_cells_leaves[lc], (u32(occupied) << lz) | (u32(full && !topped) << (lz + 16u)));
    workgroupBarrier();
    let own = ((y & 7u) >> 2u) * 2u + ((x & 7u) >> 2u);
    let leaves = atomicLoad(&w_cells_leaves[own]);
    let left = column_cells(job, wg.y, tile, x, y, own * 16u, leaves & 0xffffu, leaves >> 16u);
    cell_materials[(job.detail * TILE_COLUMNS + y * 64u + x) * CELL_WORDS + 17u] = left;
}

var<workgroup> w_occ: array<atomic<u32>, 128>;
var<workgroup> w_full: array<atomic<u32>, 128>;
var<workgroup> w_base: array<u32, 64>;
var<workgroup> w_z: array<atomic<i32>, 2>;
var<workgroup> w_total: u32;
// The chunk's pages (`leaf_pages`), and how many.
var<workgroup> w_pages: array<u32, 64>;
var<workgroup> w_page_count: u32;
var<workgroup> w_mid_material: array<atomic<u32>, 64>;
// Leaf pages holding a leaf with edited materials, and their detail pages.
var<workgroup> w_detail_need: array<atomic<u32>, 64>;
var<workgroup> w_detail: array<u32, 64>;
var<workgroup> w_detail_overflow: atomic<u32>;
// The column tops' least-squares sums (n, x, y, z, xx, xy, yy, xz, yz),
// the fitted slope, and the plane's height over it (1/256 cells).
var<workgroup> w_fit: array<atomic<i32>, 9>;
var<workgroup> w_slope: vec2<f32>;
var<workgroup> w_lift: atomic<i32>;

fn rank64(m: vec2<u32>, b: u32) -> u32 {
    // Set bits of `m` below bit `b`.
    if b < 32u { return countOneBits(m.x & ((1u << b) - 1u)); }
    return countOneBits(m.x) + countOneBits(m.y & ((1u << (b - 32u)) - 1u));
}

// One workgroup per chunk: thread t builds the 16 leaves above leaf column
// (t & 15, t >> 4) of the chunk.
@compute @workgroup_size(256)
fn chunk_build_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) t: u32) {
    let job = chunk_jobs[wg.x];
    if t < 128u {
        atomicStore(&w_occ[t], 0u);
        atomicStore(&w_full[t], 0u);
    }
    if t < 64u {
        atomicStore(&w_mid_material[t], 0u);
        atomicStore(&w_detail_need[t], 0u);
    }
    if t == 0u {
        atomicStore(&w_detail_overflow, 0u);
        slot_details[job.slot] = 0u;
    }
    if t < 9u { atomicStore(&w_fit[t], 0); }
    if t == 0u {
        atomicStore(&w_z[0], 64);
        atomicStore(&w_z[1], 0);
        atomicStore(&w_lift, -2147483647);
    }
    workgroupBarrier();
    let lx = t & 15u;
    let ly = t >> 4u;
    // 16 leaves along z, built from this leaf column's 4x4 cell columns.
    var leaf: array<vec2<u32>, 16>;
    // Each leaf's materials: its column of cells in it the most exposed's
    // (`exposed_column`), and for its surface the top material of the
    // columns whose top cell is in it (`vote_top`; `topped` where there are
    // any).
    var leaf_material: array<u32, 16>;
    var leaf_top: array<u32, 16>;
    var votes: array<u32, 16>;
    var topped = 0u;
    var z_lo = 64;
    var z_hi = 0;
    // Each column's top (one past its highest solid cell; 0: none).
    var tops: array<i32, 16>;
    var fit: array<i32, 9>;
    let tile = tile_jobs[job.tile];
    let detailed = job.detail != NONE;
    for (var c = 0u; c < 16u; c++) {
        let cx = lx * 4u + (c & 3u);
        let cy = ly * 4u + (c >> 2u);
        let m = column_masks[wg.x * TILE_COLUMNS + cy * 64u + cx];
        if m.x == 0u && m.y == 0u { continue; }
        let column_material = column_materials[wg.x * TILE_COLUMNS + cy * 64u + cx];
        z_lo = min(z_lo, select(i32(firstTrailingBit(m.x)), 32 + i32(firstTrailingBit(m.y)), m.x == 0u));
        let high = select(i32(firstLeadingBit(m.x)), 32 + i32(firstLeadingBit(m.y)), m.y != 0u);
        z_hi = max(z_hi, 1 + high);
        tops[c] = 1 + high;
        let x = i32(cx);
        let y = i32(cy);
        let z = 1 + high;
        fit[0] += 1;
        fit[1] += x;
        fit[2] += y;
        fit[3] += z;
        fit[4] += x * x;
        fit[5] += x * y;
        fit[6] += y * y;
        fit[7] += x * z;
        fit[8] += y * z;
        let top_leaf = u32(high) >> 2u;
        topped |= 1u << top_leaf;
        let pick = hash3(tile.cx * 64 + i32(cx), tile.cy * 64 + i32(cy), i32(tile.level | (tile.face << 8u)), 0x7f4a7c15u);
        let vote = vote_top(leaf_top[top_leaf], votes[top_leaf], column_material, detailed, pick);
        leaf_top[top_leaf] = vote.x;
        votes[top_leaf] = vote.y;
        for (var lz = 0u; lz < 16u; lz++) {
            let nib = (select(m.x, m.y, lz >= 8u) >> ((lz & 7u) * 4u)) & 15u;
            if nib != 0u { leaf_material[lz] = exposed_column(leaf_material[lz], column_material); }
            leaf[lz].x |= ((nib & 1u) << c) | (((nib >> 1u) & 1u) << (c + 16u));
            leaf[lz].y |= (((nib >> 2u) & 1u) << c) | (((nib >> 3u) & 1u) << (c + 16u));
        }
    }
    atomicMin(&w_z[0], z_lo);
    atomicMax(&w_z[1], z_hi);
    for (var k = 0u; k < 9u; k++) { atomicAdd(&w_fit[k], fit[k]); }
    // The heightfield's slope over this leaf column: its exact heights a
    // column past each side (central differences, one-sided at the tile's
    // edge), where every column's top here is the heightfield's.
    let cell_mm = f32(world.grid.y << tile.level);
    let base = job.cz * 64;
    var natural = true;
    for (var c = 0u; c < 16u; c++) {
        let height = tile_columns[tile.slot * TILE_COLUMNS + (ly * 4u + (c >> 2u)) * 64u + lx * 4u + (c & 3u)].x;
        if height == OUTSIDE { natural = false; break; }
        let hf = clamp((div_floor(height, world.grid.y) >> tile.level) - base, 0, 64);
        if tops[c] != hf && !(hf == 64 && tops[c] == 64) { natural = false; }
    }
    var ground = 0u;
    if natural && topped != 0u {
        let x0 = max(i32(lx * 4u) - 1, 0);
        let x1 = min(i32(lx * 4u) + 4, 63);
        let y0 = max(i32(ly * 4u) - 1, 0);
        let y1 = min(i32(ly * 4u) + 4, 63);
        var g = vec2<f32>(0.0);
        var valid = true;
        for (var r = 0u; r < 4u; r++) {
            let row = i32(ly * 4u + r);
            let col = i32(lx * 4u + r);
            let hx0 = tile_columns[tile.slot * TILE_COLUMNS + u32(row) * 64u + u32(x0)].x;
            let hx1 = tile_columns[tile.slot * TILE_COLUMNS + u32(row) * 64u + u32(x1)].x;
            let hy0 = tile_columns[tile.slot * TILE_COLUMNS + u32(y0) * 64u + u32(col)].x;
            let hy1 = tile_columns[tile.slot * TILE_COLUMNS + u32(y1) * 64u + u32(col)].x;
            if hx0 == OUTSIDE || hx1 == OUTSIDE || hy0 == OUTSIDE || hy1 == OUTSIDE { valid = false; }
            g += vec2<f32>(f32(hx1 - hx0) / f32(x1 - x0), f32(hy1 - hy0) / f32(y1 - y0));
        }
        if valid {
            g = g / (4.0 * cell_mm);
            ground = encode_slope(g.x) | (encode_slope(g.y) << 8u);
        }
    }
    var occupied = 0u;
    var full = 0u;
    for (var lz = 0u; lz < 16u; lz++) {
        let m = (lx >> 2u) + (ly >> 2u) * 4u + (lz >> 2u) * 16u;
        let l = (lx & 3u) + (ly & 3u) * 4u + (lz & 3u) * 16u;
        if leaf[lz].x != 0u || leaf[lz].y != 0u {
            let pair = leaf_materials_of(leaf_material[lz], leaf_top[lz], (topped >> lz) & 1u, lz, tile.level);
            occupied |= 1u << lz;
            atomicOr(&w_occ[m * 2u + (l >> 5u)], 1u << (l & 31u));
            atomicMax(&w_mid_material[m], pair);
        }
        if leaf[lz].x == 0xffffffffu && leaf[lz].y == 0xffffffffu && ((topped >> lz) & 1u) == 0u { full |= 1u << lz; }
    }
    // Leaves (bit lz) holding cells their pair would draw wrong
    // (`chunk_cells_main`, at `DETAIL_LEVELS`).
    var edited = 0u;
    if job.detail != NONE {
        for (var c = 0u; c < 16u; c++) {
            edited |= cell_materials[(job.detail * TILE_COLUMNS + (ly * 4u + (c >> 2u)) * 64u + lx * 4u + (c & 3u)) * CELL_WORDS + 17u];
        }
    }
    // A full leaf is implicit (its mid's material) unless a column tops in
    // it (then it is the surface and keeps its own materials: not in
    // `full`) or its pair draws cells wrong.
    for (var lz = 0u; lz < 16u; lz++) {
        if (((full & ~edited) >> lz) & 1u) != 0u {
            let m = (lx >> 2u) + (ly >> 2u) * 4u + (lz >> 2u) * 16u;
            let l = (lx & 3u) + (ly & 3u) * 4u + (lz & 3u) * 16u;
            atomicOr(&w_full[m * 2u + (l >> 5u)], 1u << (l & 31u));
        }
    }
    workgroupBarrier();
    // The tops' plane: its slope by least squares, then lifted over every
    // column's top across the column's whole footprint.
    if t == 0u {
        var slope = vec2<f32>(0.0);
        let n = f32(atomicLoad(&w_fit[0]));
        if n >= 3.0 {
            let mean = vec3<f32>(f32(atomicLoad(&w_fit[1])), f32(atomicLoad(&w_fit[2])), f32(atomicLoad(&w_fit[3]))) / n;
            let sxx = f32(atomicLoad(&w_fit[4])) / n - mean.x * mean.x;
            let sxy = f32(atomicLoad(&w_fit[5])) / n - mean.x * mean.y;
            let syy = f32(atomicLoad(&w_fit[6])) / n - mean.y * mean.y;
            let sxz = f32(atomicLoad(&w_fit[7])) / n - mean.x * mean.z;
            let syz = f32(atomicLoad(&w_fit[8])) / n - mean.y * mean.z;
            let det = sxx * syy - sxy * sxy;
            if det > 1e-3 {
                slope = clamp(vec2<f32>(sxz * syy - syz * sxy, syz * sxx - sxz * sxy) / det, vec2<f32>(-8.0), vec2<f32>(8.0));
            }
        }
        w_slope = slope;
    }
    workgroupBarrier();
    let slope = w_slope;
    var lift = -2147483647;
    for (var c = 0u; c < 16u; c++) {
        if tops[c] == 0 { continue; }
        // The plane's lowest point over the column.
        let low = vec2<f32>(f32(lx * 4u + (c & 3u)), f32(ly * 4u + (c >> 2u))) + select(vec2<f32>(0.0), vec2<f32>(1.0), slope < vec2<f32>(0.0));
        lift = max(lift, i32(ceil((f32(tops[c]) - dot(slope, low)) * 256.0)));
    }
    atomicMax(&w_lift, lift);
    workgroupBarrier();
    if t == 0u {
        // Mixed leaves per mid node, packed in mid order.
        var total = 0u;
        for (var m = 0u; m < 64u; m++) {
            w_base[m] = total;
            let occ = vec2<u32>(atomicLoad(&w_occ[m * 2u]), atomicLoad(&w_occ[m * 2u + 1u]));
            let full = vec2<u32>(atomicLoad(&w_full[m * 2u]), atomicLoad(&w_full[m * 2u + 1u]));
            total += countOneBits(occ.x & ~full.x) + countOneBits(occ.y & ~full.y);
        }
        let pages = (total + 63u) >> 6u;
        var first = 0u;
        if pages > 0u { first = take_pages(pages); }
        let overflow = first == NONE;
        for (var p = 0u; p < pages && !overflow; p++) {
            w_pages[p] = free_pages[first + p];
            leaf_pages[job.slot * LEAF_PAGES + p] = w_pages[p];
        }
        var root_occ = vec2<u32>(0u);
        var root_full = vec2<u32>(0u);
        for (var m = 0u; m < 64u; m++) {
            let occ = vec2<u32>(atomicLoad(&w_occ[m * 2u]), atomicLoad(&w_occ[m * 2u + 1u]));
            let full = vec2<u32>(atomicLoad(&w_full[m * 2u]), atomicLoad(&w_full[m * 2u + 1u]));
            mids[job.slot * 64u + m] = vec4<u32>(occ, full);
            mid_leaves[job.slot * 64u + m] = w_base[m];
            mid_materials[job.slot * 64u + m] = atomicLoad(&w_mid_material[m]);
            let bit = 1u << (m & 31u);
            if occ.x != 0u || occ.y != 0u {
                if m < 32u { root_occ.x |= bit; } else { root_occ.y |= bit; }
            }
            if full.x == 0xffffffffu && full.y == 0xffffffffu {
                if m < 32u { root_full.x |= bit; } else { root_full.y |= bit; }
            }
        }
        w_total = select(total, 0u, overflow);
        w_page_count = select(pages, 0u, overflow);
        let z = vec2<i32>(atomicLoad(&w_z[0]), atomicLoad(&w_z[1]));
        // A sixty-fourth of a cell over the tops: rays rasterized against
        // it and walked in f32 meet it where they meet the cells. A plane
        // rising past the chunk's cube anywhere would reach into the chunk
        // above, whose cells a ray seeded here would skip: the box's top is
        // then the occupied layers' (boxes never overlap).
        var top = vec4<f32>(w_slope, f32(atomicLoad(&w_lift)) / 256.0 + 1.0 / 64.0, 0.0);
        if top.z + max(top.x, 0.0) * 64.0 + max(top.y, 0.0) * 64.0 > 64.0 { top = vec4<f32>(0.0, 0.0, f32(z.y), 0.0); }
        chunk_meta[job.slot] = ChunkMeta(root_occ, root_full, z, select(total, 0u, overflow), select(pages, 0u, overflow), top);
        var flags = 1;
        if root_occ.x == 0u && root_occ.y == 0u { flags |= 2; }
        if root_full.x == 0xffffffffu && root_full.y == 0xffffffffu { flags |= 4; }
        if overflow { flags |= 8; }
        // (Flag 16, out of detail pages, is set once the detail pages are
        // taken below.)
        // The columns' mean top (quarter cells; 0xffff: none), which the
        // chunk's frame is made about.
        let columns = atomicLoad(&w_fit[0]);
        let surface = select(0xffffu, u32(clamp(round(f32(atomicLoad(&w_fit[3])) / f32(max(columns, 1)) * 4.0), 0.0, 256.0)), columns > 0);
        flags |= i32(surface << 16u);
        job_results[wg.x] = vec4<i32>(i32(job.slot), z.x, z.y, flags);
    }
    workgroupBarrier();
    let total = workgroupUniformLoad(&w_total);
    if total == 0u { return; }
    let pages = w_page_count;
    // Detail pages: one for each leaf page holding a leaf with edited
    // materials; NONE for the chunk's other pages (they may have been
    // another chunk's).
    if job.detail != NONE {
        for (var lz = 0u; lz < 16u; lz++) {
            if ((edited >> lz) & 1u) == 0u { continue; }
            let m = (lx >> 2u) + (ly >> 2u) * 4u + (lz >> 2u) * 16u;
            let l = (lx & 3u) + (ly & 3u) * 4u + (lz & 3u) * 16u;
            let mixed = vec2<u32>(atomicLoad(&w_occ[m * 2u]), atomicLoad(&w_occ[m * 2u + 1u])) & ~vec2<u32>(atomicLoad(&w_full[m * 2u]), atomicLoad(&w_full[m * 2u + 1u]));
            if bit_at(mixed, l) { atomicOr(&w_detail_need[(w_base[m] + rank64(mixed, l)) >> 6u], 1u); }
        }
    }
    workgroupBarrier();
    if t < pages {
        var detail = NONE;
        if atomicLoad(&w_detail_need[t]) != 0u {
            detail = take_detail();
            if detail == NONE { atomicStore(&w_detail_overflow, 1u); }
        }
        w_detail[t] = detail;
        detail_pages[w_pages[t]] = detail;
    }
    workgroupBarrier();
    if t == 0u {
        if atomicLoad(&w_detail_overflow) != 0u { job_results[wg.x].w |= 16; }
        var any = 0u;
        for (var p = 0u; p < pages; p++) { any |= u32(w_detail[p] != NONE); }
        slot_details[job.slot] = any;
    }
    // The detail pages cleared (leaves without edited cells read zero).
    for (var w = t; w < pages * DETAIL_WORDS; w += 256u) {
        let detail = w_detail[w / DETAIL_WORDS];
        if detail != NONE { detail_cells[detail * DETAIL_WORDS + (w % DETAIL_WORDS)] = 0u; }
    }
    // The pages' material bytes from earlier owners, cleared (32 words a
    // page).
    for (var w = t; w < pages * 32u; w += 256u) {
        atomicStore(&leaf_materials[w_pages[w >> 5u] * 32u + (w & 31u)], 0u);
        atomicStore(&leaf_normals[w_pages[w >> 5u] * 32u + (w & 31u)], 0u);
    }
    storageBarrier();
    for (var lz = 0u; lz < 16u; lz++) {
        let m = (lx >> 2u) + (ly >> 2u) * 4u + (lz >> 2u) * 16u;
        let l = (lx & 3u) + (ly & 3u) * 4u + (lz & 3u) * 16u;
        let occ = vec2<u32>(atomicLoad(&w_occ[m * 2u]), atomicLoad(&w_occ[m * 2u + 1u]));
        let full = vec2<u32>(atomicLoad(&w_full[m * 2u]), atomicLoad(&w_full[m * 2u + 1u]));
        let mixed = occ & ~full;
        if bit_at(mixed, l) {
            let order = w_base[m] + rank64(mixed, l);
            let index = (w_pages[order >> 6u] << 6u) | (order & 63u);
            leaves[index] = leaf[lz];
            atomicOr(&leaf_materials[index >> 1u], leaf_materials_of(leaf_material[lz], leaf_top[lz], (topped >> lz) & 1u, lz, tile_jobs[job.tile].level) << ((index & 1u) * 16u));
            if ((topped >> lz) & 1u) != 0u {
                atomicOr(&leaf_normals[index >> 1u], ground << ((index & 1u) * 16u));
            }
            // Its cells' edited materials, column by column.
            let detail = w_detail[order >> 6u];
            if ((edited >> lz) & 1u) != 0u && detail != NONE {
                // A column's word is nonzero where it has cells left (a
                // natural column stored only those).
                for (var c = 0u; c < 16u; c++) {
                    let at = (job.detail * TILE_COLUMNS + (lx * 4u + (c & 3u)) + (ly * 4u + (c >> 2u)) * 64u) * CELL_WORDS;
                    var cells = 0u;
                    if ((cell_materials[at + 17u] >> lz) & 1u) != 0u { cells = cell_materials[at + lz]; }
                    detail_cells[detail * DETAIL_WORDS + (order & 63u) * 16u + c] = cells;
                }
            }
        }
    }
}

// ---- Kept chunks (`docs/chunk-octree.md`, "Edited chunks kept") ----
//
// An edited chunk the plan evicts is packed before its pages go back
// (`pack_main`) and read back; a node packed and unedited since is
// restored into a new slot by copying (`restore_main`), not generated
// again from the field and the history. A packed chunk: PACK_HEADER words
// (its `ChunkMeta`, the mask of its pages with detail pages, its size),
// its mids' words, mid leaves and mid materials (384), then each page's
// leaves (128 words), materials (32), normals (32), then its detail pages
// (DETAIL_WORDS each, in page order).
const PACK_HEADER: u32 = 16u;
const PACK_LEAVES_AT: u32 = 400u;

// Pack jobs: (slot, -, -, -); the packs; the space used, then each job's
// (offset or NONE, words).
@group(1) @binding(0) var<storage, read> pack_jobs: array<vec4<u32>>;
@group(1) @binding(1) var<storage, read_write> pack_data: array<u32>;
@group(1) @binding(2) var<storage, read_write> pack_alloc: array<atomic<u32>>;
// Restore jobs: (slot, its pack's offset in `restore_data`, its result's
// index in `job_results`, its result flags); the packs.
@group(1) @binding(3) var<storage, read> restore_jobs: array<vec4<u32>>;
@group(1) @binding(4) var<storage, read> restore_data: array<u32>;

fn pack_words(pages: u32, details: u32) -> u32 {
    return PACK_LEAVES_AT + pages * 192u + details * DETAIL_WORDS;
}

var<workgroup> w_pack: vec4<u32>;
var<workgroup> w_pack_pages: array<u32, 64>;
// Each page's detail page (NONE: none) and its place among the packed ones.
var<workgroup> w_pack_detail: array<u32, 64>;
var<workgroup> w_pack_index: array<u32, 64>;

// One workgroup a chunk: dispatch (pack jobs), before `evict`.
@compute @workgroup_size(256)
fn pack_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) t: u32) {
    let slot = pack_jobs[wg.x].x;
    if t == 0u {
        let header = chunk_meta[slot];
        var mask = vec2<u32>(0u);
        var details = 0u;
        for (var p = 0u; p < header.pages; p++) {
            let page = leaf_pages[slot * LEAF_PAGES + p];
            w_pack_pages[p] = page;
            w_pack_detail[p] = detail_pages[page];
            w_pack_index[p] = details;
            if w_pack_detail[p] != NONE {
                if p < 32u { mask.x |= 1u << p; } else { mask.y |= 1u << (p - 32u); }
                details += 1u;
            }
        }
        let words = pack_words(header.pages, details);
        let at = atomicAdd(&pack_alloc[0], words);
        // Within the buffer without wrapping (the counter runs on past it).
        let room = arrayLength(&pack_data);
        let ok = words <= room && at <= room - words;
        atomicStore(&pack_alloc[1u + 2u * wg.x], select(NONE, at, ok));
        atomicStore(&pack_alloc[2u + 2u * wg.x], words);
        w_pack = vec4<u32>(at, u32(ok), header.pages, 0u);
        if ok {
            pack_data[at] = header.root_occ.x;
            pack_data[at + 1u] = header.root_occ.y;
            pack_data[at + 2u] = header.root_full.x;
            pack_data[at + 3u] = header.root_full.y;
            pack_data[at + 4u] = bitcast<u32>(header.z.x);
            pack_data[at + 5u] = bitcast<u32>(header.z.y);
            pack_data[at + 6u] = header.leaves;
            pack_data[at + 7u] = header.pages;
            pack_data[at + 8u] = bitcast<u32>(header.top.x);
            pack_data[at + 9u] = bitcast<u32>(header.top.y);
            pack_data[at + 10u] = bitcast<u32>(header.top.z);
            pack_data[at + 11u] = bitcast<u32>(header.top.w);
            pack_data[at + 12u] = mask.x;
            pack_data[at + 13u] = mask.y;
            pack_data[at + 14u] = words;
            pack_data[at + 15u] = 0u;
        }
    }
    workgroupBarrier();
    let at = w_pack.x;
    let pages = w_pack.z;
    if w_pack.y == 0u { return; }
    for (var i = t; i < 256u; i += 256u) { pack_data[at + PACK_HEADER + i] = mids[slot * 64u + i / 4u][i % 4u]; }
    for (var i = t; i < 64u; i += 256u) {
        pack_data[at + PACK_HEADER + 256u + i] = mid_leaves[slot * 64u + i];
        pack_data[at + PACK_HEADER + 320u + i] = mid_materials[slot * 64u + i];
    }
    let leaves_at = at + PACK_LEAVES_AT;
    for (var w = t; w < pages * 128u; w += 256u) {
        let leaf = w_pack_pages[w / 128u] * 64u + (w % 128u) / 2u;
        pack_data[leaves_at + w] = leaves[leaf][w % 2u];
    }
    let materials_at = leaves_at + pages * 128u;
    let normals_at = materials_at + pages * 32u;
    for (var w = t; w < pages * 32u; w += 256u) {
        let word = w_pack_pages[w / 32u] * 32u + w % 32u;
        pack_data[materials_at + w] = atomicLoad(&leaf_materials[word]);
        pack_data[normals_at + w] = atomicLoad(&leaf_normals[word]);
    }
    let details_at = normals_at + pages * 32u;
    for (var w = t; w < pages * DETAIL_WORDS; w += 256u) {
        let p = w / DETAIL_WORDS;
        if w_pack_detail[p] != NONE {
            pack_data[details_at + w_pack_index[p] * DETAIL_WORDS + w % DETAIL_WORDS] = detail_cells[w_pack_detail[p] * DETAIL_WORDS + w % DETAIL_WORDS];
        }
    }
}

var<workgroup> w_restore: vec4<u32>;
var<workgroup> w_restore_pages: array<u32, 64>;
var<workgroup> w_restore_detail: array<u32, 64>;
var<workgroup> w_restore_index: array<u32, 64>;

// One workgroup a chunk: dispatch (restore jobs), after `chunk_build_main`.
// Its pages (and detail pages) come off the free stacks as a build's do;
// out of leaf pages it stores nothing (flag 8, as a build), out of detail
// pages its leaves' pairs show (flag 16).
@compute @workgroup_size(256)
fn restore_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) t: u32) {
    let job = restore_jobs[wg.x];
    let slot = job.x;
    let at = job.y;
    if t == 0u {
        let pages = restore_data[at + 7u];
        var first = 0u;
        if pages > 0u { first = take_pages(pages); }
        let ok = first != NONE;
        let mask = vec2<u32>(restore_data[at + 12u], restore_data[at + 13u]);
        var flags = job.w;
        var details = 0u;
        var any = 0u;
        for (var p = 0u; p < pages && ok; p++) {
            let page = free_pages[first + p];
            w_restore_pages[p] = page;
            leaf_pages[slot * LEAF_PAGES + p] = page;
            var detail = NONE;
            if bit_at(mask, p) {
                w_restore_index[p] = details;
                details += 1u;
                detail = take_detail();
                if detail == NONE { flags |= 16u; } else { any = 1u; }
            }
            w_restore_detail[p] = detail;
            detail_pages[page] = detail;
        }
        slot_details[slot] = any;
        let z = vec2<i32>(bitcast<i32>(restore_data[at + 4u]), bitcast<i32>(restore_data[at + 5u]));
        if ok {
            chunk_meta[slot] = ChunkMeta(
                vec2<u32>(restore_data[at], restore_data[at + 1u]),
                vec2<u32>(restore_data[at + 2u], restore_data[at + 3u]),
                z,
                restore_data[at + 6u],
                pages,
                bitcast<vec4<f32>>(vec4<u32>(restore_data[at + 8u], restore_data[at + 9u], restore_data[at + 10u], restore_data[at + 11u])),
            );
        } else {
            chunk_meta[slot].leaves = 0u;
            chunk_meta[slot].pages = 0u;
            flags |= 8u;
        }
        job_results[job.z] = vec4<i32>(i32(slot), z.x, z.y, bitcast<i32>(flags));
        w_restore = vec4<u32>(u32(ok), pages, 0u, 0u);
    }
    workgroupBarrier();
    if w_restore.x == 0u { return; }
    let pages = w_restore.y;
    for (var i = t; i < 256u; i += 256u) { mids[slot * 64u + i / 4u][i % 4u] = restore_data[at + PACK_HEADER + i]; }
    for (var i = t; i < 64u; i += 256u) {
        mid_leaves[slot * 64u + i] = restore_data[at + PACK_HEADER + 256u + i];
        mid_materials[slot * 64u + i] = restore_data[at + PACK_HEADER + 320u + i];
    }
    let leaves_at = at + PACK_LEAVES_AT;
    for (var w = t; w < pages * 128u; w += 256u) {
        let leaf = w_restore_pages[w / 128u] * 64u + (w % 128u) / 2u;
        leaves[leaf][w % 2u] = restore_data[leaves_at + w];
    }
    let materials_at = leaves_at + pages * 128u;
    let normals_at = materials_at + pages * 32u;
    for (var w = t; w < pages * 32u; w += 256u) {
        let word = w_restore_pages[w / 32u] * 32u + w % 32u;
        atomicStore(&leaf_materials[word], restore_data[materials_at + w]);
        atomicStore(&leaf_normals[word], restore_data[normals_at + w]);
    }
    let details_at = normals_at + pages * 32u;
    for (var w = t; w < pages * DETAIL_WORDS; w += 256u) {
        let p = w / DETAIL_WORDS;
        if w_restore_detail[p] != NONE {
            detail_cells[w_restore_detail[p] * DETAIL_WORDS + w % DETAIL_WORDS] = restore_data[details_at + w_restore_index[p] * DETAIL_WORDS + w % DETAIL_WORDS];
        }
    }
}

// Features (`features.rs`, docs/features.md): stamps that spawns place, a
// function of the terrain. `features` holds the set
// (`CompiledFeatures::gpu_words`). Each footprint of a near level keeps its
// near spawns' placements in its pool slot of `tile_placements` (the set's
// stride of squares), square by square (`tile_features_main`), and chunk
// columns rasterize them (`feature_column`) over the terrain, under the
// edits.
@group(0) @binding(36) var<storage, read> features: array<u32>;
// A square's raw candidate: x = 1 | di << 1 | dj << 11 | its stamp's index
// in its spawn << 21 | excluded << 28 | turn << 29 | mirror << 31 (0:
// none), y = its anchor layer (di, dj: the anchor's column in the square;
// excluded: an earlier spawn's raw candidate meets its footprint, so it is
// not placed).
@group(0) @binding(37) var<storage, read_write> tile_placements: array<vec2<u32>>;
// Per chunk job, a bit per column (`FEATURE_JOB_WORDS` words) whose cells
// features changed: their cells' materials are stored as edited ones are.
@group(0) @binding(38) var<storage, read_write> job_features: array<atomic<u32>>;
// Candidate blocks (`chunks::candidates`): spawns' raw candidates by blocks
// of 8x8 squares (a square's word pair at `BLOCK_WORDS` words a pool slot),
// each evaluated once while the pool keeps it; then the batch's block jobs
// (`BlockJob`), each job's node quads (count, origins) and reads (a quad
// a candidate's ground read, `BLOCK_READS` a job), and each computed
// footprint's blocks (`tile_block_word`). Offsets: `FEATURE_*_AT`.
@group(0) @binding(39) var<storage, read_write> feature_blocks: array<u32>;
const FEATURE_JOB_WORDS: u32 = 128u;

const MAX_SPAWNS: u32 = 32u;
const NEAR_LEVELS: u32 = 10u;
const NO_RELIEF: i32 = 0x7fffffff;

fn spawn_word(s: u32, w: u32) -> u32 {
    return features[features[1] + 24u * s + w];
}

// Whether ground at layer `ground` of a base column (its domain point `p`)
// lies within spawn `s`'s heights and climate (words 21 to 23: its
// temperature range, moisture range) as they wander there
// (`CompiledSpawn::grows_at`).
fn spawn_grows_at(s: u32, p: vec3<i32>, ground: i32) -> bool {
    var g = ground;
    let shift = spawn_word(s, 20u);
    if shift > 0u {
        g += scale_q16(noise(p, shift, spawn_word(s, 5u) ^ 0x7a3de1f1u), bitcast<i32>(spawn_word(s, 19u)));
    }
    if g < bitcast<i32>(spawn_word(s, 8u)) || g > bitcast<i32>(spawn_word(s, 9u)) { return false; }
    let lo = bitcast<i32>(spawn_word(s, 21u));
    let hi = bitcast<i32>(spawn_word(s, 22u));
    let wet = spawn_word(s, 23u);
    if lo == i32(-2147483647 - 1) && hi == 2147483647 && wet == 0xffff0000u { return true; }
    let c = terrain_climate(p, g * world.grid.y);
    return c.x >= lo && c.x <= hi && c.y >= i32(wet & 0xffffu) && c.y <= i32(wet >> 16u);
}

fn stamp_word(t: u32, w: u32) -> u32 {
    return features[features[2] + 16u * t + w];
}

// Spawns whose placements a footprint of `level` keeps
// (`CompiledFeatures::near`).
fn feature_near(level: u32) -> u32 {
    if level >= NEAR_LEVELS { return 0u; }
    return features[features[3] + level * 4u * (1u + MAX_SPAWNS)];
}

// Whether spawn `s` draws as its stamps at `level`
// (`CompiledFeatures::drawn`).
fn feature_drawn(level: u32, s: u32) -> bool {
    if level >= NEAR_LEVELS { return false; }
    return ((features[features[3] + level * 4u * (1u + MAX_SPAWNS) + 1u] >> s) & 1u) != 0u;
}

// Spawn `s`'s squares in a footprint of `level`: offset, side, margin
// (`features::SquareGrid`).
fn feature_grid(level: u32, s: u32) -> vec3<i32> {
    let at = features[3] + 4u * ((1u + MAX_SPAWNS) * level + 1u + s);
    return vec3<i32>(i32(features[at]), i32(features[at + 1u]), i32(features[at + 2u]));
}

// Whether a spawn's surface mask (`spawn_word` 6, 7) takes `material`
// (an empty mask takes any).
fn spawn_surface(s: u32, material: u32) -> bool {
    let surfaces = vec2<u32>(spawn_word(s, 6u), spawn_word(s, 7u));
    if (surfaces.x | surfaces.y) == 0u { return true; }
    return material < 64u && ((select(surfaces.x, surfaces.y, material >= 32u) >> (material & 31u)) & 1u) != 0u;
}

struct Placed {
    valid: bool,
    excluded: bool,
    stamp: u32,
    anchor: vec3<i32>,
    turn: u32,
    mirror: bool,
    // The stamp's reach (columns each way from the anchor).
    reach: i32,
}

// Spawn `s`'s placement on square `g` from footprint `tile`'s candidates.
fn tile_placement(tile: TileJob, s: u32, g: vec2<i32>) -> Placed {
    var p = tile_candidate(tile, s, g);
    p.valid = p.valid && !p.excluded;
    return p;
}

// Spawn `s`'s squares in footprint `tile`: the first, and the index of
// square `g` among them (-1: none of them).
fn tile_square(tile: TileJob, s: u32, g: vec2<i32>) -> i32 {
    let grid = feature_grid(tile.level, s);
    let sp = i32(spawn_word(s, 0u));
    let first = vec2<i32>(div_floor((tile.cx * 64 << tile.level) - grid.z, sp), div_floor((tile.cy * 64 << tile.level) - grid.z, sp));
    let n = g - first;
    if any(n < vec2<i32>(0)) || any(n >= vec2<i32>(grid.y)) { return -1; }
    return n.y * grid.y + n.x;
}

// Spawn `s`'s raw candidate on square `g` from footprint `tile`'s.
fn tile_candidate(tile: TileJob, s: u32, g: vec2<i32>) -> Placed {
    let n = tile_square(tile, s, g);
    if n < 0 { return Placed(); }
    return placed_of(s, g, tile_placements[tile.slot * features[4] + u32(i32(feature_grid(tile.level, s).x) + n)]);
}

// Spawn `s`'s candidate on square `g` from its word pair `e`.
fn placed_of(s: u32, g: vec2<i32>, e: vec2<u32>) -> Placed {
    var p: Placed;
    let sp = i32(spawn_word(s, 0u));
    if e.x == 0u { return p; }
    p.valid = true;
    p.excluded = ((e.x >> 28u) & 1u) != 0u;
    p.stamp = features[spawn_word(s, 12u) + ((e.x >> 21u) & 0x7fu)];
    p.anchor = vec3<i32>(g.x * sp + i32((e.x >> 1u) & 1023u), bitcast<i32>(e.y), g.y * sp + i32((e.x >> 11u) & 1023u));
    p.turn = (e.x >> 29u) & 3u;
    p.mirror = (e.x >> 31u) == 1u;
    p.reach = i32(stamp_word(p.stamp, 3u));
    return p;
}

// Spawn `s`'s squares whose placements may reach base columns [lo, hi]
// (inclusive): first and last square.
fn reaching_squares(s: u32, lo: vec2<i32>, hi: vec2<i32>) -> vec4<i32> {
    let sp = i32(spawn_word(s, 0u));
    let r = i32(spawn_word(s, 1u));
    return vec4<i32>(div_floor(lo.x - r, sp), div_floor(lo.y - r, sp), div_floor(hi.x + r, sp), div_floor(hi.y + r, sp));
}

// Whether placement `p`'s footprint meets base columns [lo, hi] (inclusive).
fn placed_meets(p: Placed, lo: vec2<i32>, hi: vec2<i32>) -> bool {
    return p.valid && all(p.anchor.xz - p.reach <= hi) && all(p.anchor.xz + p.reach >= lo);
}

// World offset `d` (x, layer, z) from a placement's anchor in its stamp's
// frame (`Placement::local`).
fn placed_local(p: Placed, d: vec3<i32>) -> vec3<i32> {
    var x = d.x;
    var z = d.z;
    for (var t = 0u; t < p.turn; t++) {
        let x0 = x;
        x = z;
        z = -x0;
    }
    if p.mirror { x = -x; }
    return vec3<i32>(x, d.y, z);
}

// A stamp's part (`stamp::CompiledPart`, its 20 words): box or capsule
// (flat ended or round),
// material (0 air), the cells it may hold (inclusive), the segment's ends
// and radii or the box's corners (sixteenths of a cell), its edge's
// roughness, the share of its cells kept (of 256) and its seed.
struct Part {
    capsule: bool,
    flat: bool,
    material: u32,
    lo: vec3<i32>,
    hi: vec3<i32>,
    a: vec3<i32>,
    b: vec3<i32>,
    radius: vec2<i32>,
    rough: i32,
    keep: u32,
    seed: u32,
    lump: i32,
    lump_shift: u32,
}

// Part `n` of stamp `t`.
fn stamp_part(t: u32, n: u32) -> Part {
    let at = stamp_word(t, 8u) + 20u * n;
    var q: Part;
    q.capsule = (features[at] & 1u) != 0u;
    q.flat = (features[at] & 2u) != 0u;
    q.material = features[at] >> 8u;
    q.lo = vec3<i32>(bitcast<i32>(features[at + 1u]), bitcast<i32>(features[at + 2u]), bitcast<i32>(features[at + 3u]));
    q.hi = vec3<i32>(bitcast<i32>(features[at + 4u]), bitcast<i32>(features[at + 5u]), bitcast<i32>(features[at + 6u]));
    q.a = vec3<i32>(bitcast<i32>(features[at + 7u]), bitcast<i32>(features[at + 8u]), bitcast<i32>(features[at + 9u]));
    q.b = vec3<i32>(bitcast<i32>(features[at + 10u]), bitcast<i32>(features[at + 11u]), bitcast<i32>(features[at + 12u]));
    q.radius = vec2<i32>(i32(features[at + 13u] & 0xffffu), i32(features[at + 13u] >> 16u));
    q.rough = i32(features[at + 14u] & 0xffffu);
    q.keep = features[at + 14u] >> 16u;
    q.seed = features[at + 15u];
    q.lump = bitcast<i32>(features[at + 16u]);
    q.lump_shift = features[at + 17u];
    return q;
}

// How far along a segment of squared length `length` its closest point to
// a point lies (of 256), the point's offset dotted with the segment being
// `dot` (`stamp::along`).
fn part_along(dot: i32, length: i32) -> i32 {
    if length <= 0 { return 0; }
    let d = clamp(dot, 0, length);
    let shift = u32(max(32 - i32(countLeadingZeros(u32(length))) - 22, 0));
    return ((d >> shift) * 256) / max(length >> shift, 1);
}

// Whether part `q` holds point `p` (sixteenths of a cell): base cell
// `cell` when `base` (sparse and rough by hash), else a coarse cell's
// centre (smooth, held where the part keeps half its cells)
// (`CompiledPart::holds`).
fn part_holds(q: Part, p: vec3<i32>, base: bool, cell: vec3<i32>) -> bool {
    if any(p < q.lo * 16) || any(p >= (q.hi + 1) * 16) { return false; }
    var h = 0u;
    if base {
        h = hash3(cell.x, cell.y, cell.z, q.seed);
        if (h & 255u) >= q.keep { return false; }
    } else if q.keep < 128u {
        return false;
    }
    if !q.capsule { return all(p >= q.a) && all(p < q.b); }
    let d = q.b - q.a;
    let w = p - q.a;
    let length = dot(d, d);
    let along_d = dot(w, d);
    if q.flat && (along_d < 0 || along_d > length) { return false; }
    let t = part_along(along_d, length);
    let e = w - d * t / 256;
    var r = q.radius.x + (q.radius.y - q.radius.x) * t / 256;
    if base { r -= q.rough * i32((h >> 8u) & 255u) / 256; }
    if q.lump > 0 {
        let s = 4u + q.lump_shift;
        let l = hash3(p.x >> s, p.y >> s, p.z >> s, q.seed ^ 0x6c756d70u);
        r -= q.lump * i32(l & 255u) / 256;
    }
    return r > 0 && dot(e, e) <= r * r;
}

// A part's cell code for `feature_set`: 1 air, 2 + m material m.
fn part_code(q: Part) -> u32 {
    return select(1u, 2u + q.material, q.material != 0u);
}

// The far form over level column `(i, j)` of footprint `tile`
// (`CompiledFeatures::far_cover`: spawns not drawn at its level), whose
// height is `height` (its top cell `top - 1` at domain point `p`, surface
// word `surface`): its spawn + 1 (0: none). `tile_columns_main` keeps it
// over the column's surface word (bits 8 to 13) for the chunks over it
// (`column_far`).
fn far_spawn(tile: TileJob, i: i32, j: i32, p: vec3<i32>, top: i32, height: i32, surface: u32) -> u32 {
    let level = tile.level;
    if level == 0u { return 0u; }
    let ground = div_floor(height, world.grid.y);
    var material = NONE;
    // The middle base column's domain point, which every spawn's fields
    // read (once a column).
    let mid = 1 << (level - 1u);
    let base = domain_point(tile.face, (i << level) + mid, (j << level) + mid, 0u);
    for (var s = 0u; s < features[0]; s++) {
        let far = spawn_word(s, 14u);
        if feature_drawn(level, s) || far == 0u || !spawn_grows_at(s, base, ground) { continue; }
        // The column holds canopy with the spawn's coverage for odds.
        if (hash3(i, j, i32((level << 3u) | tile.face), spawn_word(s, 5u) ^ 0xfa7f0e55u) >> 16u) >= spawn_word(s, 18u) { continue; }
        if (spawn_word(s, 6u) | spawn_word(s, 7u)) != 0u {
            if material == NONE {
                let q = volume_point_half(p, ((top - 1) << (level + 1u)) + (1 << level));
                material = ground_material(q, surface, height, 0, 0, (top - 1) << level) & M_ID;
            }
            if !spawn_surface(s, material) { continue; }
        }
        let shift = spawn_word(s, 3u);
        if shift > 0u {
            let threshold = bitcast<i32>(spawn_word(s, 4u));
            var grows = threshold < 0;
            if level < spawn_word(s, 16u) {
                grows = noise(base, shift, spawn_word(s, 5u) ^ 0x632be5abu) > threshold;
            }
            if !grows { continue; }
        }
        return s + 1u;
    }
    return 0u;
}

// The far form over a footprint's column (`tile_columns` word `column`):
// its material (0: none) and canopy layers [y, z).
fn column_far(column: vec4<i32>) -> vec3<i32> {
    let s = (u32(column.y) >> 8u) & 63u;
    if s == 0u { return vec3<i32>(0); }
    let ground = div_floor(column.x, world.grid.y);
    return vec3<i32>(i32(spawn_word(s - 1u, 14u)), ground, ground + i32(spawn_word(s - 1u, 15u)));
}

// Whether a far form's canopy `cover` fills half of cell `k` of `level`.
fn far_cell(level: u32, k: i32, cover: vec3<i32>) -> bool {
    let a = max(k << level, cover.y);
    let b = min((k + 1) << level, cover.z);
    return cover.x != 0 && 2 * (b - a) >= (1 << level);
}

// What features make of a chunk column's 64 cells (from layer `k0`): the
// cells they make solid and air, each cell's material (a byte, cell z in
// byte z & 3 of word z >> 2; 0 none) and the far form's material over the
// column (0: none).
struct FeatureColumn {
    solid: vec2<u32>,
    air: vec2<u32>,
    materials: array<u32, 16>,
    far: u32,
}

fn feature_set(c: ptr<function, FeatureColumn>, z: i32, v: u32) {
    let solid = v >= 2u;
    (*c).solid = set_bits((*c).solid, z, z + 1, solid);
    (*c).air = set_bits((*c).air, z, z + 1, !solid);
    let m = select(0u, v - 2u, solid);
    let w = u32(z) >> 2u;
    let shift = (u32(z) & 3u) * 8u;
    (*c).materials[w] = ((*c).materials[w] & ~(0xffu << shift)) | (m << shift);
}

// Part `q` of placement `pl` over a column (its box `clo`, `chi` in the
// stamp's frame) set in its cells from layer `k0`.
fn feature_part(c: ptr<function, FeatureColumn>, q: Part, pl: Placed, clo: vec3<i32>, chi: vec3<i32>, k0: i32, level: u32) {
    if q.lo.x > chi.x || q.hi.x < clo.x || q.lo.z > chi.z || q.hi.z < clo.z { return; }
    let code = part_code(q);
    let span = 1 << level;
    var first = max((pl.anchor.y + q.lo.y) >> level, k0);
    var last = min(((pl.anchor.y + q.hi.y) >> level) + 1, k0 + 64);
    if q.capsule && !q.flat && all(q.a == q.b) {
        // A ball (a leaf mass): the cells it may hold over the column lie
        // within its radius of its centre (`part_holds`' points: the
        // column's centre, sixteenths), a cell or two to spare.
        let h = vec2<i32>((clo.x + chi.x + 1) * 8, (clo.z + chi.z + 1) * 8) - q.a.xz;
        let room = q.radius.x * q.radius.x - h.x * h.x - h.y * h.y;
        if room < 0 { return; }
        let dy = i32(sqrt(f32(room))) + 2;
        first = max(first, ((div_floor(q.a.y - dy - 8 * span, 16) + pl.anchor.y) >> level) - 1);
        last = min(last, ((div_floor(q.a.y + dy - 8 * span, 16) + pl.anchor.y) >> level) + 2);
    }
    for (var k = first; k < last; k++) {
        let y = (k << level) - pl.anchor.y;
        var held: bool;
        if level == 0u {
            let cell = vec3<i32>(clo.x, y, clo.z);
            held = part_holds(q, cell * 16 + 8, true, cell);
        } else {
            held = part_holds(q, vec3<i32>(clo.x + chi.x + 1, 2 * y + span, clo.z + chi.z + 1) * 8, false, vec3<i32>(0));
        }
        if held { feature_set(c, k - k0, code); }
    }
}

// The near placements over a brick column (`chunk_columns_main`'s
// workgroup) and their parts, culled by the workgroup together in their
// order: a round tests 64 squares (every near spawn's squares over the
// brick column, one spawn after another), a lane each, then the parts of
// the placements it kept, flattened likewise (a placement's after the
// last's), 64 a round. Each round's lanes' bits come back as a mask every
// lane reads alike, with its kept placements' parts counted
// (`feature_round`). A round's placements and parts wait in `w_feature_*`
// while their parts are culled and drawn.
var<workgroup> w_feature_bits: array<atomic<u32>, 2>;
var<workgroup> w_feature_parts: atomic<u32>;
var<workgroup> w_feature_mask: vec4<u32>;
// A square round's placements: anchor and reach, turn | mirror << 2, stamp,
// its parts.
var<workgroup> w_feature_anchor: array<vec4<i32>, 64>;
var<workgroup> w_feature_turn: array<u32, 64>;
var<workgroup> w_feature_stamp: array<u32, 64>;
var<workgroup> w_feature_count: array<u32, 64>;
// A part round's parts: its placement's lane | its index in its stamp << 6.
var<workgroup> w_feature_part: array<u32, 64>;

// The round's lanes whose `bit` is set (bit `li`), as a uniform mask, and
// the sum of their `parts`.
fn feature_round(li: u32, bit: bool, parts: u32) -> vec3<u32> {
    if bit {
        atomicOr(&w_feature_bits[li >> 5u], 1u << (li & 31u));
        atomicAdd(&w_feature_parts, parts);
    }
    workgroupBarrier();
    if li == 0u {
        w_feature_mask = vec4<u32>(atomicLoad(&w_feature_bits[0]), atomicLoad(&w_feature_bits[1]), atomicLoad(&w_feature_parts), 0u);
        atomicStore(&w_feature_bits[0], 0u);
        atomicStore(&w_feature_bits[1], 0u);
        atomicStore(&w_feature_parts, 0u);
    }
    return workgroupUniformLoad(&w_feature_mask).xyz;
}

// The lowest bit of a nonzero 64-bit mask.
fn lowest_bit(m: vec2<u32>) -> u32 {
    return select(32u + firstTrailingBit(m.y), firstTrailingBit(m.x), m.x != 0u);
}

// A 64-bit mask without bit `b`.
fn without_bit(m: vec2<u32>, b: u32) -> vec2<u32> {
    return m & ~select(vec2<u32>(0u, 1u << (b & 31u)), vec2<u32>(1u << b, 0u), b < 32u);
}

// The placement a square round kept in lane `b`.
fn round_placement(b: u32) -> Placed {
    let anchor = w_feature_anchor[b];
    let turn = w_feature_turn[b];
    return Placed(true, false, w_feature_stamp[b], anchor.xyz, turn & 3u, turn >= 4u, anchor.w);
}

// Level column `(i, j)` of footprint `tile` from layer `k0` (its
// `tile_columns` word `column`): the far forms' canopies, the near spawns'
// stamps over them (a base cell the last placement holding it; a coarse
// cell their sums). Every lane of the workgroup calls it (lane `li`, brick
// column `brick`'s first column); a lane not `inside` the face evaluates
// nothing.
fn feature_column(tile: TileJob, brick: vec2<i32>, li: u32, inside: bool, i: i32, j: i32, k0: i32, column: vec4<i32>) -> FeatureColumn {
    var c: FeatureColumn;
    let level = tile.level;
    if inside {
        let cover = column_far(column);
        c.far = u32(cover.x);
        if cover.x != 0 {
            for (var k = max(cover.y >> level, k0); k < min(((cover.z - 1) >> level) + 1, k0 + 64); k++) {
                if far_cell(level, k, cover) { feature_set(&c, k - k0, 2u + u32(cover.x)); }
            }
        }
    }
    let lo = vec2<i32>(i, j) << vec2<u32>(level);
    let hi = lo + (1 << level) - 1;
    let brick_lo = brick << vec2<u32>(level);
    let brick_hi = brick_lo + (8 << level) - 1;
    // Each placement over the column in order, its parts in order, each
    // over the cells before: a base cell takes the last holding its centre,
    // a coarse cell the last holding its box's centre (`stamp::cell_of`,
    // `coarse_of`). A part reads the cells its box spans. The workgroup
    // lists the placements meeting the brick column and the chunk's layers
    // and their parts meeting its box and layers, in the same order: those
    // a column meets are among them.
    let near = feature_near(level);
    var squares = 0;
    for (var s = 0u; s < near; s++) {
        if !feature_drawn(level, s) { continue; }
        let g = reaching_squares(s, brick_lo, brick_hi);
        squares += (g.z - g.x + 1) * (g.w - g.y + 1);
    }
    for (var first_square = 0; first_square < squares; first_square += 64) {
        // (Every lane is done with the last round's placements.)
        if first_square > 0 { workgroupBarrier(); }
        var n = first_square + i32(li);
        var meets = false;
        var parts = 0u;
        if n < squares {
            // Its spawn and square.
            var s = 0u;
            var g = vec4<i32>(0);
            loop {
                if feature_drawn(level, s) {
                    g = reaching_squares(s, brick_lo, brick_hi);
                    let m = (g.z - g.x + 1) * (g.w - g.y + 1);
                    if n < m { break; }
                    n -= m;
                }
                s++;
            }
            let side = g.z - g.x + 1;
            let pl = tile_placement(tile, s, g.xy + vec2<i32>(n % side, n / side));
            meets = placed_meets(pl, brick_lo, brick_hi);
            if meets {
                // Its stamp's box (its parts' union) within the chunk's
                // layers.
                let below = pl.anchor.y + bitcast<i32>(stamp_word(pl.stamp, 1u));
                let above = ((below + i32(stamp_word(pl.stamp, 5u)) - 1) >> level) + 1;
                meets = (below >> level) < k0 + 64 && above > k0;
            }
            if meets {
                parts = stamp_word(pl.stamp, 7u);
                w_feature_anchor[li] = vec4<i32>(pl.anchor, pl.reach);
                w_feature_turn[li] = pl.turn | (u32(pl.mirror) << 2u);
                w_feature_stamp[li] = pl.stamp;
                w_feature_count[li] = parts;
            }
        }
        let placed = feature_round(li, meets, parts);
        for (var first_part = 0u; first_part < placed.z; first_part += 64u) {
            // (Every lane is done with the last round's parts.)
            if first_part > 0u { workgroupBarrier(); }
            let m = first_part + li;
            var held = false;
            if m < placed.z {
                // Its placement: the kept lanes' parts one after another.
                var rest = placed.xy;
                var b = lowest_bit(rest);
                var index = m;
                while index >= w_feature_count[b] {
                    index -= w_feature_count[b];
                    rest = without_bit(rest, b);
                    b = lowest_bit(rest);
                }
                let pl = round_placement(b);
                // The brick column's box in the stamp's frame (x, z), and
                // the part's cell bounds (`stamp_part`'s lo, hi).
                let ba = placed_local(pl, vec3<i32>(brick_lo.x, 0, brick_lo.y) - pl.anchor);
                let bb = placed_local(pl, vec3<i32>(brick_hi.x, 0, brick_hi.y) - pl.anchor);
                let blo = min(ba, bb);
                let bhi = max(ba, bb);
                let at = stamp_word(pl.stamp, 8u) + 20u * index;
                let plo = vec3<i32>(bitcast<i32>(features[at + 1u]), bitcast<i32>(features[at + 2u]), bitcast<i32>(features[at + 3u]));
                let phi = vec3<i32>(bitcast<i32>(features[at + 4u]), bitcast<i32>(features[at + 5u]), bitcast<i32>(features[at + 6u]));
                let first = max((pl.anchor.y + plo.y) >> level, k0);
                let last = min(((pl.anchor.y + phi.y) >> level) + 1, k0 + 64);
                held = first < last && !(plo.x > bhi.x || phi.x < blo.x || plo.z > bhi.z || phi.z < blo.z);
                w_feature_part[li] = b | (index << 6u);
            }
            var kept = feature_round(li, held, 0u).xy;
            // The kept parts in order, each lane drawing those over its
            // column (its box in the stamp's frame, once a placement).
            var current = 64u;
            var pl: Placed;
            var mine = false;
            var clo = vec3<i32>(0);
            var chi = vec3<i32>(0);
            while (kept.x | kept.y) != 0u {
                let h = lowest_bit(kept);
                kept = without_bit(kept, h);
                let part = w_feature_part[h];
                if (part & 63u) != current {
                    current = part & 63u;
                    pl = round_placement(current);
                    mine = inside && placed_meets(pl, lo, hi);
                    let ca = placed_local(pl, vec3<i32>(lo.x, 0, lo.y) - pl.anchor);
                    let cb = placed_local(pl, vec3<i32>(hi.x, 0, hi.y) - pl.anchor);
                    clo = min(ca, cb);
                    chi = max(ca, cb);
                }
                if mine {
                    feature_part(&c, stamp_part(pl.stamp, part >> 6u), pl, clo, chi, k0, level);
                }
            }
        }
    }
    return c;
}

// A feature column's material of cell `z` (0: none).
fn feature_material(c: FeatureColumn, z: u32) -> u32 {
    var words = c.materials;
    return (words[z >> 2u] >> ((z & 3u) * 8u)) & 0xffu;
}

// A candidate block job (`chunks::candidates`): spawn `spawn`'s squares
// `block * 8` to `block * 8 + 7` each way on face `face`, into pool slot
// `slot` of `feature_blocks`.
struct BlockJob {
    face: u32,
    spawn: u32,
    block: vec2<i32>,
    slot: u32,
}

fn block_job(n: u32) -> BlockJob {
    let at = FEATURE_JOBS_AT + 4u * n;
    let block = vec2<i32>(bitcast<i32>(feature_blocks[at + 1u]), bitcast<i32>(feature_blocks[at + 2u]));
    return BlockJob(feature_blocks[at] & 7u, feature_blocks[at] >> 3u, block, feature_blocks[at + 3u]);
}

// Lane `li`'s square of block job `job` (row by row).
fn block_square(job: BlockJob, li: u32) -> vec2<i32> {
    return job.block * 8 + vec2<i32>(i32(li & 7u), i32(li >> 3u));
}

// Spawn `s`'s candidate anchor on square `g` of `face`, where its rules
// that read no terrain pass (density, the footprint on the face, the
// field): its column and the reach of its footprint's corners the relief
// rule reads (0: none); w 0 when it does not grow.
fn candidate_anchor(face: u32, s: u32, g: vec2<i32>) -> vec4<i32> {
    let a = candidate_square(face, s, g);
    if a.w == 0 || (spawn_word(s, 3u) > 0u && !candidate_field(s, domain_point(face, a.x, a.y, 0u))) { return vec4<i32>(0); }
    return a;
}

// Whether spawn `s`'s field lets a candidate grow at domain point `p` (a
// base column's).
fn candidate_field(s: u32, p: vec3<i32>) -> bool {
    let shift = spawn_word(s, 3u);
    return shift == 0u || noise(p, shift, spawn_word(s, 5u) ^ 0x632be5abu) > bitcast<i32>(spawn_word(s, 4u));
}

// `candidate_anchor` but for the field.
fn candidate_square(face: u32, s: u32, g: vec2<i32>) -> vec4<i32> {
    let f = i32(face);
    let sp = i32(spawn_word(s, 0u));
    let reach = i32(spawn_word(s, 1u));
    let inset = i32(spawn_word(s, 17u));
    let seed = spawn_word(s, 5u);
    if (hash3(g.x, g.y, f, seed) >> 16u) >= spawn_word(s, 2u) { return vec4<i32>(0); }
    // The anchor in the square's middle, its footprint on the face.
    let slack = u32(sp - 2 * inset);
    let i = g.x * sp + inset + i32(hash3(g.x, g.y, f, seed ^ 0x9e3779b9u) % slack);
    let j = g.y * sp + inset + i32(hash3(g.x, g.y, f, seed ^ 0x85ebca6bu) % slack);
    let n = world.grid.z;
    if i < reach || j < reach || i >= n - reach || j >= n - reach { return vec4<i32>(0); }
    var d = 0;
    if bitcast<i32>(spawn_word(s, 10u)) < NO_RELIEF {
        let stamp = features[spawn_word(s, 12u) + hash3(g.x, g.y, f, seed ^ 0xc2b2ae35u) % spawn_word(s, 13u)];
        d = max(i32(stamp_word(stamp, 3u)), 1);
    }
    return vec4<i32>(i, j, d, 1);
}

// The column a candidate's ground read `c` is (0 its anchor, 1-4 its
// footprint's corners).
fn candidate_read(a: vec4<i32>, c: u32) -> vec2<i32> {
    if c == 0u { return a.xy; }
    return a.xy + vec2<i32>(select(-a.z, a.z, ((c - 1u) & 1u) != 0u), select(-a.z, a.z, c >= 3u));
}

// The terrain's level-0 column `(i, j)` (height, surface word): read `r`
// of block job `job` (the node quad its nodes are in).
fn feature_ground(job: u32, r: u32, face: u32, i: i32, j: i32) -> vec2<i32> {
    let quad = job * BLOCK_READS + feature_blocks[FEATURE_READS_AT + job * BLOCK_READS + r];
    let first = vec2<i32>(i, j) & vec2<i32>(~(NODE_COLUMNS - 1));
    return generation_grid_column(FEATURE_LATTICE_AT + 4u * quad, 2u, first, face, i, j, 0u, false);
}

// Spawn `s`'s raw candidate on square `g` of `face` (`CompiledFeatures::
// candidate`): its word pair, or none. Heights are the terrain's at base
// columns, from block job `job`'s node quads (its square's reads from
// `reads`).
fn feature_candidate(job: u32, reads: u32, face: u32, s: u32, g: vec2<i32>) -> vec2<u32> {
    let none = vec2<u32>(0u);
    let f = i32(face);
    let sp = i32(spawn_word(s, 0u));
    let seed = spawn_word(s, 5u);
    let anchor = candidate_square(face, s, g);
    if anchor.w == 0 { return none; }
    let i = anchor.x;
    let j = anchor.y;
    // Its column's domain point, which its field, heights and rules read.
    let p = domain_point(face, i, j, 0u);
    if !candidate_field(s, p) { return none; }
    let column = feature_ground(job, reads, face, i, j);
    let ground = div_floor(column.x, world.grid.y);
    if !spawn_grows_at(s, p, ground) { return none; }
    if (spawn_word(s, 6u) | spawn_word(s, 7u)) != 0u {
        // The terrain's rules at its top cell on level ground.
        let q = volume_point_half(p, ((ground - 1) << 1u) + 1);
        if !spawn_surface(s, ground_material(q, u32(column.y) & 0xffu, column.x, 0, 0, ground - 1) & M_ID) { return none; }
    }
    let v = hash3(g.x, g.y, f, seed ^ 0xc2b2ae35u);
    let index = v % spawn_word(s, 13u);
    let max_relief = bitcast<i32>(spawn_word(s, 10u));
    if max_relief < NO_RELIEF {
        // Its anchor's and its footprint's corners' grounds.
        var lo = ground;
        var hi = ground;
        for (var c = 1u; c < 5u; c++) {
            let at = candidate_read(anchor, c);
            let h = div_floor(feature_ground(job, reads + c, face, at.x, at.y).x, world.grid.y);
            lo = min(lo, h);
            hi = max(hi, h);
        }
        if hi - lo > max_relief { return none; }
    }
    let k = ground - bitcast<i32>(spawn_word(s, 11u));
    return vec2<u32>(1u | (u32(i - g.x * sp) << 1u) | (u32(j - g.y * sp) << 11u) | (index << 21u) | (((v >> 8u) & 3u) << 29u) | (((v >> 10u) & 1u) << 31u), bitcast<u32>(k));
}

// A block job's node quads: a table of the quads its candidates' reads
// fall in (keys their quad's coordinates, low bits: a block spans far
// fewer quads; 0 free), each entry's quad in the job's list, its origin,
// and each read's entry.
const BLOCK_TABLE: u32 = 512u;
var<workgroup> w_block_keys: array<atomic<u32>, BLOCK_TABLE>;
var<workgroup> w_block_dense: array<u32, BLOCK_TABLE>;
var<workgroup> w_block_origin: array<vec2<i32>, BLOCK_TABLE>;
var<workgroup> w_block_reads: array<u32, BLOCK_READS>;
var<workgroup> w_block_count: atomic<u32>;

// The table entry of the node quad column `(i, j)` blends, kept.
fn block_need(i: i32, j: i32) -> u32 {
    let q = vec2<i32>(i, j) & vec2<i32>(~(NODE_COLUMNS - 1));
    let key = 0x80000000u | ((u32(q.y >> 4u) & 0x7fffu) << 16u) | (u32(q.x >> 4u) & 0xffffu);
    var e = (key * 0x9e3779b1u) >> 23u;
    for (var n = 0u; n < BLOCK_TABLE; n++) {
        let held = atomicCompareExchangeWeak(&w_block_keys[e], 0u, key);
        if held.exchanged {
            w_block_origin[e] = q;
            return e;
        }
        if held.old_value == key { return e; }
        // A spurious failure retries the entry; another key moves on.
        if held.old_value != 0u { e = (e + 1u) % BLOCK_TABLE; }
    }
    return e;
}

// Block jobs' node quads (a workgroup a job): the quads its squares'
// candidates read (anchors whose rules that read no terrain pass, their
// footprints' corners), listed once each, and each read's quad.
@compute @workgroup_size(64)
fn feature_block_quads_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let job = block_job(wg.x);
    for (var e = li; e < BLOCK_TABLE; e += 64u) { atomicStore(&w_block_keys[e], 0u); }
    if li == 0u { atomicStore(&w_block_count, 0u); }
    workgroupBarrier();
    let a = candidate_anchor(job.face, job.spawn, block_square(job, li));
    let reads = select(0u, select(1u, 5u, a.z > 0), a.w != 0);
    for (var c = 0u; c < reads; c++) {
        let at = candidate_read(a, c);
        w_block_reads[li * 5u + c] = block_need(at.x, at.y);
    }
    workgroupBarrier();
    for (var e = li; e < BLOCK_TABLE; e += 64u) {
        if atomicLoad(&w_block_keys[e]) != 0u {
            let d = atomicAdd(&w_block_count, 1u);
            w_block_dense[e] = d;
            let at = FEATURE_QUADS_AT + 2u * (wg.x * BLOCK_READS + d);
            feature_blocks[at] = bitcast<u32>(w_block_origin[e].x);
            feature_blocks[at + 1u] = bitcast<u32>(w_block_origin[e].y);
        }
    }
    workgroupBarrier();
    for (var c = 0u; c < reads; c++) {
        feature_blocks[FEATURE_READS_AT + wg.x * BLOCK_READS + li * 5u + c] = w_block_dense[w_block_reads[li * 5u + c]];
    }
    if li == 0u { feature_blocks[FEATURE_COUNTS_AT + wg.x] = atomicLoad(&w_block_count); }
}

// The block jobs' node quads' nodes (`wg.y` a job; a lane a node), each
// quad's four at node slots from `FEATURE_LATTICE_AT + 4 q`.
@compute @workgroup_size(64)
fn feature_block_nodes_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let n = wg.x * 64u + li;
    if n >= 4u * feature_blocks[FEATURE_COUNTS_AT + wg.y] { return; }
    let q = wg.y * BLOCK_READS + (n >> 2u);
    let origin = vec2<i32>(bitcast<i32>(feature_blocks[FEATURE_QUADS_AT + 2u * q]), bitcast<i32>(feature_blocks[FEATURE_QUADS_AT + 2u * q + 1u]));
    generation_grid_nodes(FEATURE_LATTICE_AT + 4u * q, 2u, block_job(wg.y).face, origin, 2u, 0u, n & 3u);
}

// The block jobs' raw candidates (a workgroup a job, a lane a square) into
// their pool slots.
@compute @workgroup_size(64)
fn feature_block_candidates_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let job = block_job(wg.x);
    let word = feature_candidate(wg.x, li * 5u, job.face, job.spawn, block_square(job, li));
    let at = job.slot * BLOCK_WORDS + 2u * li;
    feature_blocks[at] = word.x;
    feature_blocks[at + 1u] = word.y;
}

// Spawn `s`'s raw candidate word pair on square `g` of computed footprint
// `t` (one of its squares, `tile_square`): from the block holding it, as
// the footprint's block table places it (per spawn: its first block, the
// blocks a row, where its slots start).
fn tile_block_word(t: u32, s: u32, g: vec2<i32>) -> vec2<u32> {
    let at = FEATURE_TILES_AT + 4u * (t * MAX_SPAWNS + s);
    let b = (g >> vec2<u32>(3u)) - vec2<i32>(bitcast<i32>(feature_blocks[at]), bitcast<i32>(feature_blocks[at + 1u]));
    let slot = feature_blocks[FEATURE_SLOTS_AT + feature_blocks[at + 3u] + u32(b.y) * feature_blocks[at + 2u] + u32(b.x)];
    let w = slot * BLOCK_WORDS + 2u * u32((g.y & 7) * 8 + (g.x & 7));
    return vec2<u32>(feature_blocks[w], feature_blocks[w + 1u]);
}

// Spawn `s`'s raw candidate on square `g` for computed footprint `t`
// (none past its squares).
fn tile_block_candidate(t: u32, job: TileJob, s: u32, g: vec2<i32>) -> Placed {
    if tile_square(job, s, g) < 0 { return Placed(); }
    return placed_of(s, g, tile_block_word(t, s, g));
}

// Each computed footprint's near spawns' candidates (`wg.y` a footprint, a
// lane a square) into its pool slot: their raw candidates from the blocks,
// excluded where it reaches the footprint and an earlier spawn's raw
// candidate meets its footprint, else placed (the footprint's ranges
// raised over it). (`tile_columns_main` raised them over its far forms'
// canopies.)
@compute @workgroup_size(64)
fn tile_features_main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let t = wg.y;
    let job = tile_jobs[t];
    let near = feature_near(job.level);
    let n = wg.x * 64u + li;
    // The square's spawn (squares go spawn by spawn).
    var s = 0u;
    var grid = vec3<i32>(0);
    for (; s < near; s++) {
        grid = feature_grid(job.level, s);
        if n < u32(grid.x + grid.y * grid.y) { break; }
    }
    if s >= near { return; }
    let local = i32(n) - grid.x;
    let sp = i32(spawn_word(s, 0u));
    let first = vec2<i32>(job.cx * 64, job.cy * 64) << vec2<u32>(job.level);
    let g = vec2<i32>(div_floor(first.x - grid.z, sp), div_floor(first.y - grid.z, sp)) + vec2<i32>(local % grid.y, local / grid.y);
    var word = tile_block_word(t, s, g);
    let p = placed_of(s, g, word);
    let last = first + (64 << job.level) - 1;
    if placed_meets(p, first, last) {
        let flo = p.anchor.xz - p.reach;
        let fhi = p.anchor.xz + p.reach;
        var excluded = false;
        for (var e = 0u; e < s && !excluded; e++) {
            let ge = reaching_squares(e, flo, fhi);
            for (var gy = ge.y; gy <= ge.w && !excluded; gy++) {
                for (var gx = ge.x; gx <= ge.z && !excluded; gx++) {
                    excluded = placed_meets(tile_block_candidate(t, job, e, vec2<i32>(gx, gy)), flo, fhi);
                }
            }
        }
        if excluded {
            word.x |= 1u << 28u;
        } else {
            let lo = p.anchor.y + bitcast<i32>(stamp_word(p.stamp, 1u));
            let hi = ((lo + i32(stamp_word(p.stamp, 5u)) - 1) >> job.level) + 1;
            // Each quarter it reaches.
            for (var q = 0u; q < 4u; q++) {
                let quarter = first + (vec2<i32>(i32(q & 1u), i32(q >> 1u)) << vec2<u32>(job.level + 5u));
                if !placed_meets(p, quarter, quarter + (32 << job.level) - 1) { continue; }
                atomicMin(&tile_ranges[t * RANGE_WORDS + q * 4u], lo >> job.level);
                atomicMax(&tile_ranges[t * RANGE_WORDS + q * 4u + 1u], hi);
                atomicMin(&tile_ranges[t * RANGE_WORDS + q * 4u + 2u], lo >> job.level);
            }
        }
    }
    tile_placements[job.slot * features[4] + n] = word;
}

// Whether features changed cells of column `(x, y)` of chunk job `n`.
fn column_featured(n: u32, x: u32, y: u32) -> bool {
    let c = y * 64u + x;
    return ((atomicLoad(&job_features[n * FEATURE_JOB_WORDS + (c >> 5u)]) >> (c & 31u)) & 1u) != 0u;
}
