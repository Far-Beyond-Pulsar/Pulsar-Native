// Chunk octree (docs/chunk-octree.md): primary visibility.
//
// 1. Seed (raster): every visible chunk draws its box; a fragment writes
//    only the box's index. Boxes do not overlap, so along a ray their
//    entries are in the order of their hits: with the depth test on box
//    front faces, each pixel keeps the nearest box its ray enters, and
//    boxes behind it never run (the fragment shader writes no depth).
//    A ray that leaves its box without a hit is seeded again, from where
//    it left (`exits`), by the next round of seed and trace.
// 2. Trace (compute): each pixel's ray, in its seed box's affine frame
//    (cells of its level), walks the chunk's 64-tree: the root's 4^3 mid
//    nodes of 16^3 cells, their 4^3 leaves of 4^3 cells, a leaf's cells.
//    An empty child is crossed in one step to its exit; a full child is a
//    hit where the ray enters it. A ray leaving its chunk without a hit
//    continues by the drawn cut (`chunk_table`, `place_at`): the drawn node
//    over the point it enters, its chunk there, rock, or air (the largest
//    slab of its ancestors' boxes their content boxes leave empty); past a
//    cube face's edge, on the face across it.
//    Output: visibility (chunk slot; cell, face), the hit's distance and a
//    plain colour.
// 3. Resolve (raster): the hits' exact depth, for meshes and later passes;
//    or, in the render graph, publish: the hits into Helio's gbuffer.
// 4. Sun shadows (compute): from each hit, a walk towards the sun.

struct DrawParams {
    view_proj: mat4x4<f32>,
    inverse_view_proj: mat4x4<f32>,
    // The last frame's view-projection, relative to this frame's eye.
    prev_view_proj: mat4x4<f32>,
    // The eye, planet-centred, as a high and a low f32 part.
    eye_hi: vec4<f32>,
    eye_lo: vec4<f32>,
    sun: vec4<f32>,  // direction to the sun (xyz), camera near plane (w)
    debug: vec4<u32>, // x: colour by level
    table: vec4<u32>, // x: chunk table slots - 1; y: base cells per face edge; z: drawn chunks; w: frame
    // The eye in noise-domain units (`grid::DOMAIN_UNIT`, planet-centred):
    // integer part, fraction.
    eye_domain: vec4<i32>,
    eye_domain_frac: vec4<f32>,
    // Up at the eye (xyz; a plane's +y) and the eye's radius (w).
    up: vec4<f32>,
    // x: the eye's squared radius less the ceiling's, over which nothing
    // is solid (a plane: the eye's height over it); z: a plane's eye height
    // over its band's bottom, under which nothing is its world's.
    ceiling: vec4<f32>,
    // The planet's radius and voxel (m), the first level walked in the
    // grid's own cells (`exact_level`), the eye's squared radius less the
    // planet's (from f64).
    planet: vec4<f32>,
    // x: the octree's coarsest level; y: nodes fading (none: no switch to
    // look past); z: the chunk an eye under the ground walks from (NONE:
    // rays start at their seed boxes); w: the volume's anchor, the drawn
    // chunk walks entering it from elsewhere are placed by
    // (`enter_volume`; NONE: none).
    cut: vec4<u32>,
    // The eye in the scene lights' coordinates (xyz); w 1 when the scene's
    // lights are bound (`local_shadow`).
    light_eye: vec4<f32>,
    // x: how far the sky rays reach (m), when GI probes light the sky's
    // larger occlusion (0: SKY_RANGE_CELLS cells); y: the sun's angular
    // radius (radians); z: the cosine of the atmosphere's, where a host's
    // atmosphere gives one (copied from its frame on the GPU; else 0).
    sky: vec4<f32>,
    // An orthographic view (`ortho_view`): its rays' direction (xyz, unit;
    // zero for the camera's perspective) and the near plane's offset along
    // it (w: a point's distance along its ray is dot(p, xyz) - w).
    ortho: vec4<f32>,
    // xy: this frame's projection jitter less the last frame's; zw: this
    // frame's (NDC).
    history: vec4<f32>,
    // The pixels' rays as points a view depth unit beyond the eye's
    // (`RayPlane`): pixel (0, 0)'s and the eye's view depth (w), a pixel's
    // step along x, along y; [1].w 1 when the camera holds still since the
    // last frame (its projections differ by their jitter alone).
    rays: array<vec4<f32>, 3>,
    // The volume's axes in the world (its rotation's columns, xyz): every
    // position, ray and normal above is in the volume's frame (eye-relative
    // in its axes); rays, the sun and the scene's lights come into it by
    // `to_local`, normals written for the world leave by `to_world`. The
    // identity for the camera's own parameters (the denoiser's).
    rotation: array<vec4<f32>, 3>,
    // x: the volume's index (its chunk keys' bits 16 and up); y: its pass
    // in this view (0: the first traced, which also writes the pixels no
    // volume hits); z: the volumes traced; w: flags (1: a volume moved since
    // the last frame, `motions`; 2: this one moved, its hits write their own
    // motion).
    volume: vec4<u32>,
    // The pass's pixels: a rectangle (x0, y0, x1, y1; x0 and y0 multiples
    // of 32) holding its volume's bounds on the screen, the whole view for
    // the first pass. Its kernels run over it alone (`rect_id`).
    rect: vec4<u32>,
}

// A chunk's frame, written when it is generated: M's columns and M^-1's
// rows (xyz), its origin planet-centred as a high and a low f32 part, and
// its occupied layers [lo, hi), slot and flags (bit 0: the frame mirrors
// x; bits 8..16: level).
struct ChunkStatic {
    m0: vec4<f32>,
    m1: vec4<f32>,
    m2: vec4<f32>,
    i0: vec4<f32>,
    i1: vec4<f32>,
    i2: vec4<f32>,
    origin_hi: vec4<f32>,
    origin_lo: vec4<f32>,
    box: vec4<f32>,
}

// A chunk as drawn: its frame relative to the eye (`chunk_frame`). Local
// cell coordinates c map to the world as O + M c; the eye is at local `o`.
struct ChunkDraw {
    m0: vec4<f32>, // M's columns (xyz) and O (w)
    m1: vec4<f32>,
    m2: vec4<f32>,
    i0: vec4<f32>, // M^-1's rows (xyz) and o (w)
    i1: vec4<f32>,
    i2: vec4<f32>,
    // As in `ChunkStatic`.
    box: vec4<f32>,
}

struct ChunkMeta {
    root_occ: vec2<u32>,
    root_full: vec2<u32>,
    z: vec2<i32>,
    leaves: u32,
    pages: u32,
    // A plane over every solid cell: z = top.z + top.x x + top.y y.
    top: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: DrawParams;
@group(0) @binding(1) var<storage, read> statics: array<ChunkStatic>;
@group(0) @binding(2) var<storage, read> chunk_meta: array<ChunkMeta>;
@group(0) @binding(3) var<storage, read> mids: array<vec4<u32>>;
@group(0) @binding(4) var<storage, read> mid_leaves: array<u32>;
@group(0) @binding(5) var<storage, read> leaves: array<vec2<u32>>;
@group(0) @binding(6) var<storage, read> leaf_materials: array<u32>;
// Each slot's leaf pages (`LEAF_PAGES`, `chunk_generate.wgsl`): a chunk's
// leaves in mid order, 64 to a page.
@group(0) @binding(14) var<storage, read> leaf_pages: array<u32>;
// Each stored leaf's ground normal (`chunk_build_main`): two slope bytes
// (x | y << 8, by angle), 0 where none.
@group(0) @binding(15) var<storage, read> leaf_normals: array<u32>;
// Per-voxel materials where edits set them (`chunk_generate.wgsl`): each
// leaf page's detail page (NONE: none), 1024 words a page, 16 a leaf by its
// place in the page, word x + 4 y its column's cells (byte z); 0: the
// leaf's pair decides.
@group(0) @binding(16) var<storage, read> detail_pages: array<u32>;
@group(0) @binding(17) var<storage, read> detail_cells: array<u32>;
@group(0) @binding(18) var<storage, read> slot_details: array<u32>;
const LEAF_PAGES: u32 = 64u;

// Leaf `index` of chunk `slot` (in its mids' order) in the leaf storage.
fn leaf_at_page(slot: u32, index: u32) -> u32 {
    return (leaf_pages[slot * LEAF_PAGES + (index >> 6u)] << 6u) | (index & 63u);
}
// Shading of the terrain's materials (`engine::material_table`).
struct MaterialGpu {
    colour: vec4<f32>,            // linear colour, roughness
    patches: array<vec4<f32>, 3>, // linear dry/middle/lush patch colours; patches[0].w 1: varies
    links: vec4<u32>,             // lip, fleck, speck host, fleck share
    light: vec4<f32>,             // transmission, emission
}
struct Materials {
    m: array<MaterialGpu, 32>,
    detail: vec4<f32>, // patch contrast, pigment contrast, edge darkening, step softness
    shape: vec4<f32>,  // surface smoothing
}
@group(0) @binding(7) var<uniform> materials: Materials;

fn palette(m: u32) -> vec4<f32> {
    return materials.m[min(m, 31u)].colour;
}
@group(0) @binding(8) var<storage, read> mid_materials: array<u32>;
// Open addressing (`drawn.rs`): the drawn nodes of the octree's cut and
// their ancestors, key (face | level << 8, cx, cy, cz) then (slot or -1,
// state, kind | content lo << 8, content hi): whether a pixel shows it
// (`choice`), what it holds and the box of its cells anything may occupy.
// An empty entry's key x is -1.
@group(0) @binding(10) var<storage, read> chunk_table: array<vec4<i32>>;
@group(0) @binding(11) var<storage, read> chunk_keys: array<vec4<i32>>;
// This frame's boxes: chunk slots, those holding the eye first.
@group(0) @binding(12) var<storage, read> draw_list: array<u32>;
// Buried nodes primary rays met: how many times, then the first ones'
// keys, for them to be generated.
struct Buried {
    count: atomic<u32>,
    keys: array<vec4<i32>>,
}
@group(0) @binding(13) var<storage, read_write> buried: Buried;

// Chunk `slot`'s frame relative to the eye.
fn chunk_frame(slot: u32) -> ChunkDraw {
    let s = statics[slot];
    let origin = (s.origin_hi.xyz - params.eye_hi.xyz) + (s.origin_lo.xyz - params.eye_lo.xyz);
    let o = -vec3<f32>(dot(s.i0.xyz, origin), dot(s.i1.xyz, origin), dot(s.i2.xyz, origin));
    return ChunkDraw(vec4<f32>(s.m0.xyz, origin.x), vec4<f32>(s.m1.xyz, origin.y), vec4<f32>(s.m2.xyz, origin.z),
        vec4<f32>(s.i0.xyz, o.x), vec4<f32>(s.i1.xyz, o.y), vec4<f32>(s.i2.xyz, o.z), s.box);
}
COUNTERS
// Steps this invocation's primary walk took (the step heatmap, debug mode
// 32): counted only by the trace pipeline of that view (`STEP_HEAT`), as
// the count costs every walk's loops registers.
override STEP_HEAT: bool = false;
var<private> heat: u32;

const NONE: u32 = 0xffffffffu;

// ---- Volumes ----
// Every placed world draws by its own passes (`DrawParams::volume`), in its
// own frame. A chunk key's x is face | level << 8 | volume << 16
// (`NodeKey::table_key`): this pass's volume's keys.
fn key_x(face: i32, level: i32) -> i32 {
    return face | (level << 8u) | i32(params.volume.x << 16u);
}

fn key_level(x: i32) -> i32 {
    return (x >> 8u) & 255;
}

// The volume of the chunk in `slot`.
fn slot_volume(slot: u32) -> u32 {
    return u32(chunk_keys[slot].x) >> 16u;
}

// Whether a hit in chunk `slot` is this pass's volume's (one volume
// traced: every hit).
fn ours(slot: u32) -> bool {
    return params.volume.z <= 1u || slot_volume(slot) == params.volume.x;
}

// Invocation `id` of a kernel of `scale` pixels a thread, offset to the
// pass's rectangle.
fn rect_id(id: vec3<u32>, scale: u32) -> vec3<u32> {
    return id + vec3<u32>(params.rect.xy / scale, 0u);
}

// Whether this pass writes pixel hit `slot` (NONE: no volume's hit, the
// first pass's).
fn pass_pixel(slot: u32) -> bool {
    if slot == NONE { return params.volume.y == 0u; }
    return ours(slot);
}

// A vector of the volume's frame in the world's axes, and back.
fn to_world(v: vec3<f32>) -> vec3<f32> {
    return params.rotation[0].xyz * v.x + params.rotation[1].xyz * v.y + params.rotation[2].xyz * v.z;
}

fn to_local(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(params.rotation[0].xyz, v), dot(params.rotation[1].xyz, v), dot(params.rotation[2].xyz, v));
}

// Each volume's motion since the last frame (`ChunkWorld::volume_motions`):
// the last frame's projection of this frame's eye-relative world points,
// by volume index; read where a volume moved (`params.volume.w` bit 0).
@group(0) @binding(19) var<storage, read> motions: array<mat4x4<f32>>;

// The last frame's projection of hit slot `slot`'s points (eye-relative,
// world axes).
fn previous_projection(slot: u32) -> mat4x4<f32> {
    if (params.volume.w & 1u) == 0u || slot == NONE { return params.prev_view_proj; }
    return motions[slot_volume(slot)];
}

fn key_hash(k: vec4<i32>) -> u32 {
    var h = bitcast<u32>(k.x) * 0x9E3779B1u;
    h = (h ^ bitcast<u32>(k.y)) * 0x85EBCA77u;
    h = (h ^ bitcast<u32>(k.z)) * 0xC2B2AE3Du;
    h = (h ^ bitcast<u32>(k.w)) * 0x27D4EB2Fu;
    return h ^ (h >> 15u);
}

// The entry of key `k`, or NONE.
fn find_entry(k: vec4<i32>) -> u32 {
    var i = key_hash(k) & params.table.x;
    for (var probe = 0u; probe < 64u; probe++) {
        let e = chunk_table[i * 2u];
        if e.x == -1 { return NONE; }
        if all(e == k) { return i; }
        i = (i + 1u) & params.table.x;
    }
    return NONE;
}

// ---- 0. Cull ----
// The drawn chunks in view, built into the seed's draw list on the GPU:
// those holding the eye first (drawn by their back faces), the others in
// buckets of distance, nearest first (early depth tests reject boxes
// behind nearer hits), and the two indirect draws.

struct Cull {
    // Boxes holding the eye, boxes in view.
    inside: atomic<u32>,
    total: atomic<u32>,
    // Per bin (0: holding the eye, then distance buckets): its boxes, then
    // the next free place in the list.
    bins: array<atomic<u32>, 65>,
    cursor: array<atomic<u32>, 65>,
}

// The drawn chunks' slots (`params.table.z` of them).
@group(1) @binding(17) var<storage, read> drawn_slots: array<u32>;
@group(1) @binding(18) var<storage, read_write> cull: Cull;
// Each drawn chunk's bin (NONE: out of view).
@group(1) @binding(19) var<storage, read_write> cull_bins: array<u32>;
@group(1) @binding(20) var<storage, read_write> draw_out: array<u32>;
// The seed's indirect draws: boxes holding the eye, then the others.
@group(1) @binding(21) var<storage, read_write> seed_args: array<u32, 8>;

// Bin of drawn chunk `id.x`: the frustum against its bounding sphere (the
// box under its tilted top), the eye (with the near plane around it) in
// its box, else its distance.
@compute @workgroup_size(64)
fn cull_bin(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.table.z { return; }
    let slot = drawn_slots[id.x];
    // Another volume's chunk: its own pass draws it.
    if !ours(slot) {
        cull_bins[id.x] = NONE;
        return;
    }
    let c = chunk_frame(slot);
    let top = chunk_meta[slot].top;
    let reach = statics[slot].m0.w;
    let grow = reach + SEED_REACH;
    let z0 = c.box.x - grow;
    let z1 = select(clamp(top.z + max(top.x, 0.0) * 64.0 + max(top.y, 0.0) * 64.0, c.box.y, 64.0) + SEED_REACH, c.box.y + grow, reach > 0.0);
    let m = mat3x3<f32>(c.m0.xyz, c.m1.xyz, c.m2.xyz);
    let centre = vec3<f32>(c.m0.w, c.m1.w, c.m2.w) + m * vec3<f32>(32.0, 32.0, (z0 + z1) * 0.5);
    let half_z = (z1 - z0) * 0.5;
    let half = 32.0 + grow;
    let radius = max(length(m * vec3<f32>(half, half, half_z)), length(m * vec3<f32>(half, -half, half_z)));
    let vp = params.view_proj;
    let r0 = vec4<f32>(vp[0].x, vp[1].x, vp[2].x, vp[3].x);
    let r1 = vec4<f32>(vp[0].y, vp[1].y, vp[2].y, vp[3].y);
    let r3 = vec4<f32>(vp[0].w, vp[1].w, vp[2].w, vp[3].w);
    let planes = array<vec4<f32>, 5>(r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3);
    var bin = NONE;
    var seen = true;
    for (var i = 0u; i < 5u; i++) {
        let p = planes[i] / length(planes[i].xyz);
        if dot(p.xyz, centre) + p.w < -radius { seen = false; }
    }
    if seen {
        let distance = ray_distance(centre);
        bin = 1u + min(63u, u32(log2(max(distance, 1.0)) * 2.0));
        let near = params.sun.w;
        if !ortho_view() && distance < radius * 1.01 + near * 4.0 {
            let o = vec3<f32>(c.i0.w, c.i1.w, c.i2.w);
            let margin = vec3<f32>(length(c.i0.xyz), length(c.i1.xyz), length(c.i2.xyz)) * near * 2.0;
            if all(o > vec3<f32>(-grow - margin.x, -grow - margin.y, z0 - margin.z)) && all(o < vec3<f32>(64.0 + grow + margin.x, 64.0 + grow + margin.y, z1 + margin.z)) { bin = 0u; }
        }
        atomicAdd(&cull.bins[bin], 1u);
    }
    cull_bins[id.x] = bin;
}

// Each bin's place in the list, and the seed's draws.
@compute @workgroup_size(1)
fn cull_offsets() {
    var at = 0u;
    for (var b = 0u; b < 65u; b++) {
        let n = atomicLoad(&cull.bins[b]);
        atomicStore(&cull.cursor[b], at);
        at += n;
    }
    let inside = atomicLoad(&cull.bins[0]);
    atomicStore(&cull.inside, inside);
    atomicStore(&cull.total, at);
    seed_args = array<u32, 8>(36u, inside, 0u, 0u, 36u, at - inside, 0u, 0u);
}

@compute @workgroup_size(64)
fn cull_scatter(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= params.table.z { return; }
    let bin = cull_bins[id.x];
    if bin == NONE { return; }
    draw_out[atomicAdd(&cull.cursor[bin], 1u)] = drawn_slots[id.x];
}

// ---- 1. Seed ----

struct VOut {
    @builtin(position) position: vec4<f32>,
    // The box's chunk slot.
    @location(0) @interpolate(flat) instance: u32,
    @location(1) world: vec3<f32>,
}

// Per pixel: 0 before the first round, then where its ray left its last
// box without a hit, or -1 once resolved (hit or no box left).
@group(1) @binding(5) var exits: texture_2d<f32>;

// Box corners (bit 0 x, bit 1 y, bit 2 z), outward faces counter-clockwise.
const CORNERS = array<u32, 36>(
    0u, 4u, 6u, 0u, 6u, 2u,
    1u, 3u, 7u, 1u, 7u, 5u,
    0u, 1u, 5u, 0u, 5u, 4u,
    2u, 6u, 7u, 2u, 7u, 3u,
    0u, 2u, 3u, 0u, 3u, 1u,
    4u, 5u, 7u, 4u, 7u, 6u,
);

// The boxes holding the eye (`Cull::inside`), first in the draw list.
@group(1) @binding(22) var<storage, read> culled: array<u32>;

// Boxes the eye is outside of, after those holding it.
@vertex
fn seed_vertex(@builtin(vertex_index) v: u32, @builtin(instance_index) n: u32) -> VOut {
    return seed_box(draw_list[culled[0] + n], v, false);
}

// Boxes holding the eye.
@vertex
fn seed_vertex_inside(@builtin(vertex_index) v: u32, @builtin(instance_index) n: u32) -> VOut {
    return seed_box(draw_list[n], v, true);
}

// Corner `v` of chunk `slot`'s box (its back faces when `inside`).
fn seed_box(slot: u32, v: u32, inside: bool) -> VOut {
    let c = chunk_frame(slot);
    let corner = CORNERS[v];
    // An exact chunk's cells reach past its frame's box by `reach` cells:
    // its box holds them, flat over them. Every box reaches `SEED_REACH`
    // further.
    let reach = statics[slot].m0.w;
    let grow = reach + SEED_REACH;
    var x = f32(corner & 1u) * (64.0 + 2.0 * grow) - grow;
    // Mirroring x maps the box onto itself and reverses its winding: a
    // left-handed frame, or a box drawn from inside (its back faces).
    let mirrored = (bitcast<u32>(c.box.w) & 1u) != 0u;
    if mirrored != inside { x = 64.0 - x; }
    // The box's top lies on its chunk's plane over the solid cells (never
    // below its bottom): raising a corner keeps the faces over the cells.
    let y = f32((corner >> 1u) & 1u) * (64.0 + 2.0 * grow) - grow;
    let top = chunk_meta[slot].top;
    let plane = select(max(c.box.x, top.z + top.x * x + top.y * y) + SEED_REACH, c.box.y + grow, reach > 0.0);
    let local = vec3<f32>(x, y, select(c.box.x - grow, plane, (corner & 4u) != 0u));
    let world = vec3<f32>(c.m0.w, c.m1.w, c.m2.w) + c.m0.xyz * local.x + c.m1.xyz * local.y + c.m2.xyz * local.z;
    var out: VOut;
    // The camera's reversed depth, near / w (w: the view depth): precise to
    // a few parts in 10^8 of the distance anywhere, so boxes 100 m apart do
    // not tie 10 km away and seed a pixel with the farther box.
    out.position = params.view_proj * vec4<f32>(world, 1.0);
    out.instance = slot;
    out.world = world;
    return out;
}

// A seed box past the pixel's mesh (`scene_depth`): nothing in it is seen.
const BEHIND_MESH: u32 = 0x80000000u;
// Rays end a thousandth past a mesh: the shading tells which of a mesh and
// a hit that close is in front (`shadow`).
const MESH_MARGIN: f32 = 1.001;

// Boxes the eye is outside of: the box's front face is the depth. Later
// rounds keep only the boxes beyond where the pixel's ray left the last;
// a later volume's pass only those nearer than an earlier volume's hit.
// A box past the mesh there is marked (`BEHIND_MESH`), not discarded: the
// nearest still wins, so a hidden pixel's box is the chunk nearest past
// its mesh (`frame_chunk`). The marking reads the depth: the raster does
// not test it (the publish's NVIDIA issue, docs/chunk-octree.md).
@fragment
fn seed_fragment(in: VOut) -> @location(0) u32 {
    let p = vec2<i32>(in.position.xy);
    let exit = textureLoad(exits, p, 0).r;
    if exit < 0.0 || (exit > 0.0 && ray_distance(in.world) <= exit) { discard; }
    if params.volume.y > 0u {
        let hit = textureLoad(hit_distance, p, 0).r;
        if hit > 0.0 && ray_distance(in.world) >= hit { discard; }
    }
    // The reversed depth: farther, smaller.
    let hidden = in.position.z * MESH_MARGIN < textureLoad(scene_depth, p, 0);
    return in.instance | select(0u, BEHIND_MESH, hidden);
}

struct SeedInside {
    @location(0) instance: u32,
    @builtin(frag_depth) depth: f32,
}

// Boxes holding the eye, drawn first by their back faces: entered at the
// eye, nearer than any other box (the reversed depth's nearest), and so
// marked (`INSIDE_SEED`).
@fragment
fn seed_fragment_inside(in: VOut) -> SeedInside {
    return SeedInside(in.instance | INSIDE_SEED, 1.0);
}

// A seed from a box drawn from inside (`seed_fragment_inside`). The cull
// takes every box within twice the near plane's distance of the eye (the
// near plane may cut its front faces), many more than hold it when the
// near plane is far (a quarter of the height over the ground: 74 m 300 m
// over the Moon's pole); their back faces all win at the nearest depth,
// over every box in front of them, and the one that won a pixel is any
// of them. Its ray starts in the node of the cut where it starts
// (`point_start`), not in the seed's: from the seed it went on from the
// box behind the ground in front, whose cells drew as chunk-sized walls
// and slabs.
const INSIDE_SEED: u32 = 0x40000000u;

// How far past its cells a chunk's raster box reaches (cells), on every
// side. The raster's seed is the nearest box face a pixel's centre falls
// on, and boxes share no vertices (each in its own frame, its top on its
// own plane): a pixel whose ray runs along a side between two tops of
// different heights, edge-on to an eye in its plane, fell between them
// and was seeded by a box behind (the Moon's far side, under its pole:
// the ray went through the ground). Overlapping boxes leave no such gap;
// the walk is not widened (its hits stay exact), and a box seeding a ray
// it does not cross passes it on (`passes_beside`). A 64th of a cell is
// past the raster's sub-pixel snapping wherever cells are a pixel or more.
const SEED_REACH: f32 = 1.0 / 64.0;

// ---- 2. Trace ----


@group(1) @binding(0) var seed: texture_2d<u32>;
@group(1) @binding(1) var out_visibility: texture_storage_2d<rg32uint, write>;
@group(1) @binding(2) var out_colour: texture_storage_2d<rgba8unorm, write>;
// Read back by a later volume's pass: only hits nearer than an earlier
// volume's replace them.
@group(1) @binding(3) var out_distance: texture_storage_2d<r32float, read_write>;
@group(1) @binding(6) var out_exits: texture_storage_2d<r32float, read_write>;

fn bit64(m: vec2<u32>, b: u32) -> bool {
    return ((select(m.x, m.y, b >= 32u) >> (b & 31u)) & 1u) != 0u;
}

fn rank64(m: vec2<u32>, b: u32) -> u32 {
    if b < 32u { return countOneBits(m.x & ((1u << b) - 1u)); }
    return countOneBits(m.x) + countOneBits(m.y & ((1u << (b - 32u)) - 1u));
}

struct Hit {
    t: f32,
    cell: vec3<i32>,
    axis: u32,
    hit: bool,
    // The hit cell's material pair (`shown_material` picks one).
    material: u32,
    // Whether the ray moved up the axis it entered by.
    positive: bool,
}

// Whether cell `c` of chunk `slot` is solid (outside the chunk: no).
fn solid_at(slot: u32, c: vec3<i32>) -> bool {
    if any(c < vec3<i32>(0)) || any(c > vec3<i32>(63)) { return false; }
    let root = chunk_meta[slot];
    let m = u32((c.x >> 4u) + ((c.y >> 4u) << 2u) + ((c.z >> 4u) << 4u));
    if !bit64(root.root_occ, m) { return false; }
    if bit64(root.root_full, m) { return true; }
    let node = mids[slot * 64u + m];
    let l = u32(((c.x >> 2u) & 3) + (((c.y >> 2u) & 3) << 2u) + (((c.z >> 2u) & 3) << 4u));
    if !bit64(node.xy, l) { return false; }
    if bit64(node.zw, l) { return true; }
    let index = mid_leaves[slot * 64u + m] + rank64(node.xy & ~node.zw, l);
    return bit64(leaves[leaf_at_page(slot, index)], u32((c.x & 3) + ((c.y & 3) << 2u) + ((c.z & 3) << 4u)));
}

// The heightfield's normal stored with cell `c`'s leaf (chunk `slot`,
// frame `f`), in the world: the exact slope where the leaf holds natural
// tops; zero where none (an edit, an overhang, a cell under the surface).
// Lit by it, cells of a pixel or less read as the ground they sample, not
// as the terraces their whole-cell tops make on a gentle slope.
fn leaf_ground(slot: u32, c: vec3<i32>, f: ChunkDraw) -> vec3<f32> {
    if any(c < vec3<i32>(0)) || any(c > vec3<i32>(63)) { return vec3<f32>(0.0); }
    let root = chunk_meta[slot];
    let m = u32((c.x >> 4u) + ((c.y >> 4u) << 2u) + ((c.z >> 4u) << 4u));
    if !bit64(root.root_occ, m) || bit64(root.root_full, m) { return vec3<f32>(0.0); }
    let node = mids[slot * 64u + m];
    let l = u32(((c.x >> 2u) & 3) + (((c.y >> 2u) & 3) << 2u) + (((c.z >> 2u) & 3) << 4u));
    if !bit64(node.xy, l) || bit64(node.zw, l) { return vec3<f32>(0.0); }
    let index = leaf_at_page(slot, mid_leaves[slot * 64u + m] + rank64(node.xy & ~node.zw, l));
    let e = (leaf_normals[index >> 1u] >> ((index & 1u) * 16u)) & 0xffffu;
    if e == 0u { return vec3<f32>(0.0); }
    let g = tan((vec2<f32>(f32(e & 0xffu), f32(e >> 8u)) - 128.0) * (3.14159265 / 252.0));
    let n = -g.x * f.i0.xyz - g.y * f.i1.xyz + f.i2.xyz;
    let length2 = dot(n, n);
    if !(length2 > 1e-12) { return vec3<f32>(0.0); }
    return n * inverseSqrt(length2);
}

// The leaf storage index of the stored leaf holding cell `c` of chunk
// `slot` (NONE: the leaf is empty, full or past the chunk).
fn stored_leaf(slot: u32, c: vec3<i32>) -> u32 {
    if any(c < vec3<i32>(0)) || any(c > vec3<i32>(63)) { return NONE; }
    let root = chunk_meta[slot];
    let m = u32((c.x >> 4u) + ((c.y >> 4u) << 2u) + ((c.z >> 4u) << 4u));
    if !bit64(root.root_occ, m) || bit64(root.root_full, m) { return NONE; }
    let node = mids[slot * 64u + m];
    let l = u32(((c.x >> 2u) & 3) + (((c.y >> 2u) & 3) << 2u) + (((c.z >> 2u) & 3) << 4u));
    if !bit64(node.xy, l) || bit64(node.zw, l) { return NONE; }
    return leaf_at_page(slot, mid_leaves[slot * 64u + m] + rank64(node.xy & ~node.zw, l));
}

// The material pair of stored leaf `index`.
fn leaf_pair(index: u32) -> u32 {
    return (leaf_materials[index >> 1u] >> ((index & 1u) * 16u)) & 0xffffu;
}

// Cell `c`'s own material where edits set it (`detail_pages`), else 0.
// Leaves holding such cells are always stored.
fn cell_detail(slot: u32, c: vec3<i32>) -> u32 {
    if slot_details[slot] == 0u { return 0u; }
    let index = stored_leaf(slot, c);
    if index == NONE { return 0u; }
    return leaf_detail(index, c);
}

// Cell `c`'s own material in stored leaf `index` (0: none).
fn leaf_detail(index: u32, c: vec3<i32>) -> u32 {
    let page = detail_pages[index >> 6u];
    if page == NONE { return 0u; }
    let word = detail_cells[page * 1024u + (index & 63u) * 16u + u32((c.x & 3) + ((c.y & 3) << 2u))];
    return (word >> (u32(c.z & 3) * 8u)) & 0xffu;
}

// A hit cell's material from its leaf's (or mid's) two (`pair`: what a
// walk's hit carries): the surface's with air above it, the subsurface's
// under other cells. A ray entering by the top face (`from_above`) came
// through that air. Resolved once a ray has hit, not in the walk: its
// lookups in the chunk over (`solid_over`) inside the walk's loop cost the
// loop registers (+45% on a mountain's primary rays).
fn shown_material(pair: u32, slot: u32, c: vec3<i32>, from_above: bool) -> u32 {
    // A Solid or Buried node's rock: its material alone.
    if (pair & ROCK_PAIR) != 0u { return pair & 0xffu; }
    let own = cell_detail(slot, c);
    if own != 0u { return own; }
    return select((pair >> 8u) & 0xffu, pair & 0xffu, from_above || !solid_over(slot, c));
}

// Whether the cell over cell `c` of chunk `slot` is solid: in the chunk;
// over its top layer, in the node over it at its level (a chunk, or rock).
// (Counting a top layer's cells as under air drew the surface's material
// along every chunk layer a dug wall crosses.)
fn solid_over(slot: u32, c: vec3<i32>) -> bool {
    if c.z < 63 { return solid_at(slot, c + vec3<i32>(0, 0, 1)); }
    let key = chunk_keys[slot];
    return solid_in(find_entry(vec4<i32>(key.xyz, key.w + 1)), vec3<i32>(c.xy, 0));
}

// Whether cell `c` of the node of table entry `e` (NONE: none at that
// level) is solid: its chunk's cell, or its whole box when it is rock.
fn solid_in(e: u32, c: vec3<i32>) -> bool {
    if e == NONE { return false; }
    let v = chunk_table[e * 2u + 1u];
    let kind = entry_kind(v);
    if kind == KIND_SOLID || kind == KIND_BURIED { return true; }
    return v.x >= 0 && solid_at(bitcast<u32>(v.x), c);
}

// The first solid cell of chunk `slot` along o + t d from t0 to t1, entering
// through face `axis`; cells [0, 64) across, layers z.
fn trace_chunk(slot: u32, o: vec3<f32>, d: vec3<f32>, t0: f32, t1: f32, axis0: u32, z: vec2<i32>, counted: bool) -> Hit {
    let root = chunk_meta[slot];
    let inv = 1.0 / select(d, vec3<f32>(1e-30), d == vec3<f32>(0.0));
    let o_inv = o * inv;
    let positive = d > vec3<f32>(0.0);
    var t = t0;
    var axis = axis0;
    var c = clamp(vec3<i32>(floor(o + d * t)), vec3<i32>(0, 0, z.x), vec3<i32>(63, 63, z.y - 1));
    // The mid node and leaf last read: steps inside them read nothing.
    var node_at = NONE;
    var node = vec4<u32>(0u);
    var first_leaf = 0u;
    var leaf_at = NONE;
    var stored = 0u;
    var leaf = vec2<u32>(0u);
    for (var n = 0u; n < 512u; n++) {
        WCOUNT(1)
        let m = u32((c.x >> 4u) + ((c.y >> 4u) << 2u) + ((c.z >> 4u) << 4u));
        var size = 16;
        if bit64(root.root_occ, m) {
            if bit64(root.root_full, m) { return Hit(t, c, axis, true, mid_materials[slot * 64u + m] & 0xffffu, positive[axis]); }
            if m != node_at {
                node = mids[slot * 64u + m];
                first_leaf = mid_leaves[slot * 64u + m];
                node_at = m;
            }
            let l = u32(((c.x >> 2u) & 3) + (((c.y >> 2u) & 3) << 2u) + (((c.z >> 2u) & 3) << 4u));
            size = 4;
            if bit64(node.xy, l) {
                if bit64(node.zw, l) { return Hit(t, c, axis, true, mid_materials[slot * 64u + m] & 0xffffu, positive[axis]); }
                let index = first_leaf + rank64(node.xy & ~node.zw, l);
                if index != leaf_at {
                    stored = leaf_at_page(slot, index);
                    leaf = leaves[stored];
                    leaf_at = index;
                }
                size = 1;
                if bit64(leaf, u32((c.x & 3) + ((c.y & 3) << 2u) + ((c.z & 3) << 4u))) {
                    return Hit(t, c, axis, true, (leaf_materials[stored >> 1u] >> ((stored & 1u) * 16u)) & 0xffffu, positive[axis]);
                }
            }
        }
        // Leave the empty cube of `size` cells holding c.
        let b = c & vec3<i32>(~(size - 1));
        let bound = vec3<f32>(b) + select(vec3<f32>(0.0), vec3<f32>(f32(size)), positive);
        let tt = fma(bound, inv, -o_inv);
        let tn = min(tt.x, min(tt.y, tt.z));
        if tn >= t1 { break; }
        axis = select(select(2u, 1u, tt.y <= tt.z), 0u, tt.x <= tt.y && tt.x <= tt.z);
        t = max(tn, t);
        // A cell steps to its neighbour; a cube lands where the ray leaves it.
        if size != 1 {
            c = clamp(vec3<i32>(floor(o + d * t)), b, b + vec3<i32>(size - 1));
        }
        let step = select(b - vec3<i32>(1), b + vec3<i32>(size), positive);
        if axis == 0u { c.x = step.x; } else if axis == 1u { c.y = step.y; } else { c.z = step.z; }
        if any(c < vec3<i32>(0, 0, z.x)) || any(c > vec3<i32>(63, 63, z.y - 1)) { break; }
    }
    return Hit(t1, c, axis, false, 0u, false);
}

// ---- Exact cells ----
// Chunks and air of level `params.planet.z` and coarser are walked in the
// grid's own cells, not in an affine frame: on a face, cells of constant i
// or j lie between planes through the planet's centre (equal angles), and
// layers between spheres about it, so a ray meets their sides exactly, at
// the root of a linear function of t (a plane) or a quadratic (a sphere).
// Across a chunk an affine frame strays from the sphere by 512 x cell /
// radius of a cell: half a cell at an Earth's coarsest level, whose tiles
// span 600 km (facets, cracks between tiles, haze by the facets' heights).
// Finer levels keep their frames: exact there to under a hundredth of a
// cell, and precise where planet-centred f32 positions are not.

const QUARTER_PI: f32 = 0.78539816;

// A ray planet-centred: origin p, unit direction d, dot(p, d) and
// |p|^2 - R^2 (the eye's from f64, moved to the origin).
struct ExactRay {
    p: vec3<f32>,
    d: vec3<f32>,
    b: f32,
    c: f32,
}

// The ray org + t dir (eye-relative).
fn exact_ray(org: vec3<f32>, dir: vec3<f32>) -> ExactRay {
    let e = params.up.xyz * params.up.w;
    return ExactRay(e + org, dir, dot(e, dir) + dot(org, dir), params.planet.w + 2.0 * dot(e, org) + dot(org, org));
}

// Whether boxes of `level` are walked in the grid's own cells.
fn exact_level(level: i32) -> bool {
    return !PLANE_WORLD && level >= i32(params.planet.z);
}

// The angle a cell of `level` spans across a face.
fn cell_angle(level: i32) -> f32 {
    return 1.5707964 / f32(params.table.y) * f32(1u << u32(level));
}

// A ray's first stretch in a box of cells from a time on: [t0, t1], and
// the sides it enters and leaves by (2 x axis, + 1 on the high side; 6:
// none, or out of the face's side of the centre).
struct Span {
    t0: f32,
    t1: f32,
    enter: u32,
    leave: u32,
}

// `s` where g0 + t g1 >= 0 (side `code`).
fn half_line(s: Span, g0: f32, g1: f32, code: u32) -> Span {
    var o = s;
    if g1 > 0.0 {
        let t = -g0 / g1;
        if t > o.t0 {
            o.t0 = t;
            o.enter = code;
        }
    } else if g1 < 0.0 {
        let t = -g0 / g1;
        if t < o.t1 {
            o.t1 = t;
            o.leave = code;
        }
    } else if g0 < 0.0 {
        o.t1 = o.t0 - 1.0;
    }
    return o;
}

// Where the ray meets the sphere of layer bound `k` of `level` (radius R
// + k cells): the roots of t^2 + 2 b t + c, (smaller, larger), or none
// (x > y: missed, or no sphere).
fn shell_roots(ray: ExactRay, level: i32, k: i32) -> vec2<f32> {
    let h = f32(k) * params.planet.y * f32(1u << u32(level));
    if params.planet.x + h <= 0.0 { return vec2<f32>(1.0, -1.0); }
    let c = ray.c - h * (2.0 * params.planet.x + h);
    let disc = ray.b * ray.b - c;
    if disc < 0.0 { return vec2<f32>(1.0, -1.0); }
    // Stable: q and c / q.
    let q = -ray.b - select(-1.0, 1.0, ray.b >= 0.0) * sqrt(disc);
    if q == 0.0 { return vec2<f32>(0.0); }
    return vec2<f32>(min(q, c / q), max(q, c / q));
}

// The ray's first stretch from `after` in columns [lo, hi) of `level` on
// `face` (the face's indices), at any height (`box_shells` bounds it in
// layers): a chunk's occupied layers and its whole box share their sides.
fn box_sides(ray: ExactRay, face: i32, level: i32, lo: vec2<i32>, hi: vec2<i32>, after: f32) -> Span {
    var s = Span(after, 3.0e38, 6u, 6u);
    let n = vec3<f32>(face_n(face));
    let np = dot(n, ray.p);
    let nd = dot(n, ray.d);
    // On the face's side of the centre (its cells' angles hold there).
    s = half_line(s, np, nd, 6u);
    let delta = cell_angle(level);
    for (var axis = 0; axis < 2; axis++) {
        let a = vec3<f32>(select(face_b(face), face_a(face), axis == 0));
        let ap = dot(a, ray.p);
        let ad = dot(a, ray.d);
        let tl = tan(f32(lo[axis]) * delta - QUARTER_PI);
        let th = tan(f32(hi[axis]) * delta - QUARTER_PI);
        // Index >= lo: ap - tl np >= 0; < hi: th np - ap > 0.
        s = half_line(s, ap - tl * np, ad - tl * nd, u32(2 * axis));
        s = half_line(s, th * np - ap, th * nd - ad, u32(2 * axis + 1));
    }
    return s;
}

// The ray's first stretch in cells [lo, hi) of `level` on `face`: `sides`
// (`box_sides` of their columns) between layers lo_z and hi_z.
fn box_shells(sides: Span, ray: ExactRay, level: i32, lo_z: i32, hi_z: i32) -> Span {
    var s = sides;
    // Under the high sphere.
    let outer = shell_roots(ray, level, hi_z);
    if outer.x > outer.y {
        s.t1 = s.t0 - 1.0;
        return s;
    }
    if outer.x > s.t0 {
        s.t0 = outer.x;
        s.enter = 5u;
    }
    if outer.y < s.t1 {
        s.t1 = outer.y;
        s.leave = 5u;
    }
    // Over the low one: the first stretch out of it.
    let inner = shell_roots(ray, level, lo_z);
    if inner.x <= inner.y && inner.y > s.t0 && inner.x < s.t1 {
        if inner.x > s.t0 {
            s.t1 = inner.x;
            s.leave = 4u;
        } else {
            s.t0 = inner.y;
            s.enter = 4u;
        }
    }
    return s;
}

// When and by which side a ray inside cells [lo, hi) of `level` on `face`
// at time `t` leaves them (`box_shells`' t1 and leave): only the sides
// ahead of it. Along a ray a face's index moves one way (u = a.p / n.p has
// a derivative of constant sign); its height falls until the ray's
// closest approach, then rises.
fn box_exit(ray: ExactRay, face: i32, level: i32, lo: vec3<i32>, hi: vec3<i32>, t: f32) -> Span {
    var s = Span(t, 3.0e38, 6u, 6u);
    let n = vec3<f32>(face_n(face));
    let np = dot(n, ray.p);
    let nd = dot(n, ray.d);
    s = half_line(s, np, nd, 6u);
    let delta = cell_angle(level);
    for (var axis = 0; axis < 2; axis++) {
        let a = vec3<f32>(select(face_b(face), face_a(face), axis == 0));
        let ap = dot(a, ray.p);
        let ad = dot(a, ray.d);
        if ad * np - ap * nd > 0.0 {
            let th = tan(f32(hi[axis]) * delta - QUARTER_PI);
            s = half_line(s, th * np - ap, th * nd - ad, u32(2 * axis + 1));
        } else {
            let tl = tan(f32(lo[axis]) * delta - QUARTER_PI);
            s = half_line(s, ap - tl * np, ad - tl * nd, u32(2 * axis));
        }
    }
    if ray.b + t < 0.0 {
        let inner = shell_roots(ray, level, lo.z);
        if inner.x <= inner.y && inner.x > t && inner.x < s.t1 {
            s.t1 = inner.x;
            s.leave = 4u;
        }
    }
    let outer = shell_roots(ray, level, hi.z);
    if outer.x <= outer.y && outer.y < s.t1 {
        s.t1 = outer.y;
        s.leave = 5u;
    }
    return s;
}

// The ray's cell coordinates of `level` on `face` (continuous) at time t.
fn exact_cell(ray: ExactRay, face: i32, level: i32, t: f32) -> vec3<f32> {
    let q = ray.p + ray.d * t;
    let n = dot(vec3<f32>(face_n(face)), q);
    let delta = cell_angle(level);
    let i = (atan2(dot(vec3<f32>(face_a(face)), q), n) + QUARTER_PI) / delta;
    let j = (atan2(dot(vec3<f32>(face_b(face)), q), n) + QUARTER_PI) / delta;
    // Height over the datum from |q|^2 - R^2 (precise near it).
    let e = ray.c + t * (2.0 * ray.b + t);
    let k = e / (length(q) + params.planet.x) / (params.planet.y * f32(1u << u32(level)));
    return vec3<f32>(i, j, k);
}

// `trace_chunk` in the grid's own cells: chunk `slot`, cells `origin` +
// [0, 64) of `level` on `face`, from t0 to t1, entering through face
// `axis0` (up it: `positive0`).
fn trace_exact(slot: u32, ray: ExactRay, face: i32, level: i32, origin: vec3<i32>, t0: f32, t1: f32, axis0: u32, positive0: bool, z: vec2<i32>, counted: bool) -> Hit {
    let root = chunk_meta[slot];
    var t = t0;
    var axis = axis0;
    var positive = positive0;
    var c = clamp(vec3<i32>(floor(exact_cell(ray, face, level, t))) - origin, vec3<i32>(0, 0, z.x), vec3<i32>(63, 63, z.y - 1));
    var node_at = NONE;
    var node = vec4<u32>(0u);
    var first_leaf = 0u;
    var leaf_at = NONE;
    var stored = 0u;
    var leaf = vec2<u32>(0u);
    for (var n = 0u; n < 512u; n++) {
        WCOUNT(1)
        let m = u32((c.x >> 4u) + ((c.y >> 4u) << 2u) + ((c.z >> 4u) << 4u));
        var size = 16;
        if bit64(root.root_occ, m) {
            if bit64(root.root_full, m) { return Hit(t, c, axis, true, mid_materials[slot * 64u + m] & 0xffffu, positive); }
            if m != node_at {
                node = mids[slot * 64u + m];
                first_leaf = mid_leaves[slot * 64u + m];
                node_at = m;
            }
            let l = u32(((c.x >> 2u) & 3) + (((c.y >> 2u) & 3) << 2u) + (((c.z >> 2u) & 3) << 4u));
            size = 4;
            if bit64(node.xy, l) {
                if bit64(node.zw, l) { return Hit(t, c, axis, true, mid_materials[slot * 64u + m] & 0xffffu, positive); }
                let index = first_leaf + rank64(node.xy & ~node.zw, l);
                if index != leaf_at {
                    stored = leaf_at_page(slot, index);
                    leaf = leaves[stored];
                    leaf_at = index;
                }
                size = 1;
                if bit64(leaf, u32((c.x & 3) + ((c.y & 3) << 2u) + ((c.z & 3) << 4u))) {
                    return Hit(t, c, axis, true, (leaf_materials[stored >> 1u] >> ((stored & 1u) * 16u)) & 0xffffu, positive);
                }
            }
        }
        // Leave the empty cube of `size` cells holding c.
        let b = c & vec3<i32>(~(size - 1));
        let s = box_exit(ray, face, level, origin + b, origin + b + vec3<i32>(size), t);
        if s.t1 >= t1 || s.leave >= 6u { break; }
        axis = s.leave >> 1u;
        positive = (s.leave & 1u) == 1u;
        t = max(s.t1, t);
        if size != 1 {
            c = clamp(vec3<i32>(floor(exact_cell(ray, face, level, t))) - origin, b, b + vec3<i32>(size - 1));
        }
        let step = select(b - vec3<i32>(1), b + vec3<i32>(size), vec3<bool>(positive));
        if axis == 0u { c.x = step.x; } else if axis == 1u { c.y = step.y; } else { c.z = step.z; }
        if any(c < vec3<i32>(0, 0, z.x)) || any(c > vec3<i32>(63, 63, z.y - 1)) { break; }
    }
    return Hit(t1, c, axis, false, 0u, false);
}

// The frame shading reads at cell `cell` of chunk `slot`: the chunk's, or
// at an exact level the grid's own axes at the cell's centre (its
// mapping's derivatives there), eye-relative.
fn cell_frame(slot: u32, cell: vec3<i32>) -> ChunkDraw {
    let c = chunk_frame(slot);
    let key = chunk_keys[slot];
    let level = key_level(key.x);
    if !exact_level(level) { return c; }
    let face = key.x & 7;
    let g = vec3<f32>(key.yzw * 64 + cell) + 0.5;
    let delta = cell_angle(level);
    let u = tan(g.x * delta - QUARTER_PI);
    let v = tan(g.y * delta - QUARTER_PI);
    let a = vec3<f32>(face_a(face));
    let b = vec3<f32>(face_b(face));
    let w = vec3<f32>(face_n(face)) + a * u + b * v;
    let len = length(w);
    let dir = w / len;
    let size = params.planet.y * f32(1u << u32(level));
    let radius = params.planet.x + g.z * size;
    let mx = (a - dir * dot(dir, a)) * (radius * (1.0 + u * u) * delta / len);
    let my = (b - dir * dot(dir, b)) * (radius * (1.0 + v * v) * delta / len);
    let mz = dir * size;
    let origin = dir * radius - params.up.xyz * params.up.w - (mx * (f32(cell.x) + 0.5) + my * (f32(cell.y) + 0.5) + mz * (f32(cell.z) + 0.5));
    // M^-1's rows: cross products over the determinant.
    let det = dot(mx, cross(my, mz));
    let i0 = cross(my, mz) / det;
    let i1 = cross(mz, mx) / det;
    let i2 = cross(mx, my) / det;
    return ChunkDraw(vec4<f32>(mx, origin.x), vec4<f32>(my, origin.y), vec4<f32>(mz, origin.z), vec4<f32>(i0, -dot(i0, origin)), vec4<f32>(i1, -dot(i1, origin)), vec4<f32>(i2, -dot(i2, origin)), c.box);
}

// The view ray through pixel `p` (unit length): from the eye, or along an
// orthographic view's axis.
fn pixel_ray(p: vec2<u32>, size: vec2<u32>) -> vec3<f32> {
    if ortho_view() { return params.ortho.xyz; }
    let ndc = vec2<f32>((f32(p.x) + 0.5) / f32(size.x) * 2.0 - 1.0, 1.0 - (f32(p.y) + 0.5) / f32(size.y) * 2.0);
    let far = params.inverse_view_proj * vec4<f32>(ndc, 0.5, 1.0);
    return normalize(far.xyz / far.w);
}

// Whether this draw is an orthographic view (the sun's: `ortho_view`
// views of `ChunkWorld::draw_sun_occlusion`), not the camera's.
fn ortho_view() -> bool {
    return params.ortho.x != 0.0 || params.ortho.y != 0.0 || params.ortho.z != 0.0;
}

// Where pixel `p`'s ray starts (eye-relative): the eye, or the orthographic
// view's near plane.
fn pixel_origin(p: vec2<u32>, size: vec2<u32>) -> vec3<f32> {
    if !ortho_view() { return vec3<f32>(0.0); }
    let ndc = vec2<f32>((f32(p.x) + 0.5) / f32(size.x) * 2.0 - 1.0, 1.0 - (f32(p.y) + 0.5) / f32(size.y) * 2.0);
    let near = params.inverse_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    return near.xyz / near.w;
}

// Eye-relative point `world`'s distance along its view ray.
fn ray_distance(world: vec3<f32>) -> f32 {
    if ortho_view() { return dot(world, params.ortho.xyz) - params.ortho.w; }
    return length(world);
}

// What a pixel with dither `r` does at a tile of `state` (`drawn::choice`):
// 1 shows it, 0 looks finer (an ancestor, or the coarse side of a switch
// between levels this pixel does not show), 2 coarser (the fine side).
// A fading tile's state holds its first frame: the fade's progress q (of
// 1024) comes from this frame's.
fn choice(state: i32, r: i32) -> i32 {
    if state < 2 { return state; }
    let f = state - 2;
    let elapsed = (params.table.w - u32(f >> 2u)) & 0xfffffu;
    let q = i32(min(elapsed * 1024u / FADE_FRAMESu, 1024u));
    if (r < q) == ((f & 1) != 0) { return 1; }
    return select(2, 0, (f & 2) != 0);
}

// A pixel's dither in [0, 1024) for switches between levels: interleaved
// gradient noise moved each frame, so TSR blends the two levels.
fn fade_dither(p: vec2<u32>) -> i32 {
    let shift = 5.588238 * f32(params.table.w % 64u);
    let q = vec2<f32>(p) + shift;
    return i32(fract(52.9829189 * fract(0.06711056 * q.x + 0.00583715 * q.y)) * 1024.0);
}

// Cell `cell` of `level` (at `frac` within it) as a cell of `level + diff`:
// coarser for diff > 0, finer for diff < 0.
fn cell_at(cell: vec3<i32>, frac: vec3<f32>, diff: i32) -> vec3<i32> {
    if diff >= 0 { return cell >> vec3<u32>(u32(diff)); }
    let n = u32(-diff);
    return (cell << vec3<u32>(n)) + vec3<i32>(floor(frac * f32(1u << n)));
}

// What a ray meets at a cell: a drawn chunk (`slot`), or air to cross,
// cells [lo, hi) of `level` (LOST: off the drawn cut; ROCK: a Solid node,
// a hit where the ray enters; BURIED: a Buried node, the same, its key in
// lo and hi.x; both with their material in hi.y), in the frame of chunk
// `frame` when it is finer than the ray's (else NONE).
struct Place {
    slot: u32,
    level: i32,
    lo: vec3<i32>,
    hi: vec3<i32>,
    frame: u32,
}

const LOST: i32 = 99;
const BURIED: i32 = 98;
const ROCK: i32 = 97;
// A walk's hit pair (`WalkEnd::material`) of a Solid or Buried node: its
// material in the low byte, no cell of a chunk to look up. (Chunk hits'
// pairs are masked to 16 bits: a leaf's word holds its neighbour's too.)
const ROCK_PAIR: u32 = 0x10000u;

// The hit pair of uniform node `place` (ROCK or BURIED).
fn rock_pair(place: Place) -> u32 {
    return ROCK_PAIR | (u32(place.hi.y) & 0xffu);
}

// A table entry's kind (`drawn::KIND_*`) and content box ([lo, hi) in its
// node's cells; nothing when lo > hi).
const KIND_MIXED: i32 = 0;
const KIND_SOLID: i32 = 1;
const KIND_BURIED: i32 = 2;
// A bit of the kind word: a strict ancestor of the node is drawn
// (`drawn::UNDER_DRAWN`).
const UNDER_DRAWN: i32 = 16;

fn entry_kind(v: vec4<i32>) -> i32 {
    return v.z & 15;
}

fn unpack_corner(p: i32) -> vec3<i32> {
    return vec3<i32>(p & 127, (p >> 7u) & 127, (p >> 14u) & 127);
}

// Cube faces' bases: normal, a, b (`grid::FACE_BASIS`). Computed, not
// looked up: naga copies a constant array indexed by a variable into a
// local one at every lookup (the trace's 576 bytes of local memory and a
// tenth of its instructions).
fn face_n(face: i32) -> vec3<i32> {
    let s = 1 - 2 * (face & 1);
    return vec3<i32>(select(0, s, face < 2), select(0, s, (face >> 1u) == 1), select(0, s, face >= 4));
}
fn face_a(face: i32) -> vec3<i32> {
    return vec3<i32>(select(select(1, -1, face == 5), 0, face < 2), 0, select(0, 2 * face - 1, face < 2));
}
fn face_b(face: i32) -> vec3<i32> {
    let side = (face >> 1u) == 1;
    return vec3<i32>(0, select(1, 0, side), select(0, 2 * face - 5, side));
}

fn face_with_normal(n: vec3<i32>) -> i32 {
    if n.x != 0 { return select(1, 0, n.x > 0); }
    if n.y != 0 { return select(3, 2, n.y > 0); }
    return select(5, 4, n.z > 0);
}

// A cell past an edge of `face` (one tangential index outside [0, n), n
// cells to an edge at its level) as a cell of the face across it, and
// where in it. The faces are equal-angle: across the edge an index runs on
// mirrored, and along it cells meet exactly on the edge.
struct Across {
    face: i32,
    cell: vec3<i32>,
    frac: vec3<f32>,
    ok: bool,
}

fn across_edge(face: i32, cell: vec3<i32>, frac: vec3<f32>, n: i32) -> Across {
    let out_a = cell.x < 0 || cell.x >= n;
    let out_b = cell.y < 0 || cell.y >= n;
    // Inside, or past a corner.
    if out_a == out_b { return Across(face, cell, frac, false); }
    // The index across the edge and the one along it.
    let x = select(cell.y, cell.x, out_a);
    let fx = select(frac.y, frac.x, out_a);
    let y = select(cell.x, cell.y, out_a);
    let fy = select(frac.x, frac.y, out_a);
    let past = x >= n;
    let normal = face_n(face);
    let along = select(face_a(face), face_b(face), out_a);
    let next = face_with_normal(select(select(face_b(face), face_a(face), out_a) * -1, select(face_b(face), face_a(face), out_a), past));
    var out = Across(next, vec3<i32>(0, 0, cell.z), vec3<f32>(0.0, 0.0, frac.z), true);
    for (var t = 0; t < 2; t++) {
        let axis = select(face_b(next), face_a(next), t == 0);
        var c = 0;
        var f = 0.0;
        if all(axis == along) {
            c = y;
            f = fy;
        } else if all(axis == -along) {
            c = n - 1 - y;
            f = 1.0 - fy;
        } else if all(axis == normal) {
            c = select(n + x, 2 * n - 1 - x, past);
            f = select(fx, 1.0 - fx, past);
        } else {
            c = select(-1 - x, x - n, past);
            f = select(1.0 - fx, fx, past);
        }
        if t == 0 {
            out.cell.x = c;
            out.frac.x = f;
        } else {
            out.cell.y = c;
            out.frac.y = f;
        }
    }
    out.frac = clamp(out.frac, vec3<f32>(0.0), vec3<f32>(0.999));
    return out;
}

// No place for a ray at a cell: off the drawn cut, for reason `site` (counted
// by reason in counting builds, `TraceCounts::lost_sites`).
fn lost_at(site: u32) -> Place {
    LOST_SITE
    return Place(NONE, LOST, vec3<i32>(0), vec3<i32>(0), NONE);
}

// The drawn chunk next to node `k` of a level along its layers (the
// frame air in it is crossed in), else NONE.
fn frame_near(k: vec4<i32>) -> u32 {
    for (var n = 0; n < 4; n++) {
        // -1, 1, -2, 2 (computed: see `face_n`).
        let dz = select(1, -1, (n & 1) == 0) * (1 + (n >> 1u));
        let e = find_entry(vec4<i32>(k.xyz, k.w + dz));
        if e != NONE && chunk_table[e * 2u + 1u].x >= 0 { return bitcast<u32>(chunk_table[e * 2u + 1u].x); }
    }
    return NONE;
}

// The place at cell `cell` of `level` (at `frac` within it) on `face`, for
// a pixel of dither `r` (`docs/chunk-octree.md`): the drawn node over the
// point (up to the first entry of the cut, then finer through ancestors
// and the coarse side of switches this pixel does not show, coarser over
// their fine side), then its chunk, rock, or air. A refined node without
// an entry for the child there is air (an Empty node). Air grows to the
// largest slab of each ancestor's box its content box leaves empty, up to
// the coarsest ancestor whose content the point is outside of, and comes
// with a frame (`Place::frame`) when it is finer than the ray's, or on
// another face (`crossed`).
fn place_at(face: i32, level: i32, cell: vec3<i32>, frac: vec3<f32>, r: i32, crossed: bool) -> Place {
    let top = i32(params.cut.x);
    var diff = 0;
    var e = find_entry(vec4<i32>(key_x(face, level), cell >> vec3<u32>(6u)));
    // Mostly the next chunk of the same level.
    if e != NONE {
        let v = chunk_table[e * 2u + 1u];
        if v.x >= 0 && choice(v.y, r) == 1 { return Place(bitcast<u32>(v.x), level, vec3<i32>(0), vec3<i32>(0), NONE); }
    }
    // Up to the cut: a node drawn, or an ancestor of drawn nodes.
    for (var n = 0u; n < 32u && e == NONE; n++) {
        if level + diff >= top {
            // Over the roots' drawn nodes: air (an Empty root).
            break;
        }
        diff += 1;
        let c = cell_at(cell, frac, diff);
        e = find_entry(vec4<i32>(key_x(face, level + diff), c >> vec3<u32>(6u)));
    }
    var empty = e == NONE;
    // An ancestor found climbing: a coarser node fading in over it may show
    // here (its finer side has no node over this point), when one over it
    // is drawn (else the search would climb every ancestor to the top for
    // nothing, whenever anything fades anywhere).
    if !empty && params.cut.y > 0u && chunk_table[e * 2u + 1u].y == 0 && (chunk_table[e * 2u + 1u].z & UNDER_DRAWN) != 0 {
        var u = diff;
        for (var n = 0u; n < 32u && level + u < top; n++) {
            u += 1;
            let pe = find_entry(vec4<i32>(key_x(face, level + u), cell_at(cell, frac, u) >> vec3<u32>(6u)));
            if pe == NONE { break; }
            let state = chunk_table[pe * 2u + 1u].y;
            if state == 0 { continue; }
            if choice(state, r) == 1 {
                diff = u;
                e = pe;
            }
            break;
        }
    }
    // Up past ancestors (over the hidden fine side of a switch), down
    // through them.
    var way = 0;
    for (var n = 0u; n < 32u && !empty; n++) {
        let state = chunk_table[e * 2u + 1u].y;
        var go = choice(state, r);
        if state == 0 && way > 0 { go = 2; }
        if go == 1 { break; }
        if go == 0 {
            // Finer: a missing child is air.
            if level + diff <= 0 || way > 0 { return lost_at(0u); }
            way = -1;
            diff -= 1;
        } else {
            if level + diff >= top || way < 0 { return lost_at(1u); }
            way = 1;
            diff += 1;
        }
        let c = cell_at(cell, frac, diff);
        e = find_entry(vec4<i32>(key_x(face, level + diff), c >> vec3<u32>(6u)));
        if e == NONE {
            if way > 0 { return lost_at(2u); }
            empty = true;
        }
    }
    let at = level + diff;
    let c = cell_at(cell, frac, diff);
    let node = c >> vec3<u32>(6u);
    let key = vec4<i32>(key_x(face, at), node);
    if !empty {
        let v = chunk_table[e * 2u + 1u];
        let kind = entry_kind(v);
        let rock = (v.w >> 24u) & 0xff;
        if kind == KIND_SOLID { return Place(NONE, ROCK, key.yzw, vec3<i32>(key.x, rock, 0), NONE); }
        if kind == KIND_BURIED { return Place(NONE, BURIED, key.yzw, vec3<i32>(key.x, rock, 0), NONE); }
        if v.x >= 0 { return Place(bitcast<u32>(v.x), at, vec3<i32>(0), vec3<i32>(0), NONE); }
    }
    // Air: the node's box, grown through its ancestors' empty slabs.
    var lo = node * 64;
    var hi = lo + 64;
    var up = diff;
    for (var n = 0u; n < 32u && level + up < top; n++) {
        let pc = cell_at(cell, frac, up + 1);
        let pnode = pc >> vec3<u32>(6u);
        let pe = find_entry(vec4<i32>(key_x(face, level + up + 1), pnode));
        if pe == NONE { break; }
        let pv = chunk_table[pe * 2u + 1u];
        let clo = unpack_corner(pv.z >> 8u);
        let chi = unpack_corner(pv.w);
        let p = pc - pnode * 64;
        var slab_lo = vec3<i32>(0);
        var slab_hi = vec3<i32>(64);
        if any(clo >= chi) {
        } else if p.z >= chi.z {
            slab_lo.z = chi.z;
        } else if p.z < clo.z {
            slab_hi.z = clo.z;
        } else if p.x >= chi.x {
            slab_lo.x = chi.x;
        } else if p.x < clo.x {
            slab_hi.x = clo.x;
        } else if p.y >= chi.y {
            slab_lo.y = chi.y;
        } else if p.y < clo.y {
            slab_hi.y = clo.y;
        } else {
            // Inside its content: this level's box is the air.
            break;
        }
        up += 1;
        lo = pnode * 64 + slab_lo;
        hi = pnode * 64 + slab_hi;
    }
    // Finer than the ray's frame, or across a face: crossed in the frame of
    // a chunk beside the air's node (more than six levels finer, without
    // one, lost).
    var f = NONE;
    if up < 0 || crossed { f = frame_near(key); }
    if (crossed || up < -6) && f == NONE && !exact_level(level + up) { return lost_at(4u); }
    return Place(NONE, level + up, lo, hi, f);
}

// Box corner `hi` in the cells of chunk `key`, kept on its face: a coarse
// tile can reach past the face's edge (its columns there are empty), and a
// ray crossing them would skip what lies across the edge.
fn clip_to_face(hi: vec3<i32>, key: vec4<i32>) -> vec3<i32> {
    let edge = vec2<i32>(i32(params.table.y >> u32(key_level(key.x)))) - key.yz * 64;
    return vec3<i32>(min(hi.xy, edge), hi.z);
}

// Cell corner `a` of level `level + diff` in the cells of chunk `key` (of
// `level`): exact on tile and layer bounds for diff >= -6, else rounded up
// (`up`) or down; far coarse corners are clamped.
fn to_frame(a: vec3<i32>, diff: i32, key: vec4<i32>, up: bool) -> vec3<i32> {
    let origin = key.yzw * 64;
    if diff < 0 {
        let n = u32(-diff);
        return ((a + select(0, (1 << n) - 1, up)) >> vec3<u32>(n)) - origin;
    }
    let n = vec3<u32>(u32(diff));
    let base = origin >> n;
    let limit = 1 << u32(max(28 - diff, 0));
    return (clamp(a - base, vec3<i32>(-limit), vec3<i32>(limit)) << n) - (origin - (base << n));
}

// A walk through the drawn cut: the chunk whose frame it is in (its last)
// and its key, and the box it crosses in that frame's cells, the chunk
// itself (`real`) or air. With the key of an exact level and not `real`,
// it starts at the node of the cut where the ray starts, on that key's face
// (the slot's frame unused).
struct Walk {
    slot: u32,
    key: vec4<i32>,
    lo: vec3<i32>,
    hi: vec3<i32>,
    real: bool,
}

// Where a walk ended: a hit (chunk, cell, the face it entered by and its
// side, material), or how far it went when it left the cut, ran out of
// boxes or reached its end (and the chunk it was in last; NONE past the
// ceiling).
struct WalkEnd {
    hit: bool,
    t: f32,
    slot: u32,
    cell: vec3<i32>,
    axis: u32,
    sign: u32,
    // The hit's material pair (`Hit::material`).
    material: u32,
}

// A box's slab entry times `near` with the one on `axis` (a side the ray
// stepped into it by; 3: none) no later than `t`, where the box before
// let the ray go: the two frames place their shared side apart by rounding,
// and a ray nearly parallel to it was in neither box between the two
// (it went on under the ground, a hit on the next cell's side). The box
// takes it at once, in the border cells it is beside.
fn from_side(near: vec3<f32>, axis: u32, t: f32) -> vec3<f32> {
    return select(near, min(near, vec3<f32>(t)), vec3<u32>(axis) == vec3<u32>(0u, 1u, 2u));
}

// Where a ray goes on from a box of an affine frame it does not cross in
// that frame (o, d: the ray in its cells; `near`: the times it enters the
// box's slabs, the latest past `leave`, the earliest it leaves one): the
// cell of the box beside it that it passes in, where it has entered every
// slab it does cross (none: where it is, `t`). Each chunk's frame is its
// own f32 transform, so two chunks place their shared side apart by
// rounding (the raster's sub-pixel snapping seeds a ray within about 1/256
// of a pixel of a shared edge in either box; a step's cell can round over
// the side): a ray nearly parallel to the side is then in a box it never
// enters, for its whole length. It was walked as crossing it, to where its
// far sides end, past the ground in the box beside (rays along the plane
// through the Moon pole's eye ended on the Solid node under it). Beside a
// box, the ray is close to it, where its frame is exact.
struct Beside {
    t: f32,
    cell: vec3<i32>,
    frac: vec3<f32>,
}

fn passes_beside(o: vec3<f32>, d: vec3<f32>, near: vec3<f32>, leave: f32, t: f32) -> Beside {
    let entered = select(near, vec3<f32>(t), near >= vec3<f32>(leave));
    let at = max(t, max(entered.x, max(entered.y, entered.z)));
    let q = o + d * at;
    return Beside(at, vec3<i32>(floor(q)), clamp(fract(q), vec3<f32>(0.0), vec3<f32>(0.999)));
}

// The walk of ray org + t dir (org eye-relative, dir unit) from t_start to
// t_end, for
// a pixel of dither `r`, over at most `boxes` boxes (`counted`: in the
// counting build's counters, as `pixel`). A box of an exact level (a chunk
// or air) is crossed in the grid's own cells, a finer one in its frame.
fn walk(start: Walk, org: vec3<f32>, dir: vec3<f32>, t_start: f32, t_end: f32, r: i32, boxes: u32, counted: bool, pixel: vec2<u32>) -> WalkEnd {
    // Over the ceiling and rising, a ray meets nothing more (the squared
    // radius q0 + 2 b t + t^2 only grows once b + t > 0; a plane's height
    // q0 + b t once b > 0): it ends there, t -1.
    var q0 = params.ceiling.x + 2.0 * params.up.w * dot(params.up.xyz, org) + dot(org, org);
    var b = params.up.w * dot(params.up.xyz, dir) + dot(org, dir);
    if PLANE_WORLD {
        q0 = params.ceiling.x + org.y;
        b = dir.y;
    }
    // The frame's chunk, its frame read where it is used: kept through the
    // loop, its 28 floats cost the walk registers (spilled).
    var frame_slot = start.slot;
    var key = start.key;
    var cube_lo = start.lo;
    var cube_hi = start.hi;
    var real = start.real;
    var t_prev = t_start;
    // The box's chunk (in air, the last), and packed as in the visibility
    // buffer the cell of it the ray last left and the side it crossed into
    // the box by: what it meets under a tile's chunks or in rock is shown
    // there.
    var slot = start.slot;
    var last = (2u << 18u) | (1u << 20u);
    // The axis of the side a frame step crossed into this box by (3:
    // none). Two chunks' frames place their shared side apart by rounding:
    // the box takes the ray from where the one before left it.
    var stepped = 3u;
    // A box of an exact level: its face and level in key.x, its cells (the
    // face's) in cube_lo and cube_hi. Such a walk starts where the ray
    // enters its seed box (an exact chunk's raster box reaches past its
    // cells, over its neighbours'): the drawn cut there.
    var exact = exact_level(key_level(key.x));
    var place = exact;
    if exact && real {
        let frame = chunk_frame(start.slot);
        let o = vec3<f32>(frame.i0.w, frame.i1.w, frame.i2.w) + vec3<f32>(dot(frame.i0.xyz, org), dot(frame.i1.xyz, org), dot(frame.i2.xyz, org));
        let d = vec3<f32>(dot(frame.i0.xyz, dir), dot(frame.i1.xyz, dir), dot(frame.i2.xyz, dir));
        let inv = 1.0 / select(d, vec3<f32>(1e-30), d == vec3<f32>(0.0));
        let grow = statics[start.slot].m0.w;
        let ta = (vec3<f32>(-grow, -grow, frame.box.x - grow) - o) * inv;
        let tb = (vec3<f32>(64.0 + grow, 64.0 + grow, frame.box.y + grow) - o) * inv;
        let near = min(ta, tb);
        t_prev = max(max(near.x, max(near.y, near.z)), t_prev);
        cube_lo += key.yzw * 64;
        cube_hi += key.yzw * 64;
    }
    var chunk = 0u;
    for (; chunk < boxes; chunk++) {
        let over = select(b + t_prev > 0.0 && q0 + t_prev * (2.0 * b + t_prev) > 0.0, b > 0.0 && q0 + b * t_prev > 0.0, PLANE_WORLD);
        if over { return WalkEnd(false, -1.0, NONE, vec3<i32>(0), 0u, 0u, 0u); }
        WCOUNT(3)
        // Where the ray goes next: the face, level, cell and where in it.
        var face = key.x & 7;
        let level = key_level(key.x);
        var cell = vec3<i32>(0);
        var frac = vec3<f32>(0.0);
        if exact {
            let ray = exact_ray(org, dir);
            // A coarse tile can reach past its face's edge: its columns
            // there are empty.
            let hi = vec3<i32>(min(cube_hi.xy, vec2<i32>(i32(params.table.y >> u32(level)))), cube_hi.z);
            if place {
                place = false;
                stepped = 3u;
                let q = exact_cell(ray, face, level, t_prev);
                cell = vec3<i32>(floor(q));
                frac = clamp(fract(q), vec3<f32>(0.0), vec3<f32>(0.999));
                let c = clamp(cell - cube_lo, vec3<i32>(0), vec3<i32>(63));
                last = u32(c.x) | (u32(c.y) << 6u) | (u32(c.z) << 12u) | (2u << 18u) | (1u << 20u);
            } else {
                // The box's exit, before the chunk's walk: only it is kept
                // through the walk.
                let sides = box_sides(ray, face, level, cube_lo.xy, hi.xy, t_prev);
                let span = box_shells(sides, ray, level, cube_lo.z, hi.z);
                if real {
                    // The walk over the chunk's occupied layers, entering
                    // by their side, else by the one it crossed into the
                    // chunk by.
                    let z = vec2<i32>(statics[slot].box.xy);
                    let occupied = box_shells(sides, ray, level, cube_lo.z + z.x, cube_lo.z + z.y);
                    let t1 = min(occupied.t1, t_end);
                    if t1 > occupied.t0 {
                        let entered = occupied.enter < 6u;
                        let axis0 = select((last >> 18u) & 3u, occupied.enter >> 1u, entered);
                        let positive0 = select(((last >> 20u) & 1u) == 0u, (occupied.enter & 1u) == 0u, entered);
                        let h = trace_exact(slot, ray, face, level, cube_lo, occupied.t0, t1, axis0, positive0, z, counted);
                        if h.hit {
                            return WalkEnd(true, h.t, slot, h.cell, h.axis, select(1u, 0u, h.positive), h.material);
                        }
                    }
                }
                if span.leave >= 6u { break; }
                let t_leave = max(span.t1, t_prev);
                t_prev = t_leave;
                if t_prev >= t_end { break; }
                // The cell just past the side it leaves by.
                stepped = 3u;
                let axis = span.leave >> 1u;
                let positive = (span.leave & 1u) == 1u;
                let q = exact_cell(ray, face, level, t_leave);
                cell = clamp(vec3<i32>(floor(q)), cube_lo, hi - 1);
                var c = vec3<i32>(vec3<u32>(last, last >> 6u, last >> 12u) & vec3<u32>(63u));
                if real { c = clamp(cell - cube_lo, vec3<i32>(0), vec3<i32>(63)); }
                last = u32(c.x) | (u32(c.y) << 6u) | (u32(c.z) << 12u) | (axis << 18u) | (select(1u, 0u, positive) << 20u);
                frac = clamp(fract(q), vec3<f32>(0.0), vec3<f32>(0.999));
                let side = select(cube_lo - 1, hi, vec3<bool>(positive));
                let entry = select(0.999, 0.0, positive);
                if axis == 0u {
                    cell.x = side.x;
                    frac.x = entry;
                } else if axis == 1u {
                    cell.y = side.y;
                    frac.y = entry;
                } else {
                    cell.z = side.z;
                    frac.z = entry;
                }
            }
        } else {
            slot = frame_slot;
            let frame = chunk_frame(frame_slot);
            TRACE_PIXEL(chunk, slot, cube_lo, real)
            // The ray in the frame chunk's cells.
            let o = vec3<f32>(frame.i0.w, frame.i1.w, frame.i2.w) + vec3<f32>(dot(frame.i0.xyz, org), dot(frame.i1.xyz, org), dot(frame.i2.xyz, org));
            let d = vec3<f32>(dot(frame.i0.xyz, dir), dot(frame.i1.xyz, dir), dot(frame.i2.xyz, dir));
            let inv = 1.0 / select(d, vec3<f32>(1e-30), d == vec3<f32>(0.0));
            if real { cube_hi = clip_to_face(vec3<i32>(64), key); }
            let ca = (vec3<f32>(cube_lo) - o) * inv;
            let cb = (vec3<f32>(cube_hi) - o) * inv;
            let cube_near = from_side(min(ca, cb), stepped, t_prev);
            let cube_far = max(ca, cb);
            let t_leave = min(cube_far.x, min(cube_far.y, cube_far.z));
            // Whether the ray crosses the box in its frame (else it was put
            // in it from beside it: `passes_beside`).
            let crosses = max(cube_near.x, max(cube_near.y, cube_near.z)) < t_leave;
            if real && crosses {
                // The walk over the chunk's occupied layers.
                let ta = (vec3<f32>(0.0, 0.0, frame.box.x) - o) * inv;
                let tb = (vec3<f32>(f32(cube_hi.x), f32(cube_hi.y), frame.box.y) - o) * inv;
                // Its occupied layers reach the side crossed into it by only
                // where they reach the chunk's top or bottom.
                let layers = stepped < 2u || (stepped == 2u && select(frame.box.x <= 0.0, frame.box.y >= 64.0, d.z < 0.0));
                let lo = from_side(min(ta, tb), select(3u, stepped, layers), t_prev);
                let hi = max(ta, tb);
                var t0 = max(max(lo.x, max(lo.y, lo.z)), t_prev);
                var t1 = min(min(hi.x, min(hi.y, hi.z)), t_end);
                // Only under the chunk's plane over its solid cells.
                let top = chunk_meta[slot].top;
                let s0 = o.z - top.x * o.x - top.y * o.y - top.z;
                let ds = d.z - top.x * d.x - top.y * d.y;
                if ds > 0.0 { t1 = min(t1, -s0 / ds); } else if ds < 0.0 { t0 = max(t0, -s0 / ds); } else if s0 > 0.0 { t1 = t0; }
                if t1 > t0 {
                    let axis0 = select(select(2u, 1u, lo.y >= lo.z), 0u, lo.x >= lo.y && lo.x >= lo.z);
                    let h = trace_chunk(slot, o, d, t0, t1, axis0, vec2<i32>(i32(frame.box.x), i32(frame.box.y)), counted);
                    if h.hit {
                        return WalkEnd(true, h.t, slot, h.cell, h.axis, select(1u, 0u, h.positive), h.material);
                    }
                }
            }
            if crosses {
                // The cell just past the face the ray leaves the box by, in
                // the frame chunk's cells (exact integers far from the
                // origin), where the ray is (as an exact box's): never back
                // along it.
                t_prev = max(t_leave, t_prev);
                if t_prev >= t_end { break; }
                let q = o + d * t_prev;
                let axis = select(select(2u, 1u, cube_far.y <= cube_far.z), 0u, cube_far.x <= cube_far.y && cube_far.x <= cube_far.z);
                var local = clamp(vec3<i32>(floor(q)), cube_lo, cube_hi - 1);
                let c = clamp(local, vec3<i32>(0), vec3<i32>(63));
                last = u32(c.x) | (u32(c.y) << 6u) | (u32(c.z) << 12u) | (axis << 18u) | (select(1u, 0u, d[axis] > 0.0) << 20u);
                let step = select(cube_lo - 1, cube_hi, d > vec3<f32>(0.0));
                if axis == 0u { local.x = step.x; } else if axis == 1u { local.y = step.y; } else { local.z = step.z; }
                cell = key.yzw * 64 + local;
                stepped = axis;
                // Where in the next cell the ray enters: the side it
                // crossed on the exit axis.
                frac = clamp(fract(q), vec3<f32>(0.0), vec3<f32>(0.999));
                let entry = select(0.999, 0.0, d[axis] > 0.0);
                if axis == 0u { frac.x = entry; } else if axis == 1u { frac.y = entry; } else { frac.z = entry; }
            } else {
                let beside = passes_beside(o, d, cube_near, t_leave, t_prev);
                stepped = 3u;
                t_prev = beside.t;
                if t_prev >= t_end { break; }
                cell = key.yzw * 64 + beside.cell;
                frac = beside.frac;
            }
        }
        // Past a face's edge: the face across it (a plane's edge ends it).
        let n = i32(params.table.y >> u32(level));
        let crossed = any(cell.xy < vec2<i32>(0)) || any(cell.xy >= vec2<i32>(n));
        if crossed {
            stepped = 3u;
            if PLANE_WORLD { break; }
            let across = across_edge(face, cell, frac, n);
            if !across.ok { break; }
            face = across.face;
            cell = across.cell;
            frac = across.frac;
        }
        // Air crossed in a frame needs one on the face it crossed to.
        let next = place_at(face, level, cell, frac, r, crossed && !exact);
        if next.slot != NONE {
            slot = next.slot;
            key = chunk_keys[next.slot];
            real = true;
            exact = exact_level(key_level(key.x));
            if exact {
                cube_lo = key.yzw * 64;
                cube_hi = cube_lo + vec3<i32>(64);
            } else {
                frame_slot = next.slot;
                cube_lo = vec3<i32>(0);
                cube_hi = vec3<i32>(64);
            }
        } else {
            if next.level == LOST {
                WCOUNT(10)
                LOST_PIXEL
                break;
            }
            if next.level == BURIED || next.level == ROCK {
                // Rock, or the field's volume not generated yet (a Buried
                // node, reported so it is): a hit where the ray enters.
                ROCK_END
                if next.level == BURIED && counted {
                    let n = atomicAdd(&buried.count, 1u);
                    if n < arrayLength(&buried.keys) { buried.keys[n] = vec4<i32>(next.hi.x, next.lo); }
                }
                return WalkEnd(true, t_prev, slot, vec3<i32>(vec3<u32>(last, last >> 6u, last >> 12u) & vec3<u32>(63u)), (last >> 18u) & 3u, (last >> 20u) & 1u, rock_pair(next));
            }
            real = false;
            // Air of an exact level, or without a frame where the ray's is
            // not one (exact, or on another face): in the grid's own cells.
            if (exact_level(next.level) || (next.frame == NONE && (exact || crossed))) {
                exact = true;
                key = vec4<i32>(key_x(face, next.level), 0, 0, 0);
                cube_lo = next.lo;
                cube_hi = next.hi;
            } else {
                // Crossed in this frame or the finer one given.
                exact = false;
                if next.frame != NONE {
                    frame_slot = next.frame;
                    key = chunk_keys[next.frame];
                }
                cube_lo = to_frame(next.lo, next.level - (key_level(key.x)), key, true);
                cube_hi = clip_to_face(to_frame(next.hi, next.level - (key_level(key.x)), key, false), key);
            }
        }
    }
    WCOUNT(8)
    WCOUNT_BOXES
    return WalkEnd(false, t_prev, slot, vec3<i32>(0), 0u, 0u, 0u);
}

// The camera's rays all start at the eye: their origin is a constant zero
// the walk folds away (3% of its instructions, and registers);
// orthographic views' (`ortho_view`) start on their near plane.
@compute @workgroup_size(8, 8)
fn trace(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = rect_id(gid, 1u);
    trace_pixel(id, false);
}

@compute @workgroup_size(8, 8)
fn trace_ortho(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = rect_id(gid, 1u);
    trace_pixel(id, true);
}

// Pixel `id.xy`'s round of the trace (`ortho`: an orthographic view).
fn trace_pixel(id: vec3<u32>, ortho: bool) {
    let size = textureDimensions(seed);
    if id.x >= size.x || id.y >= size.y { return; }
    let p = vec2<i32>(id.xy);
    let exit = textureLoad(out_exits, p).r;
    if exit < 0.0 { return; }
    if exit == 0.0 && params.volume.y == 0u {
        textureStore(out_visibility, p, vec4<u32>(0xffffffffu, 0u, 0u, 0u));
        textureStore(out_distance, p, vec4<f32>(0.0));
        if plain_colour() { textureStore(out_colour, p, vec4<f32>(0.0)); }
    }
    let dir = pixel_ray(id.xy, size);
    // A later volume's pass: only a hit nearer than an earlier volume's.
    var limit = 1e30;
    if params.volume.y > 0u {
        let hit = textureLoad(out_distance, p).r;
        if hit > 0.0 { limit = hit; }
    }
    // Under the ground (a cave, a dig) the first round walks from the eye
    // itself: the chunks around it may be air or hold it under their
    // occupied layers, and the rock beside them has no box, so a ray seeded
    // by the boxes would start past them. Rock at the eye: the boxes.
    if exit == 0.0 && params.cut.z != NONE && !ortho {
        let from_eye = point_walk(id.xy, vec3<f32>(0.0), vec2<u32>(params.cut.z, 0u), dir, limit, true);
        if from_eye.hit && from_eye.slot != NONE {
            store_hit(p, from_eye, dir, id.xy);
            return;
        }
        if !from_eye.hit {
            textureStore(out_exits, p, vec4<f32>(select(from_eye.t + 1e-4, -1.0, from_eye.t < 0.0 || from_eye.t >= limit)));
            return;
        }
    }
    var box = textureLoad(seed, p, 0).x;
    // No box left: the sky.
    if box == NONE {
        textureStore(out_exits, p, vec4<f32>(-1.0));
        return;
    }
    // Its nearest box past its mesh: hidden, the box the chunk its mesh
    // point is placed by (the first volume's: its pass walks the meshes).
    if (box & BEHIND_MESH) != 0u {
        if params.volume.y == 0u {
            textureStore(out_visibility, p, vec4<u32>(NONE, (box & ~BEHIND_MESH) + 1u, 0u, 0u));
        }
        textureStore(out_exits, p, vec4<f32>(-1.0));
        return;
    }
    let inside = (box & INSIDE_SEED) != 0u;
    box &= ~INSIDE_SEED;
    COUNT(0)
    let org = select(vec3<f32>(0.0), pixel_origin(id.xy, size), ortho);
    var t_prev = max(exit, params.sun.w);
    // Past the pixel's mesh nothing is seen: its ray ends there.
    var t_end = 1e30;
    if !ortho {
        let mesh = mesh_point(id.xy, size);
        if mesh.w > 0.0 { t_end = length(mesh.xyz) * MESH_MARGIN; }
    }
    // The chunk whose frame the ray is in (its last), and the box it is
    // crossing in that frame's cells: the chunk itself, or air.
    var frame = chunk_frame(box);
    var key = chunk_keys[bitcast<u32>(frame.box.z)];
    var cube_lo = vec3<i32>(0);
    var cube_hi = vec3<i32>(64);
    var real = true;
    let r = fade_dither(id.xy);
    // Both levels of a switch are drawn: a box of the one this pixel does
    // not show starts the walk where the ray enters it.
    let own = find_entry(key);
    if !inside && own != NONE && choice(chunk_table[own * 2u + 1u].y, r) != 1 && !exact_level(key_level(key.x)) {
        let o = vec3<f32>(frame.i0.w, frame.i1.w, frame.i2.w) + vec3<f32>(dot(frame.i0.xyz, org), dot(frame.i1.xyz, org), dot(frame.i2.xyz, org));
        let d = vec3<f32>(dot(frame.i0.xyz, dir), dot(frame.i1.xyz, dir), dot(frame.i2.xyz, dir));
        let inv = 1.0 / select(d, vec3<f32>(1e-30), d == vec3<f32>(0.0));
        let ta = (vec3<f32>(0.0, 0.0, frame.box.x) - o) * inv;
        let tb = (vec3<f32>(64.0, 64.0, frame.box.y) - o) * inv;
        let lo = min(ta, tb);
        t_prev = max(max(lo.x, max(lo.y, lo.z)), t_prev);
        let q = o + d * t_prev;
        let local = clamp(vec3<i32>(floor(q)), vec3<i32>(0, 0, i32(frame.box.x)), vec3<i32>(63, 63, i32(frame.box.y) - 1));
        let next = place_at(key.x & 7, key_level(key.x), key.yzw * 64 + local, clamp(fract(q), vec3<f32>(0.0), vec3<f32>(0.999)), r, false);
        if next.slot != NONE {
            frame = chunk_frame(next.slot);
            key = chunk_keys[next.slot];
        } else if next.level == LOST || next.level == BURIED || next.level == ROCK {
            textureStore(out_exits, p, vec4<f32>(t_prev + 1e-4));
            return;
        } else {
            if next.frame != NONE {
                frame = chunk_frame(next.frame);
                key = chunk_keys[next.frame];
            }
            cube_lo = to_frame(next.lo, next.level - (key_level(key.x)), key, true);
            cube_hi = clip_to_face(to_frame(next.hi, next.level - (key_level(key.x)), key, false), key);
            real = false;
        }
    }
    var start = Walk(bitcast<u32>(frame.box.z), key, cube_lo, cube_hi, real);
    // Seeded from inside: where the ray starts (`INSIDE_SEED`; in rock or
    // off the cut there, the seed's box as before).
    if inside {
        let at = point_start(org + dir * t_prev, box, r);
        if at.kind == 0u { start = at.walk; }
    }
    let end = walk(start, org, dir, t_prev, min(limit, t_end), r, 128u, true, id.xy);
    if !end.hit {
        // At its mesh: resolved, the chunk it ended in kept (`frame_chunk`).
        if end.t >= t_end {
            if params.volume.y == 0u {
                textureStore(out_visibility, p, vec4<u32>(NONE, end.slot + 1u, 0u, 0u));
            }
            textureStore(out_exits, p, vec4<f32>(-1.0));
            return;
        }
        // Unresolved: the next round seeds the boxes beyond (none past
        // the ceiling, the sky, or an earlier volume's hit).
        textureStore(out_exits, p, vec4<f32>(select(end.t + 1e-4, -1.0, end.t < 0.0 || end.t >= limit)));
        return;
    }
    COUNT(2)
    store_hit(p, end, dir, id.xy);
}

// Pixel `p`'s hit `end` (its ray along `dir`): visibility, distance and
// the plain colour.
fn store_hit(p: vec2<i32>, end: WalkEnd, dir: vec3<f32>, pixel: vec2<u32>) {
    textureStore(out_exits, p, vec4<f32>(-1.0));
    let material = shown_material(end.material, end.slot, end.cell, end.axis == 2u && end.sign == 1u);
    // The material uses five bits (the voxel palette has 32 entries); six
    // spare high bits retain a quantized primary-walk cost for publish().
    let packed_cost = min(heat / 3u, 63u);
    let packed_cell = u32(end.cell.x) | (u32(end.cell.y) << 6u) | (u32(end.cell.z) << 12u)
        | (end.axis << 18u) | (end.sign << 20u) | (min(material, 31u) << 21u)
        | (packed_cost << 26u);
    textureStore(out_visibility, p, vec4<u32>(end.slot, packed_cell, 0u, 0u));
    textureStore(out_distance, p, vec4<f32>(end.t, 0.0, 0.0, 0.0));
    if !plain_colour() { return; }
    let c = cell_frame(end.slot, end.cell);
    let axis_world = select(select(c.m2.xyz, c.m1.xyz, end.axis == 1u), c.m0.xyz, end.axis == 0u);
    let normal = normalize(axis_world) * select(-1.0, 1.0, end.sign == 1u);
    let light = 0.25 + 0.75 * max(dot(normal, params.sun.xyz), 0.0);
    var colour = palette(material).rgb;
    if debug_mode() == 1u {
        let uv = voxel_face_uv(c, end.cell, end.axis, dir * end.t);
        colour = textureSampleLevel(uv_reference_texture, uv_reference_sampler, uv, 0.0).rgb / light;
    } else if debug_mode() == 32u {
        // Steps: red to 64, yellow to 128, white to 192.
        let h = f32(heat) / 64.0;
        colour = clamp(vec3<f32>(h, h - 1.0, h - 2.0), vec3<f32>(0.0), vec3<f32>(1.0)) / light;
    } else if voxel_levels_debug() {
        colour = level_colour((bitcast<u32>(c.box.w) >> 8u) & 31u);
    }
    textureStore(out_colour, p, vec4<f32>(colour * light, 1.0));
}

// ---- 3. Publish ----

@group(1) @binding(4) var hit_distance: texture_2d<f32>;

@vertex
fn resolve_vertex(@builtin(vertex_index) v: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((v << 1u) & 2u), f32(v & 2u));
    return vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
}

// An orthographic view's rays to trace (`trace_view`'s open rects, the
// rest resolved): unresolved.
@fragment
fn exits_open() -> @location(0) f32 {
    return 0.0;
}

@group(1) @binding(7) var hit_visibility: texture_2d<u32>;
@group(1) @binding(14) var hit_normal: texture_2d<f32>;
// The sky's visibility, denoised (`filter_last`).
@group(1) @binding(28) var hit_sky: texture_2d<f32>;
@group(1) @binding(25) var uv_reference_texture: texture_2d<f32>;
@group(1) @binding(26) var uv_reference_sampler: sampler;

// ---- Appearance ----

// Visibility of the voxel mosaic (each cell's own hue and brightness) for a
// cell `projected` pixels wide: kept down to about half a pixel; TSR
// averages what is smaller.
fn mosaic_weight(projected: f32) -> f32 {
    return smoothstep(0.35, 0.7, projected);
}

// How far a cell of chunk key `key` spanning `projected` pixels is coarser
// than its cut wants (0 to 1): the cut keeps every level but the finest at
// a pixel a cell or less, so a larger one is a tile still refining (an eye
// dropped from orbit, a slow frame's generation). It reads as the ground it
// stands for, not as the box it is: lit by the slope, without its own
// pigment, edges or corner shading. Finest cells keep their voxel look at
// any size.
fn under_refined(key: vec4<i32>, projected: f32) -> f32 {
    if (key_level(key.x)) == 0 { return 0.0; }
    return smoothstep(1.5, 4.0, projected);
}

// A pixel's width (m) at distance `t` along pixel `q`'s ray.
fn pixel_width(q: vec2<u32>, size: vec2<u32>, t: f32) -> f32 {
    return t * length(pixel_ray(q + vec2<u32>(1u, 0u), size) - pixel_ray(q, size));
}

// Material `m`'s colour varied by its patches (409 m and 25.6 m), clumps
// (6.4 m and 1.6 m, fading below a few pixels) and voxels (`blade`, in
// [-0.5, 0.5], by `voxel`), at domain point `p` for a pixel `pixel` m wide.
fn material_albedo(m: u32, p: vec3<i32>, blade: f32, pixel: f32, voxel: f32) -> vec3<f32> {
    let material = materials.m[min(m, 31u)];
    if material.patches[0].w <= 0.0 { return material.colour.rgb; }
    let broad = f32(noise(p, 15u, 0x3c6ef372u)) / f32(NOISE_ONE);
    let patches = f32(noise(p, 11u, 0xa54ff53au)) / f32(NOISE_ONE) * clamp((12.8 - pixel) / 6.4, 0.0, 1.0);
    var clumps = 0.0;
    if pixel < 3.2 {
        clumps += 0.35 * f32(noise(p, 9u, 0x510e527fu)) / f32(NOISE_ONE) * clamp((3.2 - pixel) / 1.6, 0.0, 1.0);
    }
    if pixel < 0.8 {
        clumps += 0.3 * f32(noise(p, 7u, 0x9b05688cu)) / f32(NOISE_ONE) * clamp((0.8 - pixel) / 0.4, 0.0, 1.0);
    }
    let t = clamp(0.52 + materials.detail.x * (0.7 * broad + 0.16 * patches + clumps) + materials.detail.y * blade * voxel, 0.0, 1.0);
    return select(mix(material.patches[1].rgb, material.patches[2].rgb, t * 2.0 - 1.0), mix(material.patches[0].rgb, material.patches[1].rgb, t * 2.0), t < 0.5);
}

fn corner_ao(side1: bool, side2: bool, corner: bool) -> f32 {
    if side1 && side2 { return 0.0; }
    return 3.0 - f32(u32(side1) + u32(side2) + u32(corner));
}

// Whether cell `c` (in chunk `slot`'s cells, possibly past them: then in
// the drawn chunk of its level there) is solid.
fn solid_near(slot: u32, key: vec4<i32>, c: vec3<i32>) -> bool {
    if all(c >= vec3<i32>(0)) && all(c <= vec3<i32>(63)) { return solid_at(slot, c); }
    let at = key.yzw * 64 + c;
    return solid_in(find_entry(vec4<i32>(key.x, at >> vec3<u32>(6u))), at & vec3<i32>(63));
}

struct Look {
    albedo: vec3<f32>,
    ao: f32,
}

// The colour and occlusion of hit cell `cell` of chunk `slot` (its face:
// `axis`, `sign` 1 the positive side) of `material`, at eye-relative
// `position`, for a pixel `pixel_m` wide: the material's patches and the
// cell's pigment, corner occlusion from the cells around the air in front
// of the face, and its edges darkened while it spans several pixels. Each
// fades as the cell shrinks towards a pixel (TSR averages the rest).
fn surface_look(slot: u32, cell: vec3<i32>, axis: u32, sign: u32, hit_material: u32, position: vec3<f32>, pixel_m: f32, pixel: vec2<u32>) -> Look {
    let c = cell_frame(slot, cell);
    let key = chunk_keys[slot];
    let projected = length(c.m0.xyz) / max(pixel_m, 1e-6);
    // Material boundaries filtered where cells are a pixel or less: a
    // leaf's one surface and one subsurface material (coarse leaves draw
    // one of their columns' tops, `vote_top`) would show as blocks of four
    // pixels. A face takes the material at a point jittered up to a leaf's
    // width across it each frame: the material a cell there shows on the
    // same side (solid, the cell beyond it open), of the stored leaf there
    // (TSR averages the frames into a filtered boundary, and a coarse
    // level's leaves into the mixture its finer levels' cells show). Top
    // faces alone left the risers of coarse slopes blocky.
    var material = hit_material;
    let filtered = 1.0 - smoothstep(1.0, 3.0, projected);
    if filtered > 0.0 {
        let r = vec2<f32>(f32(fade_dither(pixel)), f32(fade_dither(pixel + vec2<u32>(37u, 11u)))) / 1024.0;
        let local = vec3<f32>(c.i0.w, c.i1.w, c.i2.w) + vec3<f32>(dot(c.i0.xyz, position), dot(c.i1.xyz, position), dot(c.i2.xyz, position));
        // The face's two axes across it (u, v), the cell's along it.
        let across_u = select(select(0u, 2u, axis == 1u), 1u, axis == 0u);
        let across_v = select(select(1u, 0u, axis == 1u), 2u, axis == 0u);
        var there = cell;
        there[across_u] = i32(floor(local[across_u] + (r.x - 0.5) * 4.0 * filtered));
        there[across_v] = i32(floor(local[across_v] + (r.y - 0.5) * 4.0 * filtered));
        var out = vec3<i32>(0);
        out[axis] = select(-1, 1, sign == 1u);
        let top = axis == 2u && sign == 1u;
        // A cell there showing that side: its leaf holds a face of it.
        if any((there >> vec3<u32>(2u)) != (cell >> vec3<u32>(2u))) && solid_at(slot, there) && !solid_near(slot, key, there + out) {
            let index = stored_leaf(slot, there);
            if index != NONE {
                var own = 0u;
                if slot_details[slot] != 0u { own = leaf_detail(index, there); }
                let pair = leaf_pair(index);
                let shown = select((pair >> 8u) & 0xffu, pair & 0xffu, top || !solid_over(slot, there));
                if own != 0u {
                    material = min(own, 31u);
                } else if shown != 0u {
                    material = min(shown, 31u);
                }
            }
        }
    }
    let smooth_ground = 1.0 - under_refined(key, projected);
    // A smooth world fades the voxels' own look: their per-voxel pigment
    // and darkened edges.
    let smoothing = materials.shape.x;
    let mosaic = mosaic_weight(projected) * smooth_ground * (1.0 - 0.6 * smoothing);
    // The cell centre's noise-domain point: the same pigment from every
    // face of it.
    let centre = vec3<f32>(c.m0.w, c.m1.w, c.m2.w) + mat3x3<f32>(c.m0.xyz, c.m1.xyz, c.m2.xyz) * (vec3<f32>(cell) + 0.5);
    let p = params.eye_domain.xyz + vec3<i32>(floor(params.eye_domain_frac.xyz + centre / DOMAIN_UNIT));
    // Each voxel's own brightness and hue, as much shared with the voxels
    // around (0.4 m clumps) as its own: a hash a voxel alone read as a
    // checkerboard wherever light alone shapes nothing (shade, overcast).
    let clump = f32(noise(p, 5u, 0x3a1f9e27u)) / f32(NOISE_ONE);
    let hv = hash3(p.x, p.y, p.z, 0x68bc21ebu);
    let shade = 0.6 * (f32(hv & 255u) / 255.0 - 0.5) + 0.4 * clump;
    let pigment = 1.0 + materials.detail.y * shade * mosaic;
    let hue = 0.6 * (f32(hash3(p.x, p.y, p.z, 0x1b873593u) & 255u) / 255.0 - 0.5) + 0.4 * clump;
    let albedo = material_albedo(material, p, hue, pixel_m, mosaic) * pigment;
    // Occlusion: around the air cell in front of the face, where voxels
    // (not cells: the same at every level, as `hit_face`'s slope) are a
    // few pixels wide; under that the sky samples hold the occlusion and
    // the eight lookups would only add sub-pixel noise.
    let voxel_px = params.planet.y / max(pixel_m, 1e-6);
    let ao_weight = smoothstep(1.0, 3.0, voxel_px) * smoothstep(0.75, 1.25, projected) * smooth_ground;
    var ao = 1.0;
    if ao_weight > 0.0 {
        let out_dir = select(-1, 1, sign == 1u);
        let along = vec3<i32>(select(0, out_dir, axis == 0u), select(0, out_dir, axis == 1u), select(0, out_dir, axis == 2u));
        let du = vec3<i32>(select(1, 0, axis == 0u), select(0, 1, axis == 0u), 0);
        let dv = vec3<i32>(0, select(0, 1, axis == 2u), select(1, 0, axis == 2u));
        let f = cell + along;
        let s0 = solid_near(slot, key, f - du);
        let s1 = solid_near(slot, key, f + du);
        let s2 = solid_near(slot, key, f - dv);
        let s3 = solid_near(slot, key, f + dv);
        let c00 = corner_ao(s0, s2, solid_near(slot, key, f - du - dv));
        let c10 = corner_ao(s1, s2, solid_near(slot, key, f + du - dv));
        let c01 = corner_ao(s0, s3, solid_near(slot, key, f - du + dv));
        let c11 = corner_ao(s1, s3, solid_near(slot, key, f + du + dv));
        // Where in the face the hit is.
        let local = vec3<f32>(c.i0.w, c.i1.w, c.i2.w) + vec3<f32>(dot(c.i0.xyz, position), dot(c.i1.xyz, position), dot(c.i2.xyz, position));
        let in_cell = clamp(local - vec3<f32>(cell), vec3<f32>(0.0), vec3<f32>(1.0));
        let uv = vec2<f32>(dot(in_cell, vec3<f32>(du)), dot(in_cell, vec3<f32>(dv)));
        let a = mix(mix(c00, c10, uv.x), mix(c01, c11, uv.x), uv.y) / 3.0;
        ao = mix(1.0, mix(0.42, 1.0, a), ao_weight);
        // Crisp voxel edges while a cell spans several pixels.
        let edge = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
        let fade = clamp((projected - 3.0) / 6.0, 0.0, 1.0);
        ao *= 1.0 - materials.detail.z * (1.0 - smoothing) * fade * ao_weight * (1.0 - smoothstep(0.0, 0.12, edge));
    }
    return Look(albedo, ao);
}

// Renderer-wide debug mode (`params.debug.x` & 0xff): 32 the voxel trace's
// cost, 34 the levels drawn, 35 the levels over the left half.
fn debug_mode() -> u32 {
    return params.debug.x & 0xffu;
}

fn voxel_levels_debug() -> bool {
    return debug_mode() == 34u || debug_mode() == 35u;
}

fn voxel_levels_half_debug() -> bool {
    return debug_mode() == 35u;
}

// Whether the walks write the plain colour target (bit 8 of
// `params.debug.x`: a world drawn alone; a host's gbuffer takes the
// published surface instead).
fn plain_colour() -> bool {
    return (params.debug.x & 0x100u) != 0u;
}

// Steady shadows (`ChunkWorld::set_steady_shadows`): every hit walks its own
// sun ray and the sky rays hold still, instead of samples that move each
// frame for the denoiser to gather (dearer; the walks' own values, a
// reference for the denoised ones).
fn steady_shadows() -> bool {
    return (params.debug.x & 0x1000u) != 0u;
}

// The sun is a disk (the atmosphere's angular radius, else the host's):
// each sun walk aims at a point of it, so a shadow's penumbra widens with
// the occluder's distance. Sampled walks take a point a frame, a pixel's
// own low-discrepancy sequence (R2, rotated by a hash of the pixel), and
// the denoiser integrates them; steady shadows average SUN_DISK_RAYS
// points (the reference).
const SUN_DISK_RAYS: u32 = 32u;

fn sun_disk_cos() -> f32 {
    return select(cos(params.sky.y), params.sky.z, params.sky.z > 0.99);
}

// The direction towards point `u` ([0, 1)^2, uniform over the disk) of the
// sun's disk.
fn sun_ray(u: vec2<f32>) -> vec3<f32> {
    let s = params.sun.xyz;
    let c = sun_disk_cos();
    let spread = sqrt(max(1.0 - c * c, 0.0)) / c * sqrt(u.x);
    let t1 = normalize(select(cross(s, vec3<f32>(0.0, 0.0, 1.0)), cross(s, vec3<f32>(1.0, 0.0, 0.0)), abs(s.z) > 0.9));
    let a = 6.2831853 * u.y;
    return normalize(s + (t1 * cos(a) + cross(s, t1) * sin(a)) * spread);
}

// Pixel `q`'s point of the disk this frame.
fn sun_point(q: vec2<u32>) -> vec2<f32> {
    let h = key_hash(vec4<i32>(vec2<i32>(q), 0x51ed27, 0x2c1b3c6d));
    let n = params.table.w;
    // R2's steps (the plastic number's), in 32-bit fixed point.
    let x = vec2<u32>(n * 3242174889u + h, n * 2447445413u + (h * 0x9E3779B1u));
    return vec2<f32>(x >> vec2<u32>(8u)) / 16777216.0;
}

// Point `i` of SUN_DISK_RAYS spread evenly over the disk.
fn sun_disk_point(i: u32) -> vec2<f32> {
    return vec2<f32>((f32(i) + 0.5) / f32(SUN_DISK_RAYS), fract(f32(i) * 0.618034));
}

fn voxel_face_uv(c: ChunkDraw, cell: vec3<i32>, axis: u32, position: vec3<f32>) -> vec2<f32> {
    let frame_position = vec3<f32>(c.i0.w, c.i1.w, c.i2.w)
        + vec3<f32>(dot(c.i0.xyz, position), dot(c.i1.xyz, position), dot(c.i2.xyz, position));
    let in_cell = clamp(frame_position - vec3<f32>(cell), vec3<f32>(0.0), vec3<f32>(1.0));
    if axis == 0u {
        return in_cell.yz;
    }
    if axis == 1u {
        return in_cell.xz;
    }
    return in_cell.xy;
}

// A level's colour (`HELIO_VOXEL_CHUNKS_LEVELS`, environment-only diagnostic).
fn level_colour(level: u32) -> vec3<f32> {
    // A switch, not an array lookup (see `face_n`).
    switch level & 7u {
        case 0u: { return vec3<f32>(0.9, 0.2, 0.2); }
        case 1u: { return vec3<f32>(0.9, 0.6, 0.1); }
        case 2u: { return vec3<f32>(0.8, 0.9, 0.1); }
        case 3u: { return vec3<f32>(0.2, 0.8, 0.2); }
        case 4u: { return vec3<f32>(0.1, 0.8, 0.8); }
        case 5u: { return vec3<f32>(0.2, 0.3, 0.9); }
        case 6u: { return vec3<f32>(0.6, 0.2, 0.9); }
        default: { return vec3<f32>(0.9, 0.2, 0.7); }
    }
}

// The hit `position`'s place in cell `cell` of chunk frame `c`, in the
// cell's own coordinates ([0, 1]^3).
fn voxel_in_cell(c: ChunkDraw, cell: vec3<i32>, position: vec3<f32>) -> vec3<f32> {
    let frame_position = vec3<f32>(c.i0.w, c.i1.w, c.i2.w)
        + vec3<f32>(dot(c.i0.xyz, position), dot(c.i1.xyz, position), dot(c.i2.xyz, position));
    return clamp(frame_position - vec3<f32>(cell), vec3<f32>(0.0), vec3<f32>(1.0));
}

// A voxel material's surface from the host's scene materials (`chunks::scene`:
// textures through Helio's material table); `valid` is false where the host
// maps none, or draws without them.
struct SceneSurface {
    albedo: vec3<f32>,
    roughness: f32,
    emissive: vec3<f32>,
    // In the face's texture frame (x along u, y against v, z out).
    normal: vec3<f32>,
    occlusion: f32,
    swash: vec3<f32>,
    valid: bool,
}

SCENE_MATERIALS

// Shading normal `n` bent by texture normal `t` (in the face's texture
// frame: `scene_surface`) on a face along `axis` of a cell of frame `c`:
// the texture's u and up directions there (cell axes, as `scene_surface`
// lays textures on faces), made perpendicular to `n`.
fn textured_normal(n: vec3<f32>, t: vec3<f32>, c: ChunkDraw, axis: u32) -> vec3<f32> {
    if t.z >= 0.9999 { return n; }
    // u, and up the texture (against v): x and -y on a top or bottom; y or
    // x and up (z) on the sides.
    var u = c.m0.xyz;
    var up = -c.m1.xyz;
    if axis == 0u {
        u = c.m1.xyz;
        up = c.m2.xyz;
    } else if axis == 1u {
        up = c.m2.xyz;
    }
    let tu = normalize(u - n * dot(n, u));
    var tv = cross(n, tu);
    tv *= select(-1.0, 1.0, dot(tv, up) >= 0.0);
    return normalize(tu * t.x + tv * t.y + n * t.z);
}

// Helio's gbuffer (`engine::GBUFFER_FORMATS`).
// Helio's surface flag for two-sided foliage (`pbr_eval.wgsl`).
const SURFACE_FLAG_TWO_SIDED: u32 = 8u;
struct GOut {
    @location(0) albedo: vec4<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) orm: vec4<f32>,
    @location(3) emissive: vec4<f32>,
    @location(4) lightmap_uv: vec2<f32>,
    @location(5) sss: vec4<f32>,
    @location(6) extra: vec4<f32>,
    @location(7) velocity: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

// The hits the frame shows, over the meshes' gbuffer. Which those are was
// decided once, by `shadow` against the meshes' depth (`mesh_point`): a hit
// with a mesh in front of it, or none, has a zero normal. The pipeline
// writes the depth without testing it (`Always`): a second decision by the
// depth test could disagree with the first, and on NVIDIA's Vulkan driver
// (616.64, RTX 3060) it did, whole runs at a time: this pass's depth test
// read a depth surface that a copy of it read correctly the moment before,
// let the hits through over every mesh in front (the meadow became the turf
// behind it with `shadow`'s zero normal: black grass) and left the depth
// garbage (runs of eight equal texels, the sky's too).
@fragment
fn publish(@builtin(position) pixel: vec4<f32>) -> GOut {
    let p = vec2<i32>(pixel.xy);
    let v = textureLoad(hit_visibility, p, 0).xy;
    let shading = textureLoad(hit_normal, p, 0);
    if v.x == NONE || !ours(v.x) || dot(shading.xyz, shading.xyz) == 0.0 { discard; }
    let t = textureLoad(hit_distance, p, 0).r;
    let size = textureDimensions(hit_distance);
    let position = pixel_ray(vec2<u32>(pixel.xy), size) * t;
    let clip = params.view_proj * vec4<f32>(position, 1.0);
    let c = chunk_frame(v.x);
    let axis = (v.y >> 18u) & 3u;
    let sign = (v.y >> 20u) & 1u;
    let material = (v.y >> 21u) & 31u;
    // In the volume's frame (the cell's axes), out in the world's.
    var normal = to_local(shading.xyz);
    let cell = vec3<i32>(vec3<u32>(v.y, v.y >> 6u, v.y >> 12u) & vec3<u32>(63u));
    // A pixel's width at the hit (m).
    let pixel_m = pixel_width(vec2<u32>(pixel.xy), size, t);
    let look = surface_look(v.x, cell, axis, sign, material, position, pixel_m, vec2<u32>(pixel.xy));
    let f0 = 0.03;
    var out: GOut;
    var albedo = look.albedo;
    var roughness = palette(material).w;
    var emissive = albedo * materials.m[min(material, 31u)].light.y;
    let level = u32(key_level(chunk_keys[v.x].x)) & 31u;
    // A pixel's footprint along the face over its width (the view's slant).
    let slant = 1.0 / max(abs(dot(normal, normalize(position))), 1.0 / 16.0);
    let world_xz_from_eye = to_world(position).xz;
    let scene = scene_surface(material, axis, sign, level, chunk_keys[v.x].yzw * 64 + cell, voxel_in_cell(c, cell, position), pixel_m, slant, world_xz_from_eye);
    var occlusion = 1.0;
    if scene.valid {
        albedo = scene.albedo;
        roughness = scene.roughness;
        emissive = scene.emissive;
        occlusion = scene.occlusion;
        normal = textured_normal(normal, scene.normal, c, axis);
    }
    // The land-side half of Ultra-Water's shared shore phase: an advancing
    // bore darkens and smooths the sand, with a narrow foam edge and a drying
    // wet line. The optional scene-material shader evaluates the shared field.
    if axis == 2u && sign == 1u {
        let wetness = clamp(scene.swash.z, 0.0, 1.0);
        let film = clamp(scene.swash.x / 0.08, 0.0, 1.0);
        let edge_foam = clamp(scene.swash.y, 0.0, 1.0);
        albedo *= mix(1.0, 0.62, max(film, wetness * 0.55));
        albedo = mix(albedo, vec3<f32>(0.78, 0.84, 0.84), edge_foam * 0.72);
        roughness = mix(roughness, 0.09, max(film, wetness * 0.8));
    }
    out.albedo = vec4<f32>(albedo, 1.0);
    if debug_mode() == 32u {
        let h = f32((v.y >> 26u) & 63u) * 3.0 / 64.0;
        let cost = clamp(vec3<f32>(h, h - 1.0, h - 2.0), vec3<f32>(0.0), vec3<f32>(1.0));
        out.albedo = vec4<f32>(cost, 1.0);
    }
    if debug_mode() == 1u {
        let uv = voxel_face_uv(c, cell, axis, position);
        out.albedo = vec4<f32>(textureSample(uv_reference_texture, uv_reference_sampler, uv).rgb, 1.0);
    }
    // Environment-only level diagnostics (`HELIO_VOXEL_CHUNKS_LEVELS`).
    if voxel_levels_debug() && (!voxel_levels_half_debug() || p.x < i32(size.x / 2u)) {
        out.albedo = vec4<f32>(level_colour(u32(key_level(chunk_keys[v.x].x)) & 31u), 1.0);
    }
    out.normal = vec4<f32>(to_world(normal), f0);
    out.orm = vec4<f32>(look.ao * textureLoad(hit_sky, p, 0).r * occlusion, clamp(roughness, 0.04, 1.0), 0.0, f0);
    out.emissive = vec4<f32>(emissive, f0);
    // Terrain tag consumed by deferred lighting (skips screen-space AO).
    out.lightmap_uv = vec2<f32>(-1.0, -2.0);
    // Materials that let light through are two-sided foliage: the sun from
    // behind comes through tinted by their colour (its share through the
    // leaf and those past it is the sun's visibility: `shadow_fallback`).
    let through = materials.m[min(material, 31u)].light.x;
    out.sss = vec4<f32>(select(vec3<f32>(0.0), albedo, through > 0.0), 0.0);
    out.extra = vec4<f32>(0.0, 0.0, 0.0, select(0.0, f32(SURFACE_FLAG_TWO_SIDED), through > 0.0));
    // A still volume: w 0 (helio-pass-tsr MOTION_VECTORS) leaves its
    // motion, the camera's, to TSR, which derives it from depth at full
    // precision. These eye-relative matrices are f32 products that cancel
    // the camera's position: near points move by up to 0.05 px of rounding.
    // A volume that moved moves by itself (w 1): the last frame's projection
    // takes its points from where its transform had them.
    let previous = params.prev_view_proj * vec4<f32>(position, 1.0);
    var velocity = vec2<f32>(0.0);
    if previous.w > 0.0 {
        let ndc = previous.xy / previous.w;
        velocity = pixel.xy - (vec2<f32>(ndc.x, -ndc.y) * 0.5 + 0.5) * vec2<f32>(size);
    }
    out.velocity = vec4<f32>(velocity, 0.0, select(0.0, 1.0, (params.volume.w & 2u) != 0u));
    out.depth = clip.z / clip.w;
    return out;
}

// ---- 4. Sun shadows ----
// A walk towards the sun from a quarter of the hits (one a 2x2 block, the
// pixel moving each frame: the denoiser gathers them all, 4c), a
// hundredth of a cell off their face; every hit then takes the sample of
// its block, or of a neighbouring one, that walked within four of its
// cells; the others (silhouettes) are listed and walk in a pass of their
// own, so a few of them do not hold whole warps. The samples also measure
// the slope's normal (smooth: it serves the block).

// Helio's directional visibility (`filter_last`): x the sun's visibility,
// yzw the sun it is for.
@group(1) @binding(8) var out_sun: texture_storage_2d<rgba16float, write>;
// Each pixel's sun as its walk (or its block's) found it, 0 or 1; 1 where
// it is its own (its walk, or none needed), 0 where a neighbour's; 1 where
// a terrain hit is shown (`sun_raw`, `walked_sun`); 1 where the sun's
// occlusion pages gave it (every volume is traced into them: no other
// volume's walk is wanted, `shadow_cross`): bytes of a word a pixel, rows
// of the frame. (The denoiser's own data are buffers: an image load is
// bounds-checked by two size queries.)
@group(1) @binding(41) var<storage, read_write> out_sun_raw: array<u32>;

fn store_sun(p: vec2<i32>, size: vec2<u32>, lit: f32, own: f32, terrain: f32, paged: bool) {
    out_sun_raw[u32(p.y) * size.x + u32(p.x)] = pack4x8unorm(vec4<f32>(lit, own, terrain, select(0.0, 1.0, paged)));
}

fn store_walked(p: vec2<i32>, size: vec2<u32>, lit: f32, own: f32, terrain: f32) {
    store_sun(p, size, lit, own, terrain, false);
}
// The normal each hit is lit with (`Face::shading`) and its sky sample
// (zero where a mesh is in front of the hit, or the sky).
@group(1) @binding(13) var out_normal: texture_storage_2d<rgba16float, write>;
// Samples: lit (bit 0), walked (it faced the sun, bit 1), a hit (bit 2);
// its distance's bits; its slope's normal (`pack_unit`, 0: none).
@group(1) @binding(9) var out_sun_samples: texture_storage_2d<rgba32uint, write>;
@group(1) @binding(10) var sun_samples: texture_2d<u32>;
// Hits no sample serves: their count and pixels (x | y << 16), and the
// workgroups of their indirect dispatch (x, 1, 1).
struct Fallbacks {
    count: atomic<u32>,
    pixels: array<u32>,
}
@group(1) @binding(11) var<storage, read_write> fallbacks: Fallbacks;
@group(1) @binding(12) var<storage, read_write> fallback_groups: array<atomic<u32>, 3>;

// A hit's face: its unit normal, the length of a cell across it; the
// normal it is lit with, and how far (cells) its sun and sky rays ignore
// what they meet (natural steps and cells of a pixel or less: `shading`);
// the slope's normal there (zero: none); how far a local light's ray
// ignores (cells of a pixel or less alone: a lamp near the ground meets
// the real steps, and a step's skip could pass what stands beside the lamp,
// or the lamp itself).
struct Face {
    normal: vec3<f32>,
    cell: f32,
    shading: vec3<f32>,
    skip: f32,
    ground: vec3<f32>,
    light_skip: f32,
}

// The top of the column `d` cells from `cell` (chunk `slot`'s cells) within
// four layers of `cell`'s, and whether there is one.
fn column_top_near(slot: u32, key: vec4<i32>, cell: vec3<i32>, d: vec2<i32>) -> vec2<f32> {
    var c = cell + vec3<i32>(d, 0);
    if solid_near(slot, key, c + vec3<i32>(0, 0, 1)) {
        for (var n = 0; n < 4; n++) {
            c.z += 1;
            if !solid_near(slot, key, c + vec3<i32>(0, 0, 1)) { return vec2<f32>(f32(c.z), 1.0); }
        }
        return vec2<f32>(0.0);
    }
    for (var n = 0; n < 5; n++) {
        if solid_near(slot, key, c) { return vec2<f32>(f32(c.z), 1.0); }
        c.z -= 1;
    }
    return vec2<f32>(0.0);
}

// The ground's normal at a natural step `cell` (chunk `slot`, frame `c`):
// the slope of the column tops two cells away each way, in the world;
// zero where they are not found.
fn slope_normal(slot: u32, key: vec4<i32>, cell: vec3<i32>, c: ChunkDraw) -> vec3<f32> {
    let x0 = column_top_near(slot, key, cell, vec2<i32>(-2, 0));
    let x1 = column_top_near(slot, key, cell, vec2<i32>(2, 0));
    let y0 = column_top_near(slot, key, cell, vec2<i32>(0, -2));
    let y1 = column_top_near(slot, key, cell, vec2<i32>(0, 2));
    let top = f32(cell.z);
    // One-sided where a side has no top near.
    let gx = select(select((x1.x - top) / 2.0, (top - x0.x) / 2.0, x0.y > 0.0), (x1.x - x0.x) / 4.0, x0.y > 0.0 && x1.y > 0.0);
    let gy = select(select((y1.x - top) / 2.0, (top - y0.x) / 2.0, y0.y > 0.0), (y1.x - y0.x) / 4.0, y0.y > 0.0 && y1.y > 0.0);
    if x0.y + x1.y == 0.0 || y0.y + y1.y == 0.0 { return vec3<f32>(0.0); }
    // A cell-space normal to the world: by M's inverse transposed.
    let n = -gx * c.i0.xyz - gy * c.i1.xyz + c.i2.xyz;
    // Zero (not NaN) where it cannot be normalised.
    let l = dot(n, n);
    if !(l > 1e-12) { return vec3<f32>(0.0); }
    return n * inverseSqrt(l);
}

// The quadratic B-spline kernel at distance `x` (cells) and its slope: it
// spans three cells, so the point in a cell meets exactly that cell and its
// neighbours, and its sum over a lattice is 1 (a flat face keeps its
// normal), with a continuous slope.
fn spline_weights(x: f32) -> vec2<f32> {
    let a = abs(x);
    if a < 0.5 { return vec2<f32>(0.75 - a * a, -2.0 * x); }
    if a < 1.5 {
        let u = 1.5 - a;
        return vec2<f32>(0.5 * u * u, -sign(x) * u);
    }
    return vec2<f32>(0.0);
}

// The normal of the smooth surface the solid cells around `cell` (chunk
// `slot`, frame `c`) approximate, at cell-space point `local` in or on it,
// in the world: minus the gradient of their density, solid cells' centres
// weighted by a separable quadratic B-spline (`spline_weights`) so it is
// continuous across cells, the face's own where it is flat and rounding
// edges, steps and corners across about a cell. Zero where the density is
// flat (a thin wall, a symmetric pinch).
fn smooth_normal(slot: u32, key: vec4<i32>, cell: vec3<i32>, local: vec3<f32>, c: ChunkDraw) -> vec3<f32> {
    var wx: array<vec2<f32>, 3>;
    var wy: array<vec2<f32>, 3>;
    var wz: array<vec2<f32>, 3>;
    for (var i = 0; i < 3; i++) {
        let at = vec3<f32>(cell + vec3<i32>(i - 1)) + 0.5;
        wx[i] = spline_weights(local.x - at.x);
        wy[i] = spline_weights(local.y - at.y);
        wz[i] = spline_weights(local.z - at.z);
    }
    var g = vec3<f32>(0.0);
    for (var k = 0; k < 3; k++) {
        for (var j = 0; j < 3; j++) {
            for (var i = 0; i < 3; i++) {
                // The cell itself is solid (a hit).
                let solid = (i == 1 && j == 1 && k == 1) || solid_near(slot, key, cell + vec3<i32>(i - 1, j - 1, k - 1));
                if !solid { continue; }
                g += vec3<f32>(wx[i].y * wy[j].x * wz[k].x, wx[i].x * wy[j].y * wz[k].x, wx[i].x * wy[j].x * wz[k].y);
            }
        }
    }
    // Out of the solid: down the density's gradient; a cell-space covector
    // to the world by M's inverse transposed.
    let n = -g.x * c.i0.xyz - g.y * c.i1.xyz - g.z * c.i2.xyz;
    let l = dot(n, n);
    if !(l > 1e-12) { return vec3<f32>(0.0); }
    return n * inverseSqrt(l);
}

// The ground's normal at pixel `q` (hit at distance `t`) from the hits two
// pixels each way: on each axis the side nearer in distance (not across a
// silhouette), facing the eye; zero where an axis has no hit near. Over
// cells of a pixel or less it spans more than a cell: the slope, not the
// steps.
fn screen_ground(q: vec2<u32>, size: vec2<u32>, t: f32) -> vec3<f32> {
    let p = pixel_ray(q, size) * t;
    var d = array<vec3<f32>, 2>(vec3<f32>(0.0), vec3<f32>(0.0));
    for (var axis = 0u; axis < 2u; axis++) {
        var best = 1e30;
        for (var side = -1; side <= 1; side += 2) {
            var at = vec2<i32>(q);
            at[axis] += 2 * side;
            if any(at < vec2<i32>(0)) || any(at >= vec2<i32>(size)) { continue; }
            let tn = textureLoad(hit_distance, at, 0).r;
            if tn <= 0.0 || abs(tn - t) >= best { continue; }
            best = abs(tn - t);
            d[axis] = (pixel_ray(vec2<u32>(at), size) * tn - p) * f32(side);
        }
        if best > 0.25 * t { return vec3<f32>(0.0); }
    }
    let n = cross(d[0], d[1]);
    let l = dot(n, n);
    if !(l > 1e-20) { return vec3<f32>(0.0); }
    return n * inverseSqrt(l) * select(1.0, -1.0, dot(n, p) > 0.0);
}

// Hit `v` (pixels `pixel_m` wide there). Voxels a few pixels wide are lit
// by their own faces (a slope reads as its steps, each shadowing the next);
// a natural step (air above it, its leaf storing the heightfield's slope) is
// lit partly with the slope's normal, `Materials::detail.w` (step
// softness), its sun ray passing its own steps.
// Voxels of a pixel or less are lit by the slope's normal, at every level:
// the weight follows the voxel's size on screen, not the cell's, so it is
// the same on both sides of a switch between levels, and a coarse level's
// steps (each a cell tall, their shadows long under a low sun) never show
// as stripes. A cell larger than its cut wants (`under_refined`) is lit by
// the slope's normal too. The slope's normal is `ground` when given (a
// nearby sample's), else measured here.
fn hit_face(v: vec2<u32>, ground_hint: vec3<f32>, q: vec2<u32>, size: vec2<u32>, t: f32) -> Face {
    let pixel_m = pixel_width(q, size, t);
    let cell = vec3<i32>(vec3<u32>(v.y, v.y >> 6u, v.y >> 12u) & vec3<u32>(63u));
    let c = cell_frame(v.x, cell);
    let key = chunk_keys[v.x];
    let axis = (v.y >> 18u) & 3u;
    let sign = (v.y >> 20u) & 1u;
    let axis_world = select(select(c.m2.xyz, c.m1.xyz, axis == 1u), c.m0.xyz, axis == 0u);
    let normal = normalize(axis_world) * select(-1.0, 1.0, sign == 1u);
    let cell_m = length(axis_world);
    var f = Face(normal, cell_m, normal, 0.0, vec3<f32>(0.0), 0.0);
    let small = 1.0 - smoothstep(1.0, 3.0, params.planet.y / max(pixel_m, 1e-6));
    let coarse = max(under_refined(key, length(c.m0.xyz) / max(pixel_m, 1e-6)), small);
    // Natural steps only: the leaf stores the heightfield's slope where it
    // holds natural tops. A feature's cells (a boulder, a tree's leaves) or an
    // edit's stand for no slope: softened, rock fields lost their form and
    // shadows, and leaves' sun rays skipped the leaves around them (canopy
    // speckle).
    let stored = leaf_ground(v.x, cell, c);
    let step = dot(stored, stored) > 0.0 && !solid_near(v.x, key, cell + vec3<i32>(0, 0, 1));
    // A world made smooth lights every hit by the smooth surface its voxels
    // approximate (`smooth_normal`), natural steps by their exact slope.
    let smoothing = materials.shape.x;
    if !step && coarse == 0.0 && smoothing <= 0.0 { return f; }
    let w = max(max(select(0.0, materials.detail.w, step), coarse), smoothing);
    // The heightfield's exact slope stored with the leaf; else, cells of a
    // pixel or less: the slope the hits around make (a few loads); larger
    // ones measure their column tops (`slope_normal`).
    var ground = ground_hint;
    if smoothing > 0.0 && !step {
        // At this hit, not a neighbour's: it varies across the face.
        let position = pixel_ray(q, size) * t;
        let local = vec3<f32>(c.i0.w, c.i1.w, c.i2.w) + vec3<f32>(dot(c.i0.xyz, position), dot(c.i1.xyz, position), dot(c.i2.xyz, position));
        ground = smooth_normal(v.x, key, cell, local, c);
    }
    if dot(ground, ground) == 0.0 { ground = stored; }
    if dot(ground, ground) == 0.0 && length(c.m0.xyz) <= 1.5 * pixel_m { ground = screen_ground(q, size, t); }
    if dot(ground, ground) == 0.0 { ground = slope_normal(v.x, key, cell, c); }
    // A unit normal (written so NaN fails it), else the face's own.
    if w <= 0.0 || !(abs(dot(ground, ground) - 1.0) < 0.01) { return f; }
    let shading = mix(normal, ground, w);
    let l = dot(shading, shading);
    if !(l > 1e-6) { return f; }
    f.ground = ground;
    f.shading = shading * inverseSqrt(l);
    // A sun ray passes the steps the smooth surface stands for by as far as
    // it leans from the face (none on a flat wall: no leak at its foot).
    var skip_w = max(select(0.0, materials.detail.w, step), coarse);
    if smoothing > 0.0 && !step {
        skip_w = max(skip_w, smoothing * clamp(2.0 * length(cross(ground, normal)), 0.0, 1.0));
    }
    f.skip = 2.0 * skip_w;
    f.light_skip = 2.0 * coarse;
    return f;
}

// A unit vector in two snorm halves (octahedral) and back.
fn pack_unit(n: vec3<f32>) -> u32 {
    var p = n.xy / (abs(n.x) + abs(n.y) + abs(n.z));
    if n.z < 0.0 { p = (1.0 - abs(p.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), p >= vec2<f32>(0.0)); }
    return pack2x16snorm(p);
}

fn unpack_unit(u: u32) -> vec3<f32> {
    let p = unpack2x16snorm(u);
    var n = vec3<f32>(p, 1.0 - abs(p.x) - abs(p.y));
    if n.z < 0.0 { n = vec3<f32>((1.0 - abs(n.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), n.xy >= vec2<f32>(0.0)), n.z); }
    return normalize(n);
}


// Sky rays a sample casts a frame (one sample a 4x4 block), and how far
// they reach (cells).
const SKY_RAYS: u32 = 4u;
const SKY_RANGE_CELLS: f32 = 48.0;

// `walk` crossing every box in a frame, as at levels finer than the exact
// ones: the walk of the kernels for hits there (sun and sky samples),
// whose rays an exact box's code would cost registers. Coarse air on
// their way strays from the grid by up to 512 x cell / radius of its cells.
fn walk_fine(start: Walk, org: vec3<f32>, dir: vec3<f32>, t_start: f32, t_end: f32, r: i32, boxes: u32, counted: bool, pixel: vec2<u32>) -> WalkEnd {
    // Over the ceiling and rising, a ray meets nothing more (the squared
    // radius q0 + 2 b t + t^2 only grows once b + t > 0; a plane's height
    // q0 + b t once b > 0): it ends there, t -1.
    var q0 = params.ceiling.x + 2.0 * params.up.w * dot(params.up.xyz, org) + dot(org, org);
    var b = params.up.w * dot(params.up.xyz, dir) + dot(org, dir);
    if PLANE_WORLD {
        q0 = params.ceiling.x + org.y;
        b = dir.y;
    }
    var frame = chunk_frame(start.slot);
    var key = start.key;
    var cube_lo = start.lo;
    var cube_hi = start.hi;
    var real = start.real;
    var t_prev = t_start;
    // The axis of the side a step crossed into this box by (as `walk`).
    var stepped = 3u;
    var chunk = 0u;
    for (; chunk < boxes; chunk++) {
        let over = select(b + t_prev > 0.0 && q0 + t_prev * (2.0 * b + t_prev) > 0.0, b > 0.0 && q0 + b * t_prev > 0.0, PLANE_WORLD);
        if over { return WalkEnd(false, -1.0, NONE, vec3<i32>(0), 0u, 0u, 0u); }
        WCOUNT(3)
        let slot = bitcast<u32>(frame.box.z);
        TRACE_PIXEL(chunk, slot, cube_lo, real)
        // The ray in the frame chunk's cells.
        let o = vec3<f32>(frame.i0.w, frame.i1.w, frame.i2.w) + vec3<f32>(dot(frame.i0.xyz, org), dot(frame.i1.xyz, org), dot(frame.i2.xyz, org));
        let d = vec3<f32>(dot(frame.i0.xyz, dir), dot(frame.i1.xyz, dir), dot(frame.i2.xyz, dir));
        let inv = 1.0 / select(d, vec3<f32>(1e-30), d == vec3<f32>(0.0));
        if real { cube_hi = clip_to_face(vec3<i32>(64), key); }
        let ca = (vec3<f32>(cube_lo) - o) * inv;
        let cb = (vec3<f32>(cube_hi) - o) * inv;
        let cube_near = from_side(min(ca, cb), stepped, t_prev);
        let cube_far = max(ca, cb);
        let t_leave = min(cube_far.x, min(cube_far.y, cube_far.z));
        // Whether the ray crosses the box in its frame (as `walk`).
        let crosses = max(cube_near.x, max(cube_near.y, cube_near.z)) < t_leave;
        if real && crosses {
            // The walk over the chunk's occupied layers.
            let ta = (vec3<f32>(0.0, 0.0, frame.box.x) - o) * inv;
            let tb = (vec3<f32>(f32(cube_hi.x), f32(cube_hi.y), frame.box.y) - o) * inv;
            let layers = stepped < 2u || (stepped == 2u && select(frame.box.x <= 0.0, frame.box.y >= 64.0, d.z < 0.0));
            let lo = from_side(min(ta, tb), select(3u, stepped, layers), t_prev);
            let hi = max(ta, tb);
            var t0 = max(max(lo.x, max(lo.y, lo.z)), t_prev);
            var t1 = min(min(hi.x, min(hi.y, hi.z)), t_end);
            // Only under the chunk's plane over its solid cells.
            let top = chunk_meta[slot].top;
            let s0 = o.z - top.x * o.x - top.y * o.y - top.z;
            let ds = d.z - top.x * d.x - top.y * d.y;
            if ds > 0.0 { t1 = min(t1, -s0 / ds); } else if ds < 0.0 { t0 = max(t0, -s0 / ds); } else if s0 > 0.0 { t1 = t0; }
            if t1 > t0 {
                let axis0 = select(select(2u, 1u, lo.y >= lo.z), 0u, lo.x >= lo.y && lo.x >= lo.z);
                let h = trace_chunk(slot, o, d, t0, t1, axis0, vec2<i32>(i32(frame.box.x), i32(frame.box.y)), counted);
                if h.hit {
                    return WalkEnd(true, h.t, slot, h.cell, h.axis, select(1u, 0u, h.positive), h.material);
                }
            }
        }
        // The cell just past the face the ray leaves the box by, in the
        // frame chunk's cells (exact integers far from the origin), where
        // the ray is (as `walk`); beside a box it does not cross, the
        // cell it passes in (`passes_beside`).
        let level = key_level(key.x);
        var axis = select(select(2u, 1u, cube_far.y <= cube_far.z), 0u, cube_far.x <= cube_far.y && cube_far.x <= cube_far.z);
        var local = vec3<i32>(0);
        var cell = vec3<i32>(0);
        var frac = vec3<f32>(0.0);
        if crosses {
            t_prev = max(t_leave, t_prev);
            if t_prev >= t_end { break; }
            let q = o + d * t_prev;
            local = clamp(vec3<i32>(floor(q)), cube_lo, cube_hi - 1);
            let step = select(cube_lo - 1, cube_hi, d > vec3<f32>(0.0));
            if axis == 0u { local.x = step.x; } else if axis == 1u { local.y = step.y; } else { local.z = step.z; }
            cell = key.yzw * 64 + local;
            stepped = axis;
            // Where in the next cell the ray enters: the side it crossed
            // on the exit axis.
            frac = clamp(fract(q), vec3<f32>(0.0), vec3<f32>(0.999));
            let entry = select(0.999, 0.0, d[axis] > 0.0);
            if axis == 0u { frac.x = entry; } else if axis == 1u { frac.y = entry; } else { frac.z = entry; }
        } else {
            let beside = passes_beside(o, d, cube_near, t_leave, t_prev);
            stepped = 3u;
            t_prev = beside.t;
            if t_prev >= t_end { break; }
            local = beside.cell;
            cell = key.yzw * 64 + local;
            frac = beside.frac;
        }
        // Past a face's edge: the face across it (a plane's edge ends it).
        var face = key.x & 7;
        let n = i32(params.table.y >> u32(level));
        let crossed = any(cell.xy < vec2<i32>(0)) || any(cell.xy >= vec2<i32>(n));
        if crossed {
            stepped = 3u;
            if PLANE_WORLD { break; }
            let across = across_edge(face, cell, frac, n);
            if !across.ok { break; }
            face = across.face;
            cell = across.cell;
            frac = across.frac;
        }
        let next = place_at(face, level, cell, frac, r, crossed);
        if next.slot != NONE {
            frame = chunk_frame(next.slot);
            key = chunk_keys[next.slot];
            cube_lo = vec3<i32>(0);
            cube_hi = vec3<i32>(64);
            real = true;
        } else {
            if next.level == LOST {
                WCOUNT(10)
                LOST_PIXEL
                break;
            }
            if next.level == BURIED || next.level == ROCK {
                // Rock, or the field's volume not generated yet (a Buried
                // node, reported so it is): a hit where the ray enters.
                ROCK_END
                if next.level == BURIED && counted {
                    let n = atomicAdd(&buried.count, 1u);
                    if n < arrayLength(&buried.keys) { buried.keys[n] = vec4<i32>(next.hi.x, next.lo); }
                }
                return WalkEnd(true, t_prev, slot, clamp(local, vec3<i32>(0), vec3<i32>(63)), axis, select(1u, 0u, d[axis] > 0.0), rock_pair(next));
            }
            // Air, crossed in this frame or the finer one given.
            if next.frame != NONE {
                frame = chunk_frame(next.frame);
                key = chunk_keys[next.frame];
            }
            cube_lo = to_frame(next.lo, next.level - (key_level(key.x)), key, true);
            cube_hi = clip_to_face(to_frame(next.hi, next.level - (key_level(key.x)), key, false), key);
            real = false;
        }
    }
    WCOUNT(8)
    WCOUNT_BOXES
    return WalkEnd(false, t_prev, NONE, vec3<i32>(0), 0u, 0u, 0u);
}


// Where a ray from hit face `f` along `dir` starts: past the cell steps
// a slope-lit face stands among (`Face::skip`), until it has risen a cell
// off the slope (12 cells at most: a grazing ray).
fn skip_along(f: Face, dir: vec3<f32>) -> f32 {
    if f.skip == 0.0 { return 0.0; }
    let rise = dot(dir, f.ground);
    return f.skip * f.cell * clamp(0.75 / max(rise, 1e-3), 1.0, 12.0);
}

// Face `f` with its skip along `dir` (`skip_along`) no longer than `reach`
// (m). A lamp's ray skips what lies within the hit's own pixel at most: a
// step farther along casts a shadow wider than a pixel (a cell-high
// riser's runs 1 / rise cells), which rough ground shows.
fn skip_within(f: Face, dir: vec3<f32>, reach: f32) -> Face {
    var g = f;
    let along = skip_along(f, dir);
    if along > reach { g.skip = f.skip * reach / along; }
    return g;
}

// WALK {
// Whether the hit at pixel `q` (distance `t`, face `f`) sees along `dir` to
// distance `range` (the sun: as far as the world goes; a light: to it).
// `through`: from the far side of the hit's cell (a light behind a material
// that lets light through reaches its face through the cell).
fn hit_walk(q: vec2<u32>, size: vec2<u32>, slot: u32, t: f32, f: Face, dir: vec3<f32>, range: f32, through: bool) -> f32 {
    let org = pixel_ray(q, size) * t + f.normal * (f.cell * select(0.01, -1.01, through));
    let end = walk(Walk(slot, chunk_keys[slot], vec3<i32>(0), vec3<i32>(64), true), org, dir, select(skip_along(f, dir), 0.0, through), range, fade_dither(q), 96u, false, q);
    return select(1.0, 0.0, end.hit);
}

// The share of the sky the hit at pixel `q` (distance `t`, face `f`, chunk
// `slot`) sees: cosine-weighted rays about the planet's up, turned each
// frame (the denoiser gathers them); an occluder near the range end shades less (no
// edge at the range). A ceiling looks out sideways. With GI probes, only
// the occlusion nearer than their spacing (`params.sky`).
fn sky_visibility(q: vec2<u32>, size: vec2<u32>, slot: u32, t: f32, f: Face) -> f32 {
    let at = pixel_ray(q, size) * t;
    let up = select(normalize(params.up.xyz * params.up.w + at), vec3<f32>(0.0, 1.0, 0.0), PLANE_WORLD);
    let ceiling = dot(f.normal, up) < -0.25;
    let t1 = normalize(select(cross(up, vec3<f32>(0.0, 0.0, 1.0)), cross(up, vec3<f32>(1.0, 0.0, 0.0)), abs(up.z) > 0.9));
    let t2 = cross(up, t1);
    var range = SKY_RANGE_CELLS * f.cell;
    // With GI the probes see the sky's occlusion past their spacing: these
    // rays keep the nearer (no less than a few cells).
    if params.sky.x > 0.0 { range = clamp(params.sky.x, 4.0 * f.cell, range); }
    let org = pixel_ray(q, size) * t + f.normal * (f.cell * 0.01);
    let turn = select(f32(params.table.w % 64u) * 0.618034, 0.0, steady_shadows());
    var open = 0.0;
    var total = 0.0;
    for (var i = 0u; i < SKY_RAYS; i++) {
        let u = fract((f32(i) + 0.5) / f32(SKY_RAYS) + turn);
        let phi = (f32(i) + turn * 7.0) * 2.39996323;
        let around = t1 * cos(phi) + t2 * sin(phi);
        var dir = normalize(around * sqrt(u) + up * sqrt(1.0 - u));
        if ceiling { dir = normalize(around - up * 0.02); }
        else if dot(dir, f.normal) <= 0.0 { continue; }
        total += 1.0;
        let end = walk(Walk(slot, chunk_keys[slot], vec3<i32>(0), vec3<i32>(64), true), org, dir, skip_along(f, dir), range, fade_dither(q), 24u, false, q);
        open += select(1.0, smoothstep(0.5, 1.0, end.t / range), end.hit && end.t < range);
    }
    return select(1.0, open / total, total > 0.0);
}
// } WALK

// Sky visibility samples, one a 4x4 block: visibility, the sample's
// distance (0: no hit).
@group(1) @binding(15) var out_sky_samples: texture_storage_2d<rg32float, write>;
@group(1) @binding(16) var sky_samples: texture_2d<f32>;

// The pixel 4x4 block `b` samples this frame (16 frames visit them all).
fn sky_pixel(b: vec2<u32>, size: vec2<u32>) -> vec2<u32> {
    let turn = select((params.table.w * 7u) & 15u, 5u, steady_shadows());
    return min(b * 4u + vec2<u32>(turn & 3u, turn >> 2u), size - 1u);
}

// One thread a 4x4 block: dispatch over a quarter of each side.
@compute @workgroup_size(8, 8)
fn sky_sample(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = rect_id(gid, 4u);
    let size = textureDimensions(hit_distance);
    let quarter = (size + 3u) / 4u;
    if id.x >= quarter.x || id.y >= quarter.y { return; }
    let q = sky_pixel(id.xy, size);
    let v = textureLoad(hit_visibility, vec2<i32>(q), 0).xy;
    if !pass_pixel(v.x) { return; }
    var sample = vec2<f32>(1.0, 0.0);
    if v.x != NONE {
        if exact_level(key_level(chunk_keys[v.x].x)) { return; }
        let t = textureLoad(hit_distance, vec2<i32>(q), 0).r;
        sample = vec2<f32>(sky_visibility_fine(q, size, v.x, t, hit_face(v, vec3<f32>(0.0), q, size, t)), t);
    }
    textureStore(out_sky_samples, vec2<i32>(id.xy), vec4<f32>(sample, 0.0, 0.0));
}

// `sky_sample` for the hits of exact levels (after it).
@compute @workgroup_size(8, 8)
fn sky_sample_far(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = rect_id(gid, 4u);
    let size = textureDimensions(hit_distance);
    let quarter = (size + 3u) / 4u;
    if id.x >= quarter.x || id.y >= quarter.y { return; }
    let q = sky_pixel(id.xy, size);
    let v = textureLoad(hit_visibility, vec2<i32>(q), 0).xy;
    if v.x == NONE || !ours(v.x) || !exact_level(key_level(chunk_keys[v.x].x)) { return; }
    var sample = vec2<f32>(1.0, 0.0);
    {
        let t = textureLoad(hit_distance, vec2<i32>(q), 0).r;
        sample = vec2<f32>(sky_visibility(q, size, v.x, t, hit_face(v, vec3<f32>(0.0), q, size, t)), t);
    }
    textureStore(out_sky_samples, vec2<i32>(id.xy), vec4<f32>(sample, 0.0, 0.0));
}

// A hit's sky visibility (at `at_hit`, pixel `p`): its 4x4 block's sample
// within `reach`, else the nearest such neighbour's, else its block's.
fn sky_at(p: vec2<i32>, at_hit: vec3<f32>, reach: f32, size: vec2<u32>) -> f32 {
    let quarter = vec2<i32>((size + 3u) / 4u);
    let block = p / 4;
    let toward = select(vec2<i32>(-1), vec2<i32>(1), (p & vec2<i32>(3)) >= vec2<i32>(2));
    let blocks = array<vec2<i32>, 4>(block, block + vec2<i32>(toward.x, 0), block + vec2<i32>(0, toward.y), block + toward);
    for (var b = 0u; b < 4u; b++) {
        let at = blocks[b];
        if any(at < vec2<i32>(0)) || any(at >= quarter) { continue; }
        let s = textureLoad(sky_samples, at, 0).xy;
        if s.y > 0.0 && distance(pixel_ray(sky_pixel(vec2<u32>(at), size), size) * s.y, at_hit) <= reach * 2.0 { return s.x; }
    }
    return textureLoad(sky_samples, block, 0).x;
}

// Whether hit face `f` at pixel `q` (distance `t`) is of cells under a
// pixel and a half: the sun's occlusion maps light it (`shadow`).
fn sub_pixel(f: Face, q: vec2<u32>, size: vec2<u32>, t: f32) -> bool {
    return f.cell < 1.5 * pixel_width(q, size, t);
}

// The pixel block `b` samples this frame.
fn sample_pixel(b: vec2<u32>, size: vec2<u32>) -> vec2<u32> {
    let turn = params.table.w & 3u;
    return min(b * 2u + vec2<u32>(turn & 1u, (turn >> 1u) ^ (turn & 1u)), size - 1u);
}

// One thread a 2x2 block: dispatch over half the size.
@compute @workgroup_size(8, 8)
fn shadow_sample(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = rect_id(gid, 2u);
    let size = textureDimensions(hit_distance);
    let half = (size + 1u) / 2u;
    if id.x >= half.x || id.y >= half.y { return; }
    let q = sample_pixel(id.xy, size);
    let v = textureLoad(hit_visibility, vec2<i32>(q), 0).xy;
    if !pass_pixel(v.x) { return; }
    var sample = vec4<u32>(0u);
    if v.x != NONE {
        if exact_level(key_level(chunk_keys[v.x].x)) { return; }
        let t = textureLoad(hit_distance, vec2<i32>(q), 0).r;
        let f = hit_face(v, vec3<f32>(0.0), q, size, t);
        sample = vec4<u32>(4u, bitcast<u32>(t), select(0u, pack_unit(f.ground), dot(f.ground, f.ground) > 0.0), 0u);
        if dot(f.shading, params.sun.xyz) > 0.0 && !sub_pixel(f, q, size, t) { sample.x |= 2u | u32(hit_walk_fine(q, size, v.x, t, f, sun_ray(sun_point(q)), 1e30, false)); }
    }
    textureStore(out_sun_samples, vec2<i32>(id.xy), sample);
}

// `shadow_sample` for the hits of exact levels (after it).
@compute @workgroup_size(8, 8)
fn shadow_sample_far(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = rect_id(gid, 2u);
    let size = textureDimensions(hit_distance);
    let half = (size + 1u) / 2u;
    if id.x >= half.x || id.y >= half.y { return; }
    let q = sample_pixel(id.xy, size);
    let v = textureLoad(hit_visibility, vec2<i32>(q), 0).xy;
    if v.x == NONE || !ours(v.x) || !exact_level(key_level(chunk_keys[v.x].x)) { return; }
    var sample = vec4<u32>(0u);
    {
        let t = textureLoad(hit_distance, vec2<i32>(q), 0).r;
        let f = hit_face(v, vec3<f32>(0.0), q, size, t);
        sample = vec4<u32>(4u, bitcast<u32>(t), select(0u, pack_unit(f.ground), dot(f.ground, f.ground) > 0.0), 0u);
        if dot(f.shading, params.sun.xyz) > 0.0 && !sub_pixel(f, q, size, t) { sample.x |= 2u | u32(hit_walk(q, size, v.x, t, f, sun_ray(sun_point(q)), 1e30, false)); }
    }
    textureStore(out_sun_samples, vec2<i32>(id.xy), sample);
}

// The scene's depth before the chunks publish theirs: the meshes'
// (reversed, 0 where none).
@group(1) @binding(27) var scene_depth: texture_depth_2d;

// A listed pixel whose mesh walks, not its hit (`shadow_fallback`).
const MESH_PIXEL: u32 = 0x8000u;

// The mesh point at pixel `q` (eye-relative), w 1; w 0 where no mesh.
fn mesh_point(q: vec2<u32>, size: vec2<u32>) -> vec4<f32> {
    let d = textureLoad(scene_depth, vec2<i32>(q), 0);
    if !(d > 0.0) { return vec4<f32>(0.0); }
    let ndc = vec2<f32>((f32(q.x) + 0.5) / f32(size.x) * 2.0 - 1.0, 1.0 - (f32(q.y) + 0.5) / f32(size.y) * 2.0);
    let h = params.inverse_view_proj * vec4<f32>(ndc, d, 1.0);
    return vec4<f32>(h.xyz / h.w, 1.0);
}

// Whether the mesh point `org` (eye-relative) at pixel `q` sees along `dir`
// to distance `range` (`point_walk`).
fn mesh_walk(q: vec2<u32>, org: vec3<f32>, v: vec2<u32>, dir: vec3<f32>, range: f32) -> f32 {
    return select(1.0, 0.0, point_walk(q, org, v, dir, range, false).hit);
}

// The chunk pixel `v`'s points are placed by: its hit's, else the one its
// ray ended in at its mesh or whose box past the mesh it had
// (`trace_pixel`), else NONE.
fn frame_chunk(v: vec2<u32>) -> u32 {
    return select(v.y - 1u, v.x, v.x != NONE);
}

// Where a walk from the point `org` (eye-relative) starts, for a pixel of
// dither `r`: the node of the cut it is in, placed in the frame of chunk
// `placed` (one near it, of this volume) to its cells' precision; without
// one finer than the exact levels, the exact level's node over it
// (planet-centred: to half a metre on an Earth). `kind`: 0 a walk, 1 in
// rock or a Buried node, 2 off the cut (past a corner, or a plane's edge).
struct PointStart {
    walk: Walk,
    kind: u32,
}

fn point_start(org: vec3<f32>, placed: u32, r: i32) -> PointStart {
    var out = PointStart(Walk(0u, vec4<i32>(0), vec3<i32>(0), vec3<i32>(0), false), 2u);
    if placed != NONE && ours(placed) && !exact_level(key_level(chunk_keys[placed].x)) {
        let key = chunk_keys[placed];
        let frame = chunk_frame(placed);
        let o = vec3<f32>(frame.i0.w, frame.i1.w, frame.i2.w) + vec3<f32>(dot(frame.i0.xyz, org), dot(frame.i1.xyz, org), dot(frame.i2.xyz, org));
        var face = key.x & 7;
        let level = key_level(key.x);
        var cell = key.yzw * 64 + vec3<i32>(floor(o));
        var frac = clamp(fract(o), vec3<f32>(0.0), vec3<f32>(0.999));
        // Past a face's edge: the face across it (past a corner, or a
        // plane's edge, nothing).
        let n = i32(params.table.y >> u32(level));
        let crossed = any(cell.xy < vec2<i32>(0)) || any(cell.xy >= vec2<i32>(n));
        if crossed {
            if PLANE_WORLD { return out; }
            let across = across_edge(face, cell, frac, n);
            if !across.ok { return out; }
            face = across.face;
            cell = across.cell;
            frac = across.frac;
        }
        let at = place_at(face, level, cell, frac, r, crossed);
        out.kind = 0u;
        if at.slot != NONE {
            out.walk = Walk(at.slot, chunk_keys[at.slot], vec3<i32>(0), vec3<i32>(64), true);
        } else if at.level == ROCK || at.level == BURIED {
            out.kind = 1u;
        } else if at.level == LOST {
            out.kind = 2u;
        } else if exact_level(at.level) {
            out.walk = Walk(placed, vec4<i32>(key_x(face, at.level), 0, 0, 0), at.lo, at.hi, false);
        } else {
            let f = select(placed, at.frame, at.frame != NONE);
            let fk = chunk_keys[f];
            out.walk = Walk(f, fk, to_frame(at.lo, at.level - (key_level(fk.x)), fk, true), clip_to_face(to_frame(at.hi, at.level - (key_level(fk.x)), fk, false), fk), false);
        }
    } else {
        if PLANE_WORLD { return out; }
        // The face the point is over (planet-centred).
        let c = params.up.xyz * params.up.w + org;
        let a = abs(c);
        var normal = vec3<i32>(0, 0, select(-1, 1, c.z > 0.0));
        if a.x >= a.y && a.x >= a.z {
            normal = vec3<i32>(select(-1, 1, c.x > 0.0), 0, 0);
        } else if a.y >= a.z {
            normal = vec3<i32>(0, select(-1, 1, c.y > 0.0), 0);
        }
        let level = min(i32(params.planet.z), i32(params.cut.x));
        if !exact_level(level) { return out; }
        out.walk.key = vec4<i32>(key_x(face_with_normal(normal), level), 0, 0, 0);
        out.kind = 0u;
    }
    return out;
}

// What the point `org` (eye-relative) at pixel `q` meets along `dir` within
// `range`: a walk from where it is (`point_start`, placed by pixel `v`'s
// chunk: `frame_chunk`). In rock or a Buried node, a hit at once with no
// chunk (slot NONE). `counted`: a primary ray's walk, which reports the
// Buried nodes it meets.
fn point_walk(q: vec2<u32>, org: vec3<f32>, v: vec2<u32>, dir: vec3<f32>, range: f32, counted: bool) -> WalkEnd {
    let rock = WalkEnd(true, 0.0, NONE, vec3<i32>(0), 0u, 0u, 0u);
    var none = rock;
    none.hit = false;
    let r = fade_dither(q);
    let placed = frame_chunk(v);
    let start = point_start(org, placed, r);
    if start.kind == 1u { return rock; }
    if start.kind == 2u { return none; }
    // Off the surface a mesh stands on: a twentieth of the hit's cell.
    var t0 = 0.0;
    if placed != NONE && ours(placed) && !exact_level(key_level(chunk_keys[placed].x)) { t0 = 0.05 * length(chunk_frame(placed).m0.xyz); }
    return walk(start.walk, org, dir, t0, range, r, 96u, counted, q);
}

// ---- Walks into the volume from anywhere ----
// Points of other volumes (their hits and meshes in shadow passes, GI
// probes, foliage queries) see this pass's volume by walking it from where
// their ray enters it: a sphere's ceiling ball, a plane's edges between its
// band's bottom and its ceiling (`params.ceiling`). The walk starts there as a point's
// does (`point_start`): placed by the anchor's frame (`params.cut.w`: a
// drawn chunk of the volume, near the eye on a sphere; NONE without one),
// a plane's anywhere (its frames are one affine map), a sphere's within
// ANCHOR_REACH of its cells (its frames stray from the sphere farther out);
// else by a sphere's exact levels.
struct Entry {
    walk: Walk,
    t: f32,
    // 0: a walk from t; 1: in rock or a Buried node at t; 2: the ray
    // misses the volume (or nothing is drawn where it enters).
    kind: u32,
}

const ANCHOR_REACH: f32 = 512.0;

fn enter_volume(org: vec3<f32>, dir: vec3<f32>, t_end: f32, r: i32) -> Entry {
    var out = Entry(Walk(0u, vec4<i32>(0), vec3<i32>(0), vec3<i32>(0), false), 0.0, 2u);
    let anchor = params.cut.w;
    var t0 = 0.0;
    var t1 = t_end;
    if PLANE_WORLD {
        if anchor == NONE { return out; }
        // Between the band's bottom and the ceiling (eye-relative).
        let bottom_y = -params.ceiling.z;
        let ceiling_y = -params.ceiling.x;
        if dir.y != 0.0 {
            let ta = (bottom_y - org.y) / dir.y;
            let tb = (ceiling_y - org.y) / dir.y;
            t0 = max(t0, min(ta, tb));
            t1 = min(t1, max(ta, tb));
        } else if org.y < bottom_y || org.y > ceiling_y {
            return out;
        }
        // Within its edges, in the anchor's cells.
        let f = chunk_frame(anchor);
        let key = chunk_keys[anchor];
        let o = vec3<f32>(f.i0.w, f.i1.w, f.i2.w) + vec3<f32>(dot(f.i0.xyz, org), dot(f.i1.xyz, org), dot(f.i2.xyz, org));
        let d = vec3<f32>(dot(f.i0.xyz, dir), dot(f.i1.xyz, dir), dot(f.i2.xyz, dir));
        let lo = -vec2<f32>(key.yz * 64);
        let hi = lo + f32(params.table.y >> u32(key_level(key.x)));
        var entered_by = 3u;
        for (var a = 0u; a < 2u; a++) {
            if d[a] != 0.0 {
                let ta = (lo[a] - o[a]) / d[a];
                let tb = (hi[a] - o[a]) / d[a];
                if min(ta, tb) > t0 {
                    t0 = min(ta, tb);
                    entered_by = a;
                }
                t1 = min(t1, max(ta, tb));
            } else if o[a] < lo[a] || o[a] >= hi[a] {
                return out;
            }
        }
        // Into the cells it enters entered_by (rounding would place it beside).
        if entered_by < 2u { t0 += 1e-3 / abs(d[entered_by]); }
    } else {
        // In the ceiling's ball: |c|^2 less its squared radius from the
        // eye's (`params.ceiling.x`, from f64), as `exact_ray`'s.
        let e = params.up.xyz * params.up.w;
        let b = dot(e, dir) + dot(org, dir);
        let c = params.ceiling.x + 2.0 * dot(e, org) + dot(org, org);
        let disc = b * b - c;
        if disc < 0.0 { return out; }
        let s = sqrt(disc);
        t0 = max(t0, -b - s);
        t1 = min(t1, -b + s);
    }
    if t1 <= t0 { return out; }
    let p = org + dir * t0;
    var placed = anchor;
    if !PLANE_WORLD && anchor != NONE {
        let f = chunk_frame(anchor);
        let o = vec3<f32>(f.i0.w, f.i1.w, f.i2.w) + vec3<f32>(dot(f.i0.xyz, p), dot(f.i1.xyz, p), dot(f.i2.xyz, p));
        if any(abs(o - 32.0) > vec3<f32>(ANCHOR_REACH)) { placed = NONE; }
    }
    let start = point_start(p, placed, r);
    out.walk = start.walk;
    out.t = t0;
    out.kind = start.kind;
    return out;
}

// Whether a point `org` (eye-relative) sees along `dir` past this pass's
// volume (`enter_volume`), for a pixel `q` of dither `r`.
fn sees_past(org: vec3<f32>, dir: vec3<f32>, r: i32, q: vec2<u32>) -> f32 {
    let e = enter_volume(org, dir, 1e30, r);
    if e.kind == 2u { return 1.0; }
    if e.kind == 1u { return 0.0; }
    return select(1.0, 0.0, walk(e.walk, org, dir, e.t, 1e30, r, 96u, false, q).hit);
}

// The share of each pixel's light its sun samples give stochastically
// (helio-pass-tsr's STOCHASTIC_MASK: TSR integrates it rather than reject
// history): a terrain hit facing the sun whose cells are one and a half to
// three pixels wide, where the 2x2 blocks' rotating samples and the sun
// disk's points cross cells the denoiser cannot tell apart. Steady
// shadows, meshes and smaller cells (`sun_occlusion`), faces turned from
// the sun and larger cells: none.
@group(1) @binding(64) var out_stochastic: texture_storage_2d<rgba8unorm, write>;

// The sun's occlusion pages this frame's pixels read (a bit a page of each
// level's window: `ChunkWorld::draw_sun_occlusion` reads them back and
// traces those missing).
@group(1) @binding(66) var<storage, read_write> sun_requests: array<atomic<u32>>;

// Asks for the pages camera-relative `p` reads at `footprint` (m): its
// level's (`sun_occlusion_level`) under its filter's taps, in the current
// epoch.
fn ask_sun_pages(p: vec3<f32>, footprint: f32) {
    if sun_occlusion_frame.epochs[0].right.w == 0.0 { return; }
    let level = sun_occlusion_level(footprint);
    let at = sun_occlusion_texel(p, 0u, level);
    let base = at.texel + select(vec2<i32>(0), vec2<i32>(-1), at.frac < vec2<f32>(0.5));
    let eye = sun_occlusion_frame.epochs[0].levels[level].texel >> vec2<u32>(SUN_OCCLUSION_PAGE_SHIFT);
    let side = 1 << SUN_OCCLUSION_WINDOW_SHIFT;
    let lo = (base >> vec2<u32>(SUN_OCCLUSION_PAGE_SHIFT)) - eye + side / 2;
    let hi = ((base + 1) >> vec2<u32>(SUN_OCCLUSION_PAGE_SHIFT)) - eye + side / 2;
    for (var y = lo.y; y <= hi.y; y++) {
        for (var x = lo.x; x <= hi.x; x++) {
            if x < 0 || y < 0 || x >= side || y >= side { continue; }
            let bit = (level * u32(side) + u32(y)) * u32(side) + u32(x);
            let mask = 1u << (bit & 31u);
            if (atomicLoad(&sun_requests[bit >> 5u]) & mask) == 0u {
                atomicOr(&sun_requests[bit >> 5u], mask);
            }
        }
    }
}

// The sun at camera-relative `p` (receiver normal `n`) by its occlusion
// pages, for pixel `q` `footprint` m wide, asked for (by one pixel of each
// 2x2 block a frame, turning: a page spans dozens of pixels): the share
// from the first level resident from the footprint's up to `coarser`
// levels coarser (no further looked at), or -1 where none is (the pixel
// walks).
fn sun_by_pages(p: vec3<f32>, n: vec3<f32>, footprint: f32, q: vec2<u32>, coarser: u32) -> f32 {
    let turn = params.table.w & 3u;
    if all((q & vec2<u32>(1u)) == vec2<u32>(turn & 1u, turn >> 1u)) {
        ask_sun_pages(p, footprint);
    }
    let seen = sun_occlusion_at(p, n, footprint, coarser);
    return select(-1.0, seen.x, seen.y > 0.0);
}

fn stochastic_share(p: vec2<i32>, size: vec2<u32>, v: vec2<u32>, mesh: vec4<f32>) -> f32 {
    if v.x == NONE || steady_shadows() { return 0.0; }
    let t = textureLoad(hit_distance, p, 0).r;
    if mesh.w > 0.0 && length(mesh.xyz) < t { return 0.0; }
    let c = chunk_frame(v.x);
    let axis = (v.y >> 18u) & 3u;
    let n = select(select(c.m2.xyz, c.m1.xyz, axis == 1u), c.m0.xyz, axis == 0u) * select(-1.0, 1.0, ((v.y >> 20u) & 1u) == 1u);
    if dot(n, params.sun.xyz) <= 0.0 { return 0.0; }
    let cells = length(c.m0.xyz) / max(pixel_width(vec2<u32>(p), size, t), 1e-6);
    return select(1.0 - smoothstep(1.5, 3.0, cells), 0.0, cells < 1.5);
}

@compute @workgroup_size(8, 8)
fn shadow(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = rect_id(gid, 1u);
    let size = textureDimensions(hit_distance);
    if id.x >= size.x || id.y >= size.y { return; }
    let p = vec2<i32>(id.xy);
    let sun = params.sun.xyz;
    let v = textureLoad(hit_visibility, p, 0).xy;
    if !pass_pixel(v.x) { return; }
    textureStore(out_stochastic, p, vec4<f32>(stochastic_share(p, size, v, mesh_point(id.xy, size)), 0.0, 0.0, 0.0));
    // A mesh in front of the hit (or the sky): the sun's occlusion pages
    // at its pixel's footprint (a texture read where a walk cost a pixel's
    // own, and steady from frame to frame: shared walks flicker across
    // grass); where they are not there yet, its own walk, listed.
    let mesh = mesh_point(id.xy, size);
    if mesh.w > 0.0 && (v.x == NONE || length(mesh.xyz) < textureLoad(hit_distance, p, 0).r) {
        textureStore(out_normal, p, vec4<f32>(0.0));
        // Meshes take a level at most one coarser (grass near the eye walks
        // rather than read texels four times its pixels).
        let seen = sun_by_pages(to_world(mesh.xyz), vec3<f32>(0.0), pixel_width(id.xy, size, length(mesh.xyz)), id.xy, 1u);
        if seen >= 0.0 && !steady_shadows() {
            store_sun(p, size, seen, 1.0, 0.0, true);
            return;
        }
        let n = atomicAdd(&fallbacks.count, 1u);
        fallbacks.pixels[n] = id.x | MESH_PIXEL | (id.y << 16u);
        atomicMax(&fallback_groups[0], n / 64u + 1u);
        return;
    }
    if v.x == NONE {
        textureStore(out_normal, p, vec4<f32>(0.0));
        store_walked(p, size, 1.0, 1.0, 0.0);
        return;
    }
    let t = textureLoad(hit_distance, p, 0).r;
    // Its block's sample, then the nearer neighbours' (towards the pixel's
    // side of its block): the first within four of its cells (or eight
    // pixels' widths: cells under a pixel, a grazing slope) lends its
    // slope's normal, the first of those that walked its sunlight.
    let at_hit = pixel_ray(id.xy, size) * t;
    let reach = max(4.0 * length(chunk_frame(v.x).m0.xyz), 8.0 * pixel_width(id.xy, size, t));
    let half = vec2<i32>((size + 1u) / 2u);
    let block = p / 2;
    let toward = vec2<i32>(p & vec2<i32>(1)) * 2 - 1;
    let blocks = array<vec2<i32>, 4>(block, block + vec2<i32>(toward.x, 0), block + vec2<i32>(0, toward.y), block + toward);
    var ground = vec3<f32>(0.0);
    var sampled = 0u;
    var own = 0.0;
    let steady = steady_shadows();
    for (var b = 0u; b < 4u; b++) {
        let at = blocks[b];
        if steady || any(at < vec2<i32>(0)) || any(at >= half) { continue; }
        let s = textureLoad(sun_samples, at, 0);
        if (s.x & 4u) == 0u || distance(pixel_ray(sample_pixel(vec2<u32>(at), size), size) * bitcast<f32>(s.y), at_hit) > reach { continue; }
        if dot(ground, ground) == 0.0 && s.z != 0u { ground = unpack_unit(s.z); }
        if sampled == 0u && (s.x & 2u) != 0u {
            sampled = 2u | (s.x & 1u);
            own = select(0.0, 1.0, all(sample_pixel(vec2<u32>(at), size) == id.xy));
        }
    }
    let f = hit_face(v, ground, id.xy, size, t);
    // Lit in the world (the denoiser's and lighting's frame).
    textureStore(out_normal, p, vec4<f32>(to_world(f.shading), sky_at(p, at_hit, reach, size)));
    var lit = 0.0;
    var walks = false;
    var paged = false;
    let footprint = pixel_width(id.xy, size, t);
    if dot(f.shading, sun) <= 0.0 {
        own = 1.0;
        // A material that lets light through, its back to the sun: the
        // light through it (its share) where the sun reaches its far side,
        // by the sun's occlusion pages (as GI's hits: a texture read,
        // steady from frame to frame).
        let through = materials.m[min((v.y >> 21u) & 31u, 31u)].light.x;
        if dot(f.normal, sun) < 0.0 && through > 0.0 {
            let back = to_world(at_hit - f.normal * (1.5 * f.cell));
            ask_sun_pages(back, footprint);
            lit = clamp(through, 0.0, 1.0) * sun_occlusion(back, vec3<f32>(0.0), footprint);
            paged = true;
        }
    } else if sub_pixel(f, id.xy, size, t) && !steady {
        // Cells under a pixel and a half: their own sun is aliasing (each
        // frame's jitter and samples land on other cells), so the sun's
        // occlusion pages', steady from frame to frame, their texels about
        // the pixel's footprint; against the slope's plane, so the steps
        // across a texel do not shade it, and from as far up the sun as a
        // walk starts (past the steps it stands among). Not there yet: its
        // own walk.
        let lifted = to_world(at_hit + f.normal * (0.5 * f.cell) + sun * skip_along(f, sun));
        // Any level resident: while a page comes the pixel reads a coarser
        // one (as virtual shadow maps fall back) rather than walk.
        lit = sun_by_pages(lifted, to_world(f.shading), footprint, id.xy, 32u);
        own = 1.0;
        walks = lit < 0.0;
        paged = !walks;
    } else {
        lit = f32(sampled & 1u);
        walks = sampled == 0u;
    }
    if walks {
        let n = atomicAdd(&fallbacks.count, 1u);
        fallbacks.pixels[n] = id.x | (id.y << 16u);
        atomicMax(&fallback_groups[0], n / 64u + 1u);
        return;
    }
    store_sun(p, size, lit, own, 1.0, paged);
}

// The listed hits' and meshes' own walks: dispatched indirectly, 64 a
// workgroup.
@compute @workgroup_size(64)
fn shadow_fallback(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= atomicLoad(&fallbacks.count) { return; }
    let packed = fallbacks.pixels[id.x];
    let q = vec2<u32>(packed & 0x7fffu, packed >> 16u);
    let size = textureDimensions(hit_distance);
    let v = textureLoad(hit_visibility, vec2<i32>(q), 0).xy;
    if !pass_pixel(v.x) { return; }
    if (packed & MESH_PIXEL) != 0u {
        store_walked(vec2<i32>(q), size, mesh_walk(q, mesh_point(q, size).xyz, v, params.sun.xyz, 1e30), 1.0, 0.0);
        return;
    }
    let t = textureLoad(hit_distance, vec2<i32>(q), 0).r;
    let f = hit_face(v, vec3<f32>(0.0), q, size, t);
    // Steady shadows: the disk's points averaged (the reference).
    var lit = 0.0;
    if steady_shadows() {
        for (var i = 0u; i < SUN_DISK_RAYS; i++) {
            lit += hit_walk(q, size, v.x, t, f, sun_ray(sun_disk_point(i)), 1e30, false);
        }
        lit /= f32(SUN_DISK_RAYS);
    } else {
        lit = hit_walk(q, size, v.x, t, f, sun_ray(sun_point(q)), 1e30, false);
    }
    store_walked(vec2<i32>(q), size, lit, 1.0, 1.0);
}

// Shadows across volumes: once every hit has its own volume's sun, each
// volume that can shade another's (`ChunkWorld::draw`: a pass a caster,
// over the screen rectangle its shadow can fall on) walks from every other
// volume's lit hit, and from the meshes the first pass did not walk,
// towards the sun (the same ray as their own walk: the disk's points with
// steady shadows) from where the ray enters it (`enter_volume`); a hit
// shades the pixel. Pixels the sun's occlusion pages lit are left: every
// volume is traced into them.
@compute @workgroup_size(8, 8)
fn shadow_cross(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = rect_id(gid, 1u);
    let size = textureDimensions(hit_distance);
    if id.x >= size.x || id.y >= size.y { return; }
    let p = vec2<i32>(id.xy);
    let at = u32(p.y) * size.x + u32(p.x);
    let word = unpack4x8unorm(out_sun_raw[at]);
    if word.x == 0.0 || word.w > 0.5 { return; }
    let v = textureLoad(hit_visibility, p, 0).xy;
    var org = vec3<f32>(0.0);
    let mesh = mesh_point(id.xy, size);
    if mesh.w > 0.0 && (v.x == NONE || length(mesh.xyz) < textureLoad(hit_distance, p, 0).r) {
        if params.volume.y == 0u { return; }
        org = mesh.xyz;
    } else {
        if v.x == NONE || ours(v.x) { return; }
        org = pixel_ray(id.xy, size) * textureLoad(hit_distance, p, 0).r;
    }
    let r = fade_dither(id.xy);
    // One walk in the kernel (each call site of a walk is a long compile).
    let steady = steady_shadows();
    let rays = select(1u, SUN_DISK_RAYS, steady);
    var lit = 0.0;
    for (var i = 0u; i < rays; i++) {
        let u = select(sun_point(id.xy), sun_disk_point(i), steady);
        lit += sees_past(org, sun_ray(u), r, id.xy);
    }
    lit /= f32(rays);
    if lit >= 1.0 { return; }
    out_sun_raw[at] = pack4x8unorm(vec4<f32>(word.x * lit, 1.0, word.z, 0.0));
}

// ---- 4b. Local lights' terrain shadows ----
// The terrain shadows the scene's point and spot lights the way it does the
// sun: meshes cast into the lights' shadow maps, the terrain by walks. A
// pixel's lights are those of its tile's list (Helio's light culling: 16 px
// tiles, 64 lights a tile) that are shadowed and reach it; bit i of its
// first word of `local_seen` (a row of the frame after another) says whether it
// sees the tile's light i past the terrain (the first 32; the others, and
// pixels without terrain, are seen); the denoiser (4c) turns those into
// what deferred lighting multiplies each light by. As the sun's: a hit of each 2x2 block (turning each
// frame) walks to all its lights (`local_sample`), its block's hits within
// reach take its word, the others (and meshes in front of the terrain)
// walk their own.
@group(1) @binding(29) var hit_normals: texture_2d<f32>;
// The second: 1 where the word is the pixel's own walks', 0 where a
// neighbour's.
@group(1) @binding(30) var<storage, read_write> local_seen: array<vec2<u32>>;
// Helio's lights (`GpuLight`, 128 bytes) and its tile light lists.
struct SceneLight {
    position_range: vec4<f32>,
    direction_outer: vec4<f32>,
    color_intensity: vec4<f32>,
    shadow_index: u32,
    light_type: u32,
    inner_angle: f32,
    flags: u32,
    rest: array<vec4<u32>, 4>,
}
@group(1) @binding(31) var<storage, read> scene_lights: array<SceneLight>;
@group(1) @binding(32) var<storage, read> tile_light_lists: array<u32>;
@group(1) @binding(33) var<storage, read> tile_light_counts: array<u32>;
// A sample a 2x2 block (`local_sample`, rows of the half frame): its word,
// its hit's distance (0: none).
@group(1) @binding(34) var<storage, read_write> local_samples: array<vec2<u32>>;
const LIGHT_TILE: u32 = 16u;
const LIGHTS_PER_TILE: u32 = 64u;
const LOCAL_LIGHTS: u32 = 32u;

// Whether light `light` (in a tile's list) wants the terrain's shadow at
// eye-relative point `at`: a shadowed point or spot light that reaches it,
// toward which the surface (normal `n`, none: any) faces, or that is behind a
// surface that lets light through (`transmits`: two-sided foliage, lit from
// behind through its cell: `back`). Its direction and distance from the point.
struct LightReach {
    walk: bool,
    dir: vec3<f32>,
    range: f32,
    back: bool,
}

fn light_reach(light: SceneLight, at: vec3<f32>, n: vec3<f32>, transmits: bool) -> LightReach {
    var none: LightReach;
    // Shadows as the deferred lighting requests them (`shadow_factor`).
    let requested = select(light.shadow_index != 0xffffffffu, (light.flags & 4u) != 0u, (light.flags & 8u) != 0u);
    if light.light_type == 0u || !requested || (light.flags & 48u) == 48u { return none; }
    // The light in the volume's frame (eye-relative).
    let to = to_local(light.position_range.xyz - params.light_eye.xyz) - at;
    let range = length(to);
    if range > light.position_range.w || range < 1e-4 { return none; }
    let dir = to / range;
    let back = dot(n, n) > 0.0 && dot(n, dir) <= 0.0;
    if back && !transmits { return none; }
    if light.light_type == 2u && dot(-dir, normalize(to_local(light.direction_outer.xyz))) < light.direction_outer.w { return none; }
    return LightReach(true, dir, range, back);
}

// The tile list of pixel `p` (its first entry, its count up to 32).
fn light_tile(p: vec2<u32>, size: vec2<u32>) -> vec2<u32> {
    let tiles_x = (size.x + LIGHT_TILE - 1u) / LIGHT_TILE;
    let tile = (p.y / LIGHT_TILE) * tiles_x + p.x / LIGHT_TILE;
    return vec2<u32>(tile * LIGHTS_PER_TILE, min(tile_light_counts[tile], LOCAL_LIGHTS));
}

// WALK {
// The local lights the hit at pixel `q` (chunk `slot`, distance `t`) sees.
fn hit_lights(q: vec2<u32>, size: vec2<u32>, slot: u32, t: f32) -> u32 {
    let list = light_tile(q, size);
    let at = pixel_ray(q, size) * t;
    let n = to_local(textureLoad(hit_normals, vec2<i32>(q), 0).xyz);
    let v = textureLoad(hit_visibility, vec2<i32>(q), 0).xy;
    let transmits = materials.m[min((v.y >> 21u) & 31u, 31u)].light.x > 0.0;
    var seen = 0xffffffffu;
    var face: Face;
    var faced = false;
    for (var i = 0u; i < list.y; i++) {
        let reach = light_reach(scene_lights[tile_light_lists[list.x + i]], at, n, transmits);
        if !reach.walk { continue; }
        if !faced {
            face = hit_face(v, vec3<f32>(0.0), q, size, t);
            faced = true;
        }
        // Short of the light: the cell a lamp stands in does not shade it.
        var lit = face;
        lit.skip = face.light_skip;
        lit = skip_within(lit, reach.dir, pixel_width(q, size, t));
        if hit_walk(q, size, slot, t, lit, reach.dir, reach.range - face.cell, reach.back) < 0.5 { seen &= ~(1u << i); }
    }
    return seen;
}
// } WALK

// Whether pixel `p`'s hit is the terrain's (no mesh in front of it), and
// of a fine level (`fine`) or an exact one.
fn local_hit(p: vec2<u32>, size: vec2<u32>, v: vec2<u32>, t: f32, fine: bool) -> bool {
    if v.x == NONE || !ours(v.x) || exact_level(key_level(chunk_keys[v.x].x)) == fine { return false; }
    let mesh = mesh_point(p, size);
    return !(mesh.w > 0.0 && length(mesh.xyz) < t);
}

// The hit of block `b` this frame (`sample_pixel`): its word and distance.
fn store_local_sample(b: vec2<u32>, size: vec2<u32>, fine: bool) {
    let half = (size + 1u) / 2u;
    let q = sample_pixel(b, size);
    let v = textureLoad(hit_visibility, vec2<i32>(q), 0).xy;
    let t = textureLoad(hit_distance, vec2<i32>(q), 0).r;
    if !local_hit(q, size, v, t, fine) {
        // The other kernel's, or none (a mesh, the sky); another volume's
        // pass writes its own.
        if fine && pass_pixel(v.x) { local_samples[b.y * half.x + b.x] = vec2<u32>(0xffffffffu, 0u); }
        return;
    }
    var seen = 0xffffffffu;
    if fine { seen = hit_lights_fine(q, size, v.x, t); } else { seen = hit_lights(q, size, v.x, t); }
    local_samples[b.y * half.x + b.x] = vec2<u32>(seen, bitcast<u32>(t));
}

// One thread a 2x2 block: dispatch over half the size (the fine levels'
// hits, then the exact levels').
@compute @workgroup_size(8, 8)
fn local_sample(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = rect_id(gid, 2u);
    let size = textureDimensions(hit_distance);
    let half = (size + 1u) / 2u;
    if id.x >= half.x || id.y >= half.y || params.light_eye.w == 0.0 { return; }
    store_local_sample(id.xy, size, true);
}

@compute @workgroup_size(8, 8)
fn local_sample_far(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = rect_id(gid, 2u);
    let size = textureDimensions(hit_distance);
    let half = (size + 1u) / 2u;
    if id.x >= half.x || id.y >= half.y || params.light_eye.w == 0.0 { return; }
    store_local_sample(id.xy, size, false);
}

// Pixel `p`'s hit (distance `t`, chunk `slot`) from its block's sample, or
// its nearer neighbours' (towards its side of its block): the first within
// reach of it (a slope's cells, a pixel's width), else NONE (steady
// shadows: none).
fn local_reuse(p: vec2<u32>, size: vec2<u32>, slot: u32, t: f32) -> u32 {
    if steady_shadows() { return NONE; }
    let half = vec2<i32>((size + 1u) / 2u);
    let at_hit = pixel_ray(p, size) * t;
    let reach = max(4.0 * length(chunk_frame(slot).m0.xyz), 8.0 * pixel_width(p, size, t));
    let block = vec2<i32>(p) / 2;
    let toward = vec2<i32>(vec2<i32>(p) & vec2<i32>(1)) * 2 - 1;
    let blocks = array<vec2<i32>, 4>(block, block + vec2<i32>(toward.x, 0), block + vec2<i32>(0, toward.y), block + toward);
    for (var b = 0u; b < 4u; b++) {
        let at = blocks[b];
        // Blocks of another light tile list another tile's lights.
        if any(at < vec2<i32>(0)) || any(at >= half) || any(at * 2 / i32(LIGHT_TILE) != vec2<i32>(p) / i32(LIGHT_TILE)) { continue; }
        let s = local_samples[u32(at.y) * u32(half.x) + u32(at.x)];
        if s.y != 0u && distance(pixel_ray(sample_pixel(vec2<u32>(at), size), size) * bitcast<f32>(s.y), at_hit) <= reach { return s.x; }
    }
    return NONE;
}

// `local_reuse`, and whether the word is the pixel's own (its block's
// sample this frame, or NONE: its walks to come).
fn local_word(p: vec2<u32>, size: vec2<u32>, slot: u32, t: f32) -> vec2<u32> {
    let word = local_reuse(p, size, slot, t);
    return vec2<u32>(word, u32(word == NONE || all(sample_pixel(p / 2u, size) == p)));
}

// The fine levels' hits (no mesh in front of them): a sample's word, else
// their own walks.
@compute @workgroup_size(8, 8)
fn local_shadow(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = rect_id(gid, 1u);
    let size = textureDimensions(hit_distance);
    if id.x >= size.x || id.y >= size.y { return; }
    let v = textureLoad(hit_visibility, vec2<i32>(id.xy), 0).xy;
    let t = textureLoad(hit_distance, vec2<i32>(id.xy), 0).r;
    if !local_hit(id.xy, size, v, t, true) { return; }
    var seen = vec2<u32>(0xffffffffu, 1u);
    if params.light_eye.w > 0.0 {
        seen = local_word(id.xy, size, v.x, t);
        if seen.x == NONE { seen.x = hit_lights_fine(id.xy, size, v.x, t); }
    }
    local_seen[id.y * size.x + id.x] = seen;
}

// Every other pixel (after `local_shadow`): the exact levels' hits (as the
// fine ones), meshes in front of the terrain (their own walks), the sky
// (all seen).
@compute @workgroup_size(8, 8)
fn local_shadow_far(@builtin(global_invocation_id) gid: vec3<u32>) {
    let id = rect_id(gid, 1u);
    let size = textureDimensions(hit_distance);
    if id.x >= size.x || id.y >= size.y { return; }
    let p = vec2<i32>(id.xy);
    let v = textureLoad(hit_visibility, p, 0).xy;
    if !pass_pixel(v.x) { return; }
    let t = textureLoad(hit_distance, p, 0).r;
    if local_hit(id.xy, size, v, t, true) { return; }
    var seen = vec2<u32>(0xffffffffu, 1u);
    if params.light_eye.w > 0.0 {
        let mesh = mesh_point(id.xy, size);
        if mesh.w > 0.0 && (v.x == NONE || length(mesh.xyz) < t) {
            let list = light_tile(id.xy, size);
            for (var i = 0u; i < list.y; i++) {
                let reach = light_reach(scene_lights[tile_light_lists[list.x + i]], mesh.xyz, vec3<f32>(0.0), false);
                if reach.walk && mesh_walk(id.xy, mesh.xyz, v, reach.dir, reach.range) < 0.5 { seen.x &= ~(1u << i); }
            }
        } else if v.x != NONE {
            seen = local_word(id.xy, size, v.x, t);
            if seen.x == NONE { seen.x = hit_lights(id.xy, size, v.x, t); }
        }
    }
    local_seen[id.y * size.x + id.x] = seen;
}

// ---- 4c. Denoising ----
// The walks above sample: a sun ray a 2x2 block, sky rays a 4x4 block,
// local lights a 2x2 block, the sampled pixel turning each frame, every
// hit taking a sample near it. These passes turn that into each term's
// visibility (cf. NVIDIA's SIGMA): a temporal accumulation of each term
// along the hits' motion, filters guided by the terms' variance that keep
// to a surface (its plane and normal), and a resolve to where this frame's
// jitter put each pixel's sample.
//
// Each term keeps a long history (stable) and a short one (a sample
// block's cycle). A pixel's own sample (its walk, once a cycle) weighs more
// than one lent by a neighbour, so the history keeps to the pixel's own
// shadow, sharp. The sun's and the lights' histories are clamped to the
// range of what the samples around show this frame, so a moved light or
// sun, an edit, shows at once where they all agree; where the short history
// leaves the long one (beside a moved shadow's edge; the sky's anywhere),
// the long one restarts from it. Steady shadows (every hit its own walk)
// pass the walks' own.
//
// Kept two frames in turn, two words a pixel (rows of the frame): `history`,
// the sun's mean, the sky's mean and second moment, the sky's frames
// accumulated (halves); `history_short`, the short histories and the sun's
// samples' weight accumulated (halves); `geometry`, the hits they are for:
// view depth (f32 bits, 0: no terrain hit) and normal (`pack_unit`). The
// filters read only `geometry` and their terms (the means and their
// variance, halves).

// Frames a term accumulates at most: the sun's samples visit a block's
// pixels in 4 frames, the sky's in 16 (their rays turn over 64); the short
// histories' (a cycle).
const SUN_FRAMES: f32 = 32.0;
// A pixel's sun sample weighs 1 where it is its own, this where a
// neighbour's; a frame's mean weight.
const SUN_BORROWED: f32 = 0.05;
const SUN_FRAME_WEIGHT: f32 = 0.25 + 0.75 * SUN_BORROWED;
const SKY_FRAMES: f32 = 64.0;
const SUN_SHORT: f32 = 4.0;
const SKY_SHORT: f32 = 8.0;
// How far the long history may stray from the short one before it
// restarts from it (a cycle's own swing stays inside).
const SUN_LAG: f32 = 0.3;
const SKY_LAG: f32 = 0.25;
// A pixel's neighbourhood for the history's clamp: 9x9 pixels, a few of
// each term's sample blocks. (The sky's samples, four rays a 4x4 block,
// swing too far from frame to frame for a clamp to their neighbourhood.)
const CLAMP_RADIUS: i32 = 4;

// The walked sun (`shadow`, `shadow_fallback`): x lit, y 1 where the
// pixel's own walk's, z 1 where a terrain hit is shown (no mesh in front
// of it, not the sky).
@group(1) @binding(40) var<storage, read> sun_raw: array<u32>;
@group(1) @binding(42) var<storage, read> history_prev: array<vec2<u32>>;
@group(1) @binding(43) var<storage, read> geometry_prev: array<vec2<u32>>;
@group(1) @binding(44) var<storage, read_write> out_history: array<vec2<u32>>;
@group(1) @binding(45) var<storage, read_write> out_geometry: array<vec2<u32>>;
@group(1) @binding(54) var<storage, read> history_short_prev: array<vec2<u32>>;
@group(1) @binding(55) var<storage, read_write> out_history_short: array<vec2<u32>>;
@group(1) @binding(46) var<storage, read> filter_in: array<vec2<u32>>;
@group(1) @binding(47) var<storage, read> geometry: array<vec2<u32>>;
@group(1) @binding(48) var<storage, read_write> out_filter: array<vec2<u32>>;

// Pixel `q`'s word in a frame's rows.
fn texel(q: vec2<i32>, size: vec2<u32>) -> u32 {
    return u32(q.y) * size.x + u32(q.x);
}

fn halves(v: vec2<u32>) -> vec4<f32> {
    return vec4<f32>(unpack2x16float(v.x), unpack2x16float(v.y));
}

fn to_halves(v: vec4<f32>) -> vec2<u32> {
    return vec2<u32>(pack2x16float(v.xy), pack2x16float(v.zw));
}

// Pixel `q`'s walked sun: lit, own, terrain.
fn walked_sun(q: vec2<i32>, size: vec2<u32>) -> vec3<f32> {
    return unpack4x8unorm(sun_raw[texel(q, size)]).xyz;
}
@group(1) @binding(49) var out_sky: texture_storage_2d<r32float, write>;

// A pixel's hit as the filters see it: eye-relative, its view depth (0:
// none to filter), normal, width (m).
struct Surface {
    at: vec3<f32>,
    depth: f32,
    normal: vec3<f32>,
    pixel_m: f32,
}

fn view_depth(at: vec3<f32>) -> f32 {
    return (params.view_proj * vec4<f32>(at, 1.0)).w;
}

// The pixels' rays as points a view depth unit beyond the eye's (`base`,
// the view depth of the eye itself): affine in the pixel, pixel q's at
// `origin + q.x dx + q.y dy`; its hit at view depth z is that point times
// z - base (`params.rays`, made on the CPU: a filter's taps cost no
// unprojection).
struct RayPlane {
    origin: vec3<f32>,
    dx: vec3<f32>,
    dy: vec3<f32>,
    base: f32,
}

fn ray_plane(size: vec2<u32>) -> RayPlane {
    return RayPlane(params.rays[0].xyz, params.rays[1].xyz, params.rays[2].xyz, params.rays[0].w);
}

fn plane_ray(rp: RayPlane, q: vec2<i32>) -> vec3<f32> {
    return rp.origin + f32(q.x) * rp.dx + f32(q.y) * rp.dy;
}

fn inside(q: vec2<i32>, size: vec2<u32>) -> bool {
    return all(q >= vec2<i32>(0)) && all(q < vec2<i32>(size));
}

// Pixel `p`'s terrain hit (none where a mesh is in front of it, or the
// sky: `shadow` writes those a zero normal).
fn surface(p: vec2<i32>, rp: RayPlane) -> Surface {
    let n = textureLoad(hit_normals, p, 0).xyz;
    if dot(n, n) == 0.0 { return Surface(vec3<f32>(0.0), 0.0, vec3<f32>(0.0), 0.0); }
    let d = plane_ray(rp, p);
    let z = textureLoad(hit_distance, p, 0).r / length(d);
    return Surface(d * z, rp.base + z, n, length(rp.dx) * z);
}

// Its view depth alone, from its distance and walked sun (0: none).
fn surface_depth(q: vec2<i32>, walked: vec3<f32>, rp: RayPlane) -> f32 {
    if walked.z < 0.5 { return 0.0; }
    return rp.base + textureLoad(hit_distance, q, 0).r / length(plane_ray(rp, q));
}

// Pixel `q`'s hit from `geometry` (this frame's, by `temporal`): its view
// depth past the eye's (0: none) and normal.
fn stored_hit(q: vec2<i32>, size: vec2<u32>, rp: RayPlane) -> vec4<f32> {
    if !inside(q, size) { return vec4<f32>(0.0); }
    let g = geometry[texel(q, size)];
    if g.x == 0u { return vec4<f32>(0.0); }
    return vec4<f32>(bitcast<f32>(g.x) - rp.base, unpack_unit(g.y));
}

// Where pixel `p`'s hit `s` was last frame: its place among the history's
// texels (`base` the texel at or before it, `f` the fraction past it) and
// how far its view depth then (`depth`) may differ from a texel's of the
// same surface (by its depth's slope); none (depth 0: behind the eye). A
// texel's history is its pixel's, whatever point the projection's jitter
// had it sample: the point is placed as this frame's jitter would have it
// (a still camera reads its own texel).
struct Reprojection {
    base: vec2<i32>,
    f: vec2<f32>,
    depth: f32,
    tolerance: f32,
}

fn reproject(s: Surface, size: vec2<u32>, previous: mat4x4<f32>) -> Reprojection {
    var r: Reprojection;
    let clip = previous * vec4<f32>(s.at, 1.0);
    if clip.w <= 0.0 { return r; }
    let ndc = clip.xy / clip.w + params.history.xy;
    let uv = (vec2<f32>(ndc.x, -ndc.y) * 0.5 + 0.5) * vec2<f32>(size) - 0.5;
    r.base = vec2<i32>(floor(uv));
    r.f = uv - floor(uv);
    r.depth = clip.w;
    // The depth a pixel's width moves over the surface, seen this steeply.
    let facing = abs(dot(s.normal, normalize(s.at)));
    r.tolerance = 0.01 * clip.w + 2.0 * s.pixel_m / max(facing, 0.1);
    return r;
}

// Whether `geometry_prev` texel `g` holds the surface (normal `n`, packed
// `packed`) the reprojection `r` is of (not another one, disoccluded).
fn same_surface(r: Reprojection, g: vec2<u32>, n: vec3<f32>, packed: u32) -> bool {
    if r.depth <= 0.0 || g.x == 0u || abs(bitcast<f32>(g.x) - r.depth) > r.tolerance { return false; }
    return g.y == packed || dot(unpack_unit(g.y), n) >= 0.8;
}

// The bilinear weight of texel `i` (of the 2x2 at `base`).
fn bilinear_weight(f: vec2<f32>, i: u32) -> f32 {
    let w = select(1.0 - f, f, vec2<bool>((i & 1u) != 0u, (i >> 1u) != 0u));
    return w.x * w.y;
}

// Catmull-Rom's weights for the four texels about fraction `f`.
fn catmull_rom(f: f32) -> vec4<f32> {
    return vec4<f32>(f * (-0.5 + f * (1.0 - 0.5 * f)), 1.0 + f * f * (-2.5 + 1.5 * f), f * (0.5 + f * (2.0 - 1.5 * f)), f * f * (-0.5 + 0.5 * f));
}

// The sun's and the sky's history at reprojection `r` of a hit (normal
// `n`): read sharp (Catmull-Rom over 4x4 texels less the corners, which
// weigh little) where they all hold its surface,
// else bilinear over those of the 2x2 that do (a camera's slow drift
// would otherwise blur it a little more each frame); `found` false where
// none does.
struct History {
    h: vec4<f32>,
    short: vec4<f32>,
    found: bool,
}

fn history_at(r: Reprojection, n: vec3<f32>, size: vec2<u32>) -> History {
    var out: History;
    let packed = pack_unit(n);
    // A still camera: the texel it lands on (its matrices' rounding moves
    // a near point by a few hundredths of a pixel).
    let near = round(r.f);
    if params.rays[1].w > 0.5 {
        let q = r.base + vec2<i32>(near);
        if inside(q, size) && same_surface(r, geometry_prev[texel(q, size)], n, packed) {
            return History(halves(history_prev[texel(q, size)]), halves(history_short_prev[texel(q, size)]), true);
        }
        return out;
    }
    let wx = catmull_rom(r.f.x);
    let wy = catmull_rom(r.f.y);
    var sharp = true;
    var c = History(vec4<f32>(0.0), vec4<f32>(0.0), true);
    var b = History(vec4<f32>(0.0), vec4<f32>(0.0), true);
    var b_total = 0.0;
    for (var j = 0; j < 4; j++) {
        for (var i = 0; i < 4; i++) {
            let edge = vec2<bool>(i == 0 || i == 3, j == 0 || j == 3);
            if all(edge) { continue; }
            let q = r.base + vec2<i32>(i - 1, j - 1);
            let inner = !any(edge);
            let k = wx[i] * wy[j];
            let w = select(0.0, bilinear_weight(r.f, u32(i - 1) | (u32(j - 1) << 1u)), inner);
            var g = vec2<u32>(0u);
            if inside(q, size) { g = geometry_prev[texel(q, size)]; }
            if !same_surface(r, g, n, packed) {
                sharp = false;
                continue;
            }
            let h = halves(history_prev[texel(q, size)]);
            let short = halves(history_short_prev[texel(q, size)]);
            c.h += k * h;
            c.short += k * short;
            if inner {
                b.h += w * h;
                b.short += w * short;
                b_total += w;
            }
        }
    }
    if sharp {
        // The weights without the corners.
        let total = (wx[1] + wx[2]) * (wy[0] + wy[1] + wy[2] + wy[3]) + (wx[0] + wx[3]) * (wy[1] + wy[2]);
        c.h = max(c.h / total, vec4<f32>(0.0));
        c.short = max(c.short / total, vec4<f32>(0.0));
        return c;
    }
    if b_total <= 0.01 { return out; }
    return History(b.h / b_total, b.short / b_total, true);
}

// A pass over 16x16 pixels a workgroup, with a tile of pixels around them
// in workgroup memory.
const GROUP: i32 = 16;

// The clamp's tile: this frame's walked sun about the workgroup (-1: no
// terrain hit), then each tile row's range over the window's width.
const CLAMP_TILE: i32 = GROUP + 2 * CLAMP_RADIUS;
var<workgroup> w_sun: array<f32, 576>;
var<workgroup> w_lo: array<f32, 384>;
var<workgroup> w_hi: array<f32, 384>;

// The sun's and the sky's history (`history_prev`, `history_short_prev`
// along the hits' motion, the sun's clamped to the range of this frame's
// samples in the 9x9 pixels around) and this frame's samples, accumulated,
// and the hits they are for (`out_geometry`); the filters' terms
// (`out_filter`: the means and their variance: the binary sun's m(1 - m),
// the sky's moments, over the frames accumulated; a fresh history's
// widened to its samples' blocks' spread).
@compute @workgroup_size(16, 16)
fn temporal(@builtin(global_invocation_id) id: vec3<u32>, @builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let size = textureDimensions(hit_distance);
    let rp = ray_plane(size);
    let origin = vec2<i32>(group.xy) * GROUP - CLAMP_RADIUS;
    for (var i = li; i < u32(CLAMP_TILE * CLAMP_TILE); i += 256u) {
        let q = origin + vec2<i32>(i32(i) % CLAMP_TILE, i32(i) / CLAMP_TILE);
        var sun = -1.0;
        if inside(q, size) {
            let walked = walked_sun(q, size);
            sun = select(-1.0, walked.x, walked.z > 0.5);
        }
        w_sun[i] = sun;
    }
    workgroupBarrier();
    // The window's range, a row's then a column's (no terrain hit: none).
    for (var i = li; i < u32(CLAMP_TILE * GROUP); i += 256u) {
        let row = i32(i) / GROUP;
        let column = i32(i) % GROUP;
        var lo = 1.0;
        var hi = 0.0;
        for (var k = 0; k <= 2 * CLAMP_RADIUS; k++) {
            let v = w_sun[row * CLAMP_TILE + column + k];
            if v >= 0.0 {
                lo = min(lo, v);
                hi = max(hi, v);
            }
        }
        w_lo[i] = lo;
        w_hi[i] = hi;
    }
    workgroupBarrier();
    if id.x >= size.x || id.y >= size.y { return; }
    let p = vec2<i32>(id.xy);
    let pixel = texel(p, size);
    let walked = walked_sun(p, size);
    let sun = walked.x;
    let weight = select(SUN_BORROWED, 1.0, walked.y > 0.5);
    let s = surface(p, rp);
    if s.depth <= 0.0 {
        out_history[pixel] = to_halves(vec4<f32>(sun, 1.0, 1.0, 0.0));
        out_history_short[pixel] = to_halves(vec4<f32>(sun, 1.0, 0.0, 0.0));
        out_geometry[pixel] = vec2<u32>(0u);
        out_filter[pixel] = to_halves(vec4<f32>(sun, 1.0, 0.0, 0.0));
        return;
    }
    let sky = textureLoad(hit_normals, p, 0).w;
    var lo = sun;
    var hi = sun;
    for (var k = 0; k <= 2 * CLAMP_RADIUS; k++) {
        let i = (i32(local.y) + k) * GROUP + i32(local.x);
        lo = min(lo, w_lo[i]);
        hi = max(hi, w_hi[i]);
    }
    // The histories along the motion (none: this frame's samples).
    // Along the hit's volume's motion (`previous_projection`).
    let slot = textureLoad(hit_visibility, p, 0).x;
    let last = history_at(reproject(s, size, previous_projection(slot)), s.normal, size);
    var h = vec4<f32>(sun, sky, sky * sky, 0.0);
    var short = vec2<f32>(sun, sky);
    var sun_weight = 0.0;
    if last.found {
        h = last.h;
        short = last.short.xy;
        sun_weight = last.short.z;
        h.x = clamp(h.x, lo, hi);
        short.x = clamp(short.x, lo, hi);
    }
    // The short histories: the sun's weighted as the long one (a cycle's
    // weight), so both keep to the same mean.
    var frames = vec2<f32>(sun_weight / SUN_FRAME_WEIGHT, h.w) + 1.0;
    short.x = mix(short.x, sun, weight / (min(sun_weight, SUN_SHORT * SUN_FRAME_WEIGHT) + weight));
    short.y = mix(short.y, sky, 1.0 / min(frames.y, SKY_SHORT));
    // The long histories restart from the short ones they leave.
    // (In a penumbra the samples themselves swing between 0 and 1: their
    // spread, the long history's m(1 - m), widens the bound.)
    if abs(h.x - short.x) > SUN_LAG + 0.5 * sqrt(h.x * (1.0 - h.x)) {
        h.x = short.x;
        sun_weight = min(sun_weight, SUN_SHORT * SUN_FRAME_WEIGHT);
    }
    if abs(h.y - short.y) > SKY_LAG {
        h.z += short.y * short.y - h.y * h.y;
        h.y = short.y;
        frames.y = min(frames.y, SKY_SHORT);
    }
    sun_weight = min(sun_weight + weight, SUN_FRAMES * SUN_FRAME_WEIGHT);
    let a = vec2<f32>(weight / sun_weight, 1.0 / min(frames.y, SKY_FRAMES));
    let out = vec4<f32>(mix(h.x, sun, a.x), mix(h.y, sky, a.y), mix(h.z, sky * sky, a.y), min(frames.y, SKY_FRAMES));
    out_history[pixel] = to_halves(out);
    out_history_short[pixel] = to_halves(vec4<f32>(short, sun_weight, 0.0));
    out_geometry[pixel] = vec2<u32>(bitcast<u32>(s.depth), pack_unit(s.normal));
    // The sun's samples counted as own samples (a lent one is worth 0.05 of
    // one), the sky's as frames.
    let accumulated = vec2<f32>(sun_weight, out.w);
    let variance = vec2<f32>(out.x * (1.0 - out.x), max(out.z - out.y * out.y, 0.0)) / max(accumulated, vec2<f32>(1.0));
    // Few frames: the samples' blocks' own spread, whatever they showed.
    let fresh = max(4.0 - accumulated, vec2<f32>(0.0)) * 0.0625;
    out_filter[pixel] = to_halves(vec4<f32>(out.xy, variance + fresh));
}

// The filters' tile: the workgroup's pixels and FILTER_APRON around, each
// pixel's hit as its view depth past the eye's (0: no terrain hit) and
// normal, and its terms and their variance (two halves each), decoded once
// for the workgroup's taps.
const FILTER_APRON: i32 = 2;
const FILTER_SIDE: i32 = GROUP + 2 * FILTER_APRON;
var<workgroup> t_hit: array<vec4<f32>, 400>;
var<workgroup> t_terms: array<vec2<u32>, 400>;

fn load_filter_tile(group: vec2<u32>, li: u32, size: vec2<u32>, rp: RayPlane) {
    let origin = vec2<i32>(group) * GROUP - FILTER_APRON;
    for (var i = li; i < u32(FILTER_SIDE * FILTER_SIDE); i += 256u) {
        let q = origin + vec2<i32>(i32(i) % FILTER_SIDE, i32(i) / FILTER_SIDE);
        var hit = vec4<f32>(0.0);
        var terms = vec2<u32>(0u);
        if inside(q, size) {
            let g = geometry[texel(q, size)];
            if g.x != 0u { hit = vec4<f32>(bitcast<f32>(g.x) - rp.base, unpack_unit(g.y)); }
            terms = filter_in[texel(q, size)];
        }
        t_hit[i] = hit;
        t_terms[i] = terms;
    }
    workgroupBarrier();
}

// A pixel's surface as its neighbours are weighed against it: its plane
// (a pixel q's hit at view depth z past the eye's lies
// `(a + q.x bx + q.y by) z - c` off it), its normal, and the plane distance
// that weighs 1/e (`reach`: a pixel's width at the filter's step).
struct Plane {
    a: f32,
    bx: f32,
    by: f32,
    c: f32,
    normal: vec3<f32>,
    reach: f32,
}

fn hit_plane(p: vec2<i32>, hit: vec4<f32>, rp: RayPlane, step: f32) -> Plane {
    let n = hit.yzw;
    return Plane(dot(n, rp.origin), dot(n, rp.dx), dot(n, rp.dy), dot(n, plane_ray(rp, p) * hit.x), n, length(rp.dx) * hit.x * step + 1e-3 * (hit.x + rp.base));
}

// Whether pixel `q`'s hit `hit` is of plane `pl`'s surface: within half
// its reach of the plane, its normal within 11 degrees (`same_plane` a
// half or more).
fn on_plane(pl: Plane, q: vec2<i32>, hit: vec4<f32>) -> bool {
    return hit.x > 0.0 && abs((pl.a + f32(q.x) * pl.bx + f32(q.y) * pl.by) * hit.x - pl.c) <= 0.5 * pl.reach && dot(pl.normal, hit.yzw) >= 0.98;
}

// How much pixel `q`'s hit `hit` (none: 0) belongs to plane `pl`'s
// surface: its distance off the plane, its normal's turn.
fn same_plane(pl: Plane, q: vec2<i32>, hit: vec4<f32>) -> f32 {
    if hit.x <= 0.0 { return 0.0; }
    let off = abs((pl.a + f32(q.x) * pl.bx + f32(q.y) * pl.by) * hit.x - pl.c);
    let n = max(dot(pl.normal, hit.yzw), 0.0);
    let n2 = n * n;
    let n4 = n2 * n2;
    let n16 = n4 * n4 * n4 * n4;
    return exp2(-1.442695 * off / pl.reach) * n16 * n16;
}

// À-trous filter weights (3x3, a B-spline's: 1/4, 1/2, 1/4 a side).
fn spline(i: i32) -> f32 {
    return select(0.25, 0.5, i == 0);
}
// How many of a pixel's standard deviations a neighbour's value may stray
// and still weigh (1/e).
const FILTER_SIGMAS: f32 = 1.5;

// One à-trous pass (3x3 taps `step` pixels apart) for pixel `p`, at
// `local` in the tile: the sun's and the sky's mean and their means'
// variance. Each term's neighbours weigh by how far their value lies from
// the pixel's, in its standard deviations: a settled history barely
// blurs, a fresh one blurs over its samples' blocks.
struct Filtered {
    value: vec2<f32>,
    variance: vec2<f32>,
}

fn atrous(p: vec2<i32>, local: vec2<i32>, step: i32, rp: RayPlane) -> Filtered {
    let c = local.y * FILTER_SIDE + local.x;
    let centre = Filtered(unpack2x16float(t_terms[c].x), unpack2x16float(t_terms[c].y));
    if t_hit[c].x <= 0.0 { return centre; }
    let pl = hit_plane(p, t_hit[c], rp, f32(step));
    let sigma = FILTER_SIGMAS * sqrt(centre.variance) + 1e-3;
    var sum = centre.value * 0.25;
    var weights = vec2<f32>(0.25);
    var variance = centre.variance * 0.0625;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            if dx == 0 && dy == 0 { continue; }
            let d = vec2<i32>(dx, dy) * step;
            let i = (local.y + d.y) * FILTER_SIDE + local.x + d.x;
            // Past the frame: no terrain hit there (depth 0).
            let g = same_plane(pl, p + d, t_hit[i]);
            if g <= 1e-4 { continue; }
            let v = unpack2x16float(t_terms[i].x);
            let w = spline(dx) * spline(dy) * g * exp2(-1.442695 * abs(v - centre.value) / sigma);
            sum += w * v;
            weights += w;
            variance += w * w * unpack2x16float(t_terms[i].y);
        }
    }
    return Filtered(sum / weights, variance / (weights * weights));
}

// A pixel's terms settled enough to pass the filters as they are: both
// variances under this (a settled history's binary sun at an edge, 0.25
// over its 32 frames, is under it; a fresh one's, widened, far over).
const SETTLED: f32 = 0.01;
var<workgroup> w_unsettled: u32;

// A filter pass (`step`): pixels with settled terms pass as they are, and
// a workgroup with none else loads no tile.
fn filter_pass(id: vec2<u32>, group: vec2<u32>, local: vec2<u32>, li: u32, step: i32) {
    let size = textureDimensions(hit_distance);
    let p = vec2<i32>(id);
    let shown = all(id < size);
    if li == 0u { w_unsettled = 0u; }
    workgroupBarrier();
    var own = vec2<u32>(0u);
    var settled = true;
    if shown {
        own = filter_in[texel(p, size)];
        settled = all(halves(own).zw <= vec2<f32>(SETTLED)) || geometry[texel(p, size)].x == 0u;
        // (Every unsettled pixel writes the same word.)
        if !settled { w_unsettled = 1u; }
    }
    if workgroupUniformLoad(&w_unsettled) == 0u {
        if shown { out_filter[texel(p, size)] = own; }
        return;
    }
    let rp = ray_plane(size);
    load_filter_tile(group, li, size, rp);
    if !shown { return; }
    if settled {
        out_filter[texel(p, size)] = own;
        return;
    }
    let f = atrous(p, vec2<i32>(local) + FILTER_APRON, step, rp);
    out_filter[texel(p, size)] = to_halves(vec4<f32>(f.value, f.variance));
}

@compute @workgroup_size(16, 16)
fn filter_first(@builtin(global_invocation_id) id: vec3<u32>, @builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    filter_pass(id.xy, group.xy, local.xy, li, 1);
}

@compute @workgroup_size(16, 16)
fn filter_second(@builtin(global_invocation_id) id: vec3<u32>, @builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_id) local: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    filter_pass(id.xy, group.xy, local.xy, li, 2);
}

// The terms as lighting takes them (Helio's directional visibility, the
// sky's for the gbuffer): the filtered terms, which are of pixels (their
// whole area), read where this frame's jitter put the pixel's sample, as
// a host's TSR takes it (read at the pixel's centre, TSR would blur them
// by the jitter's spread); sharp (Catmull-Rom less its corners) over the
// neighbours of the same surface, else bilinear over those, else the
// pixel's own. A mesh's
// or the sky's own walk as it was; steady shadows: the walks' own.
@compute @workgroup_size(16, 16)
fn resolve(@builtin(global_invocation_id) id: vec3<u32>, @builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_id) local_id: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let size = textureDimensions(hit_distance);
    let rp = ray_plane(size);
    load_filter_tile(group.xy, li, size, rp);
    if id.x >= size.x || id.y >= size.y { return; }
    let p = vec2<i32>(id.xy);
    let local = vec2<i32>(local_id.xy) + FILTER_APRON;
    let c = local.y * FILTER_SIDE + local.x;
    var value: vec2<f32>;
    if t_hit[c].x <= 0.0 || steady_shadows() {
        value = vec2<f32>(walked_sun(p, size).x, textureLoad(hit_normals, p, 0).w);
    } else {
        let pl = hit_plane(p, t_hit[c], rp, 1.0);
        value = unpack2x16float(t_terms[c].x);
        // The sample's place among the pixels: within half a pixel.
        let offset = vec2<f32>(-params.history.z, params.history.w) * vec2<f32>(size) * 0.5;
        let base = vec2<i32>(floor(offset));
        let f = offset - floor(offset);
        let wx = catmull_rom(f.x);
        let wy = catmull_rom(f.y);
        var sharp = vec2<f32>(0.0);
        var all_same = true;
        var bilinear = vec2<f32>(0.0);
        var total = 0.0;
        for (var j = 0; j < 4; j++) {
            for (var i = 0; i < 4; i++) {
                if (i == 0 || i == 3) && (j == 0 || j == 3) { continue; }
                let d = base + vec2<i32>(i - 1, j - 1);
                let t = (local.y + d.y) * FILTER_SIDE + local.x + d.x;
                if !on_plane(pl, p + d, t_hit[t]) {
                    all_same = false;
                    continue;
                }
                let v = unpack2x16float(t_terms[t].x);
                sharp += wx[i] * wy[j] * v;
                if i >= 1 && i <= 2 && j >= 1 && j <= 2 {
                    let w = bilinear_weight(f, u32(i - 1) | (u32(j - 1) << 1u));
                    bilinear += w * v;
                    total += w;
                }
            }
        }
        if all_same {
            // The weights without the corners.
            let weights = (wx[1] + wx[2]) * (wy[0] + wy[1] + wy[2] + wy[3]) + (wx[0] + wx[3]) * (wy[1] + wy[2]);
            value = clamp(sharp / weights, vec2<f32>(0.0), vec2<f32>(1.0));
        } else if total > 0.01 {
            value = bilinear / total;
        }
    }
    textureStore(out_sun, p, vec4<f32>(value.x, params.sun.xyz));
    textureStore(out_sky, p, vec4<f32>(value.y));
    if plain_colour() && debug_mode() == 0u {
        let v = textureLoad(hit_visibility, p, 0).xy;
        if v.x != NONE {
            let n = textureLoad(hit_normals, p, 0).xyz;
            let material = (v.y >> 21u) & 31u;
            textureStore(out_colour, p, vec4<f32>(palette(material).rgb * (0.25 + 0.75 * max(dot(n, params.sun.xyz), 0.0) * value.x), 1.0));
        }
    }
}

// Local lights: as the sun, by light (its index in the scene's lights,
// which its tile slots name). A pixel tracks the four lights reaching it
// with the least indices (`LOCAL_TRACKED`): its history (`local_history`)
// an entry each, (index + 1) << 16 | long visibility << 8 (8 bits) |
// short visibility << 3 (5 bits) | samples' weight (3 bits, of
// `LOCAL_WEIGHT`); lights
// past those, or of larger indices (`LOCAL_IDS`), keep their walks' bits.
// Lighting's words a pixel (`out_local`, Helio's `local_visibility`): the
// tracked lights' tile slots (a byte each, 0xff: none), their visibility
// (a byte each), the other slots' bits (1: seen).
const LOCAL_TRACKED: u32 = 4u;
// The long history's frames once settled, and its samples' weight then.
const LOCAL_FRAMES: f32 = 16.0;
const LOCAL_WEIGHT: f32 = LOCAL_FRAMES * SUN_FRAME_WEIGHT;
// Light indices an entry holds (16 bits, less the empty one).
const LOCAL_IDS: u32 = 0xfffeu;

@group(1) @binding(51) var<storage, read> local_history_prev: array<vec4<u32>>;
@group(1) @binding(52) var<storage, read_write> local_history: array<vec4<u32>>;
@group(1) @binding(53) var<storage, read_write> out_local: array<u32>;
// The local lights' passes' indirect dispatch (x, y, z: none without a
// point or spot light in the scene) and whether the scene had one this
// frame and the last (`local_gate`; `local_present` as `local_temporal`
// reads it).
@group(1) @binding(56) var<storage, read_write> local_dispatch: array<u32, 5>;
@group(1) @binding(57) var<storage, read> local_present: array<u32, 5>;

var<workgroup> w_present: atomic<u32>;

// Whether the scene has a point or spot light, and the local lights'
// passes' workgroups (8x8 pixels each) if so.
@compute @workgroup_size(64)
fn local_gate(@builtin(local_invocation_index) li: u32) {
    if li == 0u { atomicStore(&w_present, 0u); }
    workgroupBarrier();
    for (var i = li; i < arrayLength(&scene_lights); i += 64u) {
        let light = scene_lights[i];
        if light.light_type != 0u && light.color_intensity.w > 0.0 { atomicOr(&w_present, 1u); }
    }
    workgroupBarrier();
    if li != 0u { return; }
    let present = atomicLoad(&w_present);
    let groups = (textureDimensions(hit_distance) + 7u) / 8u;
    local_dispatch[0] = select(0u, groups.x, present != 0u);
    local_dispatch[1] = groups.y;
    local_dispatch[2] = 1u;
    local_dispatch[4] = local_dispatch[3];
    local_dispatch[3] = present;
}

// The clamp's tile (8x8 pixels a workgroup, in one light tile).
const LOCAL_TILE: i32 = 8 + 2 * CLAMP_RADIUS;
var<workgroup> w_seen: array<u32, 256>;
var<workgroup> l_depth: array<f32, 256>;

fn entry_light(e: u32) -> u32 {
    return e >> 16u;
}

fn entry_visibility(e: u32) -> f32 {
    return f32((e >> 8u) & 255u) / 255.0;
}

fn entry_short(e: u32) -> f32 {
    return f32((e >> 3u) & 31u) / 31.0;
}

// The samples' weight accumulated.
fn entry_weight(e: u32) -> f32 {
    return f32(e & 7u) / 7.0 * LOCAL_WEIGHT;
}

// A visibility to `steps` (255: 8 bits), rounded by a dither that changes
// each frame (a history taking small steps keeps moving).
fn visibility_steps(v: f32, steps: f32, p: vec2<u32>, k: u32) -> u32 {
    let q = vec2<f32>(p) + 5.588238 * f32((params.table.w + 7u * k) % 64u);
    let r = fract(52.9829189 * fract(0.06711056 * q.x + 0.00583715 * q.y));
    return u32(clamp(v * steps + r, 0.0, steps));
}

// The tracked lights' history and this frame's walks (`local_seen`),
// accumulated as the sun's (`temporal`), the clamp's neighbourhood within
// the pixel's light tile (whose slots name the same lights); lighting's
// words.
@compute @workgroup_size(8, 8)
fn local_temporal(@builtin(global_invocation_id) id: vec3<u32>, @builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let size = textureDimensions(hit_distance);
    let rp = ray_plane(size);
    let origin = vec2<i32>(group.xy) * 8 - CLAMP_RADIUS;
    // A workgroup lies in one light tile (8 divides LIGHT_TILE). Without a
    // point or spot light in it, nothing tracked (lighting reads none of
    // its words: they are only for those lights).
    let tile_lo = (vec2<i32>(group.xy) * 8 / i32(LIGHT_TILE)) * i32(LIGHT_TILE);
    let tile_list = light_tile(vec2<u32>(tile_lo), size);
    var local_lights = false;
    for (var i = 0u; i < tile_list.y; i++) {
        local_lights = local_lights || scene_lights[tile_light_lists[tile_list.x + i]].light_type != 0u;
    }
    if !local_lights {
        if id.x < size.x && id.y < size.y { local_history[id.y * size.x + id.x] = vec4<u32>(0u); }
        return;
    }
    for (var i = li; i < u32(LOCAL_TILE * LOCAL_TILE); i += 64u) {
        let q = origin + vec2<i32>(i32(i) % LOCAL_TILE, i32(i) / LOCAL_TILE);
        var depth = 0.0;
        var seen = 0u;
        if all(q >= tile_lo) && all(q < min(tile_lo + i32(LIGHT_TILE), vec2<i32>(size))) {
            depth = surface_depth(q, walked_sun(q, size), rp);
            seen = local_seen[u32(q.y) * size.x + u32(q.x)].x;
        }
        l_depth[i] = depth;
        w_seen[i] = seen;
    }
    workgroupBarrier();
    if id.x >= size.x || id.y >= size.y { return; }
    let pixel = id.y * size.x + id.x;
    let walked = local_seen[pixel];
    let seen = walked.x;
    let weight = select(SUN_BORROWED, 1.0, walked.y != 0u);
    var tracked_slots = 0xffffffffu;
    var tracked = 0u;
    var entries = vec4<u32>(0u);
    let s = surface(vec2<i32>(id.xy), rp);
    if s.depth > 0.0 && !steady_shadows() && params.light_eye.w > 0.0 {
        // The shadowed lights reaching the hit, those of least index first.
        let list = light_tile(id.xy, size);
        // Lights behind foliage reach it through its cells (`light_reach`).
        let hit = textureLoad(hit_visibility, vec2<i32>(id.xy), 0).xy;
        let transmits = hit.x != NONE && materials.m[min((hit.y >> 21u) & 31u, 31u)].light.x > 0.0;
        var lights = array<u32, 4>(NONE, NONE, NONE, NONE);
        var slots = array<u32, 4>(0u, 0u, 0u, 0u);
        for (var i = 0u; i < list.y; i++) {
            var light = tile_light_lists[list.x + i];
            if light > LOCAL_IDS || !light_reach(scene_lights[light], s.at, s.normal, transmits).walk { continue; }
            var slot = i;
            for (var k = 0u; k < LOCAL_TRACKED; k++) {
                if light < lights[k] {
                    let l = lights[k];
                    let o = slots[k];
                    lights[k] = light;
                    slots[k] = slot;
                    light = l;
                    slot = o;
                }
            }
        }
        // The 2x2 texels about the hit last frame that hold its surface.
        // No history after a frame without local lights (none was kept).
        var r = reproject(s, size, previous_projection(hit.x));
        if local_present[4] == 0u { r.depth = 0.0; }
        let packed = pack_unit(s.normal);
        var texels: array<u32, 4>;
        var weights = vec4<f32>(0.0);
        for (var i = 0u; i < 4u; i++) {
            let q = r.base + vec2<i32>(i32(i & 1u), i32(i >> 1u));
            if inside(q, size) && same_surface(r, geometry_prev[texel(q, size)], s.normal, packed) {
                texels[i] = u32(q.y) * size.x + u32(q.x);
                weights[i] = bilinear_weight(r.f, i);
            }
        }
        let centre = vec2<i32>(id.xy) - origin;
        for (var k = 0u; k < LOCAL_TRACKED; k++) {
            if lights[k] == NONE { break; }
            let slot = slots[k];
            let now = f32((seen >> slot) & 1u);
            // The long and the short history, their frames.
            var h = vec3<f32>(0.0);
            var total = 0.0;
            for (var i = 0u; i < 4u; i++) {
                if weights[i] == 0.0 { continue; }
                let e = local_history_prev[texels[i]];
                for (var j = 0u; j < LOCAL_TRACKED; j++) {
                    if entry_light(e[j]) == lights[k] + 1u {
                        h += weights[i] * vec3<f32>(entry_visibility(e[j]), entry_short(e[j]), entry_weight(e[j]));
                        total += weights[i];
                    }
                }
            }
            var v = vec2<f32>(now);
            var accumulated = weight;
            if total > 0.01 {
                var lo = now;
                var hi = now;
                for (var dy = -CLAMP_RADIUS; dy <= CLAMP_RADIUS; dy++) {
                    for (var dx = -CLAMP_RADIUS; dx <= CLAMP_RADIUS; dx++) {
                        let i = (centre.y + dy) * LOCAL_TILE + centre.x + dx;
                        if abs(l_depth[i] - s.depth) > 0.1 * s.depth { continue; }
                        let b = f32((w_seen[i] >> slot) & 1u);
                        lo = min(lo, b);
                        hi = max(hi, b);
                    }
                }
                h /= total;
                // Weighted as the sun's: the pixel's own walks count most.
                let short = mix(clamp(h.y, lo, hi), now, weight / (min(h.z, SUN_SHORT * SUN_FRAME_WEIGHT) + weight));
                var long = clamp(h.x, lo, hi);
                // Restarts from the short history it leaves.
                if abs(long - short) > SUN_LAG + 0.5 * sqrt(long * (1.0 - long)) {
                    long = short;
                    h.z = min(h.z, SUN_SHORT * SUN_FRAME_WEIGHT);
                }
                accumulated = min(h.z + weight, LOCAL_WEIGHT);
                v = vec2<f32>(mix(long, now, weight / accumulated), short);
            }
            let byte = visibility_steps(v.x, 255.0, id.xy, k);
            entries[k] = ((lights[k] + 1u) << 16u) | (byte << 8u) | (visibility_steps(v.y, 31.0, id.xy, k + 4u) << 3u) | visibility_steps(accumulated / LOCAL_WEIGHT, 7.0, id.xy, k + 8u);
            tracked_slots = (tracked_slots & ~(0xffu << (8u * k))) | (slot << (8u * k));
            tracked |= byte << (8u * k);
        }
    }
    local_history[pixel] = entries;
    out_local[pixel * 3u] = tracked_slots;
    out_local[pixel * 3u + 1u] = tracked;
    out_local[pixel * 3u + 2u] = seen;
}

// The tracked lights' visibility filtered as the sun's (one 3x3 pass, the
// neighbours' entries for the same light).
@compute @workgroup_size(8, 8)
fn local_filter(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(hit_distance);
    if id.x >= size.x || id.y >= size.y || steady_shadows() { return; }
    let p = vec2<i32>(id.xy);
    let pixel = id.y * size.x + id.x;
    let entries = local_history[pixel];
    if entries.x == 0u { return; }
    let rp = ray_plane(size);
    let pl = hit_plane(p, stored_hit(p, size, rp), rp, 1.0);
    var tracked = 0u;
    for (var k = 0u; k < LOCAL_TRACKED; k++) {
        let light = entry_light(entries[k]);
        if light == 0u { break; }
        let v = entry_visibility(entries[k]);
        let frames = entry_weight(entries[k]) / SUN_FRAME_WEIGHT;
        let sigma = FILTER_SIGMAS * sqrt(v * (1.0 - v) / max(frames, 1.0) + max(4.0 - frames, 0.0) * 0.0625) + 1e-3;
        var sum = v * 0.25;
        var weights = 0.25;
        for (var dy = -1; dy <= 1; dy++) {
            for (var dx = -1; dx <= 1; dx++) {
                if dx == 0 && dy == 0 { continue; }
                let q = p + vec2<i32>(dx, dy);
                let g = same_plane(pl, q, stored_hit(q, size, rp));
                if g <= 1e-4 { continue; }
                let e = local_history[u32(q.y) * size.x + u32(q.x)];
                for (var j = 0u; j < LOCAL_TRACKED; j++) {
                    if entry_light(e[j]) != light { continue; }
                    let vq = entry_visibility(e[j]);
                    let w = spline(dx) * spline(dy) * g * exp2(-1.442695 * abs(vq - v) / sigma);
                    sum += w * vq;
                    weights += w;
                }
            }
        }
        tracked |= u32(clamp(sum / weights, 0.0, 1.0) * 255.0 + 0.5) << (8u * k);
    }
    out_local[pixel * 3u + 1u] = tracked;
}

// ---- 5. Ray reports ----
// Where rays decide detail goes (`docs/chunk-octree.md`): a pixel of each
// 2x2 a frame (all four over `REPORT_SWEEP` frames, in `chunks/mod.rs`)
// names the node its hit's level wants there: cells of a pixel, two levels
// of margin for the prefetch eye and the coarsening hysteresis, the hit's
// own node at least. A workgroup appends the distinct nodes of its 32x32
// pixels to `reports`, up to a node per 8x8 pixels in all; the planner
// marks them and their ancestors seen. A view's whole set is known a few
// frames after it shows.

struct Reports {
    count: atomic<u32>,
    keys: array<vec4<i32>>,
}
@group(1) @binding(24) var<storage, read_write> reports: Reports;
var<workgroup> w_keys: array<vec4<i32>, 256>;
// Distinct nodes so far, by hash: a lane's index + 1 (0: free).
var<workgroup> w_table: array<atomic<u32>, 512>;
var<workgroup> w_count: atomic<u32>;
var<workgroup> w_base: u32;

// The node a hit (`v`: slot and packed cell; distance `t`) at pixel `p`
// wants.
fn wanted_node(v: vec2<u32>, t: f32, p: vec2<u32>, size: vec2<u32>) -> vec4<i32> {
    let key = chunk_keys[v.x];
    let level = key_level(key.x);
    let cell = vec3<i32>(vec3<u32>(v.y, v.y >> 6u, v.y >> 12u) & vec3<u32>(63u));
    var size_m = length(statics[v.x].m0.xyz);
    if exact_level(level) { size_m = length(cell_frame(v.x, cell).m0.xyz); }
    let n = clamp(i32(ceil(log2(max(size_m / max(pixel_width(p, size, t), 1e-6), 1e-3)))) + 2, 0, level);
    // Up to six levels finer, the node's 64 cells hold the hit's whole cell.
    if n <= 6 {
        return vec4<i32>(key_x(key.x & 7, level - n), (key.yzw * 64 + cell) >> vec3<u32>(u32(6 - n)));
    }
    let c = cell_frame(v.x, cell);
    let position = pixel_ray(p, size) * t;
    let local = vec3<f32>(c.i0.w, c.i1.w, c.i2.w) + vec3<f32>(dot(c.i0.xyz, position), dot(c.i1.xyz, position), dot(c.i2.xyz, position));
    let frac = clamp(local - vec3<f32>(cell), vec3<f32>(0.0), vec3<f32>(0.999));
    let fine = ((key.yzw * 64 + cell) << vec3<u32>(u32(n))) + vec3<i32>(floor(frac * f32(1 << u32(n))));
    return vec4<i32>(key_x(key.x & 7, level - n), fine >> vec3<u32>(6u));
}

@compute @workgroup_size(16, 16)
fn feedback(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>, @builtin(local_invocation_index) li: u32) {
    let id = rect_id(gid, 2u);
    let size = textureDimensions(hit_distance);
    if li == 0u { atomicStore(&w_count, 0u); }
    atomicStore(&w_table[li], 0u);
    atomicStore(&w_table[li + 256u], 0u);
    let phase = params.table.w & 3u;
    let p = id.xy * 2u + vec2<u32>(phase & 1u, phase >> 1u);
    var key = vec4<i32>(-1, 0, 0, 0);
    if all(p < size) {
        let v = textureLoad(hit_visibility, vec2<i32>(p), 0).xy;
        let t = textureLoad(hit_distance, vec2<i32>(p), 0).r;
        if v.x != NONE && t > 0.0 && ours(v.x) { key = wanted_node(v, t, p, size); }
    }
    w_keys[li] = key;
    workgroupBarrier();
    // Each node once: the lane that claims its hash slot (open addressing;
    // another node there sends it on). Lanes whose left or upper neighbour
    // holds it too skip that (the first lane holding a node never does):
    // most do.
    var first = key.x >= 0;
    if first && lid.x > 0u && all(w_keys[li - 1u] == key) { first = false; }
    if first && lid.y > 0u && all(w_keys[li - 16u] == key) { first = false; }
    var rank = 0u;
    if first {
        let h = vec4<u32>(key) * vec4<u32>(0x9e3779b1u, 0x85ebca77u, 0xc2b2ae3du, 0x27d4eb2fu);
        var at = h.x ^ h.y ^ h.z ^ h.w;
        at = (at ^ (at >> 15u)) & 511u;
        first = false;
        for (var probe = 0u; probe < 1024u; probe++) {
            let r = atomicCompareExchangeWeak(&w_table[at], 0u, li + 1u);
            if r.exchanged {
                first = true;
                rank = atomicAdd(&w_count, 1u);
                break;
            }
            // A weak exchange may fail spuriously: the same slot again.
            if r.old_value == 0u { continue; }
            if all(w_keys[r.old_value - 1u] == key) { break; }
            at = (at + 1u) & 511u;
        }
    }
    workgroupBarrier();
    if li == 0u {
        let n = atomicLoad(&w_count);
        w_base = select(0u, atomicAdd(&reports.count, n), n > 0u);
    }
    workgroupBarrier();
    // Up to a node per 8x8 pixels (the readback's size).
    let room = ((size.x + 7u) / 8u) * ((size.y + 7u) / 8u);
    let at = w_base + rank;
    if first && at < room { reports.keys[at] = key; }
}

// ---- 6. GI probe rays ----
// The terrain as a tracer of the probes' rays (helio-pass-gi): a ray walks
// the cut from its probe's place, and its hit sees the sun by the sun's
// occlusion maps (`sun_occlusion`: a texture read where a walk to the sun
// cost several times the ray's own). A probe's
// place (`gi_cells`: base cell i, j, k and face | 8) is located on the CPU
// in f64 when the probe comes into the clipmap.
@group(1) @binding(35) var<uniform> frame_gi: GiFrame;
@group(1) @binding(36) var<storage, read> gi_rays: array<GiRay>;
@group(1) @binding(37) var<storage, read_write> gi_hits: array<GiHit>;
@group(1) @binding(38) var<storage, read> gi_cells: array<vec4<i32>>;
@group(1) @binding(39) var<storage, read> gi_sun: array<vec4<f32>, 2>;
@group(1) @binding(62) var<uniform> sun_occlusion_frame: SunOcclusionFrame;
@group(1) @binding(63) var sun_occlusion_maps: texture_2d<f32>;
@group(1) @binding(65) var<storage, read> sun_occlusion_pages: array<u32>;

// Where a walk from eye-relative point `org` in base cell `c` starts: its
// chunk, rock, or the air box it is in, crossed in the frame of a drawn
// chunk beside it (in its column, at its level or finer: the ground under
// air). None without one (nothing drawn near).
struct ProbeStart {
    walk: Walk,
    rock: bool,
    ok: bool,
}

fn probe_start(c: vec4<i32>, org: vec3<f32>, r: i32) -> ProbeStart {
    var out = ProbeStart(Walk(0u, vec4<i32>(0), vec3<i32>(0), vec3<i32>(0), false), false, false);
    if (c.w & 8) == 0 { return out; }
    let face = c.w & 7;
    let at = place_at(face, 0, c.xyz, vec3<f32>(0.5), r, false);
    if at.slot != NONE {
        let f = chunk_frame(at.slot);
        let o = vec3<f32>(f.i0.w, f.i1.w, f.i2.w) + vec3<f32>(dot(f.i0.xyz, org), dot(f.i1.xyz, org), dot(f.i2.xyz, org));
        out.rock = solid_at(at.slot, vec3<i32>(floor(o)));
        out.walk = Walk(at.slot, chunk_keys[at.slot], vec3<i32>(0), vec3<i32>(64), true);
        out.ok = !out.rock;
        return out;
    }
    if at.level == ROCK || at.level == BURIED {
        out.rock = true;
        return out;
    }
    if at.level == LOST { return out; }
    if exact_level(at.level) {
        out.walk = Walk(0u, vec4<i32>(key_x(face, at.level), 0, 0, 0), at.lo, at.hi, false);
        out.ok = true;
        return out;
    }
    var f = at.frame;
    for (var l = at.level; l >= 0 && f == NONE; l--) {
        f = frame_near(vec4<i32>(key_x(face, l), (c.xyz >> vec3<u32>(u32(l))) >> vec3<u32>(6u)));
    }
    if f == NONE { return out; }
    let fk = chunk_keys[f];
    let diff = at.level - (key_level(fk.x));
    out.walk = Walk(f, fk, to_frame(at.lo, diff, fk, true), clip_to_face(to_frame(at.hi, diff, fk, false), fk), false);
    out.ok = true;
    return out;
}

// A thread a ray: its hit (distance, the face's normal, the material's
// colour, the share of the sun reaching it (`sun_occlusion`); for a
// material that lets light through, its
// transmission and the sun's share on its cell's far side; the light it
// gives off), or none; from rock, a back face at once.
@compute @workgroup_size(64)
fn gi_trace(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= frame_gi.counts.x * GI_RAYS { return; }
    let ray = gi_rays[id.x];
    let q = vec2<u32>(id.x & 63u, id.x >> 6u);
    let r = fade_dither(q);
    // In the primary volume's frame (its probes were located there).
    let org = to_local(ray.origin.xyz);
    let start = probe_start(gi_cells[ray.probe], org, r);
    if start.rock {
        gi_hits[id.x] = GiHit(1e-3, gi_pack_unit(ray.dir), 0u, 1u);
        return;
    }
    if !start.ok { return; }
    let end = walk(start.walk, org, to_local(ray.dir), 0.0, ray.origin.w, r, 96u, false, q);
    if !end.hit { return; }
    gi_shade(id.x, ray, end);
}

// The other volumes' parts in the probes' rays (`ChunkWorld::draw`: a pass
// each volume whose bounds the clipmap's rays reach): a ray walks this
// volume from where it enters it (`enter_volume`), up to the nearer of its
// reach and the hit already there (each tracer's contract: the hit kept is
// the nearest); from its rock, a back face at once, as a probe's.
@compute @workgroup_size(64)
fn gi_trace_cross(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= frame_gi.counts.x * GI_RAYS { return; }
    let ray = gi_rays[id.x];
    let q = vec2<u32>(id.x & 63u, id.x >> 6u);
    let r = fade_dither(q);
    let org = to_local(ray.origin.xyz);
    let dir = to_local(ray.dir);
    var reach = ray.origin.w;
    let before = gi_hits[id.x].t;
    if before > 0.0 { reach = min(reach, before); }
    let e = enter_volume(org, dir, reach, r);
    if e.kind == 2u { return; }
    if e.kind == 1u {
        gi_hits[id.x] = GiHit(max(e.t, 1e-3), gi_pack_unit(ray.dir), 0u, 1u);
        return;
    }
    let end = walk(e.walk, org, dir, e.t, reach, r, 96u, false, q);
    if !end.hit || end.t >= reach { return; }
    gi_shade(id.x, ray, end);
}

// Ray `index`'s hit `end` (`ray` from its origin): its face, material and
// light, and the sun's share there (`gi_trace`).
fn gi_shade(index: u32, ray: GiRay, end: WalkEnd) {
    let c = cell_frame(end.slot, end.cell);
    let axis = select(select(c.m2.xyz, c.m1.xyz, end.axis == 1u), c.m0.xyz, end.axis == 0u);
    let n = to_world(normalize(axis) * select(-1.0, 1.0, end.sign == 1u));
    let material = shown_material(end.material, end.slot, end.cell, end.axis == 2u && end.sign == 1u);
    // The sun from off the hit's face by half its cell; through a cell
    // that lets light through, from off its far side.
    let hit = ray.origin.xyz + ray.dir * end.t;
    let cell = length(c.m0.xyz);
    let towards = dot(n, gi_sun[0].xyz);
    let through = clamp(materials.m[min(material, 31u)].light.x, 0.0, 1.0);
    var sun = 0.0;
    var back = 0.0;
    if towards > 0.0 {
        sun = sun_occlusion(hit + n * (0.5 * cell), n, cell);
    } else if through > 0.0 && towards < 0.0 {
        back = sun_occlusion(hit - n * (1.5 * cell), vec3<f32>(0.0), cell);
    }
    let glow = u32(clamp(materials.m[min(material, 31u)].light.y, 0.0, 15.9375) * 16.0 + 0.5);
    let light = 1u | (u32(through * 255.0 + 0.5) << 8u) | (u32(back * 255.0 + 0.5) << 16u) | (glow << 24u);
    gi_hits[index] = GiHit(end.t, gi_pack_unit(n), pack4x8unorm(vec4<f32>(clamp(palette(material).rgb, vec3<f32>(0.0), vec3<f32>(1.0)), sun)), light);
}

// ---- 7. Foliage ground probes ----
// The terrain as the ground foliage grows on (`chunks::ground`): each query
// is a segment down a foliage tile's up, from a point its tile's
// `probe_tiles` entry places in a base cell (located on the CPU in f64, as
// GI probes are); it walks the drawn cut, and where it meets a top face it
// answers the distance, the face's normal and its cell's material (the
// ground id foliage types are keyed by). Walls, rock and nothing drawn are
// no ground. A switch between levels shows its new side (dither 0): the
// ground changes when it starts and again when it ends.
struct GroundQuery {
    start: vec3<f32>,
    length: f32,
    up: vec3<f32>,
    pad: u32,
}

struct GroundHit {
    distance: u32,
    normal: u32,
    ground: u32,
    // The ground's linear colour there, RGBA8 (0: none).
    colour: u32,
}

struct ProbeTile {
    cell: vec4<i32>,
    frac: vec4<f32>,
    at: vec4<f32>,
    j0: vec4<f32>,
    j1: vec4<f32>,
    j2: vec4<f32>,
}

// The eye in the queries' space; x the queries in all, y the blocks (their first
// queries ascending, in `at.w`).
struct ProbeFrame {
    eye: vec4<f32>,
    counts: vec4<u32>,
}

// Starts a probe tries, PROBE_STEP metres apart downwards.
const PROBE_ATTEMPTS: u32 = 4u;
const PROBE_STEP: f32 = 4.0;

@group(1) @binding(58) var<uniform> probe_frame: ProbeFrame;
@group(1) @binding(59) var<storage, read> ground_queries: array<GroundQuery>;
@group(1) @binding(60) var<storage, read_write> ground_hits: array<GroundHit>;
@group(1) @binding(61) var<storage, read> probe_tiles: array<ProbeTile>;

@compute @workgroup_size(64)
fn ground_probe(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= probe_frame.counts.x || id.x >= arrayLength(&ground_queries) || id.x >= arrayLength(&ground_hits) { return; }
    var hit = GroundHit(bitcast<u32>(-1.0), 0u, 0xffffffffu, 0u);
    // The block holding the query: the last whose first query is at or before it.
    var lo = 0u;
    var hi = min(probe_frame.counts.y, arrayLength(&probe_tiles));
    if hi == 0u || id.x < bitcast<u32>(probe_tiles[0].at.w) {
        ground_hits[id.x] = hit;
        return;
    }
    loop {
        if hi - lo <= 1u { break; }
        let mid = (lo + hi) / 2u;
        if bitcast<u32>(probe_tiles[mid].at.w) <= id.x { lo = mid; } else { hi = mid; }
    }
    let t = lo;
    let query = ground_queries[id.x];
    let tile = probe_tiles[t];
    // Walked from under the query start by the tile drop (`frac.w`), where drawn
    // chunks are near; lower, a step at a time, where none is near enough to start
    // from (rock ends it: the ground is over the start there).
    // In the primary volume's frame (its tiles were placed there).
    let up = to_local(normalize(query.up));
    let from_eye = to_local(query.start - probe_frame.eye.xyz);
    var drop = tile.frac.w;
    var org = vec3<f32>(0.0);
    var start: ProbeStart;
    for (var attempt = 0u; attempt < PROBE_ATTEMPTS; attempt++) {
        org = from_eye - up * drop;
        let d = org - tile.at.xyz;
        let c = tile.frac.xyz + vec3<f32>(dot(tile.j0.xyz, d), dot(tile.j1.xyz, d), dot(tile.j2.xyz, d));
        start = probe_start(vec4<i32>(tile.cell.xyz + vec3<i32>(floor(c)), tile.cell.w), org, 0);
        if start.ok || start.rock { break; }
        drop += PROBE_STEP;
    }
    if start.ok {
        let end = walk(start.walk, org, -up, 0.0, query.length - drop, 0, 96u, false, vec2<u32>(id.x & 63u, id.x >> 6u));
        if end.hit && end.slot != NONE && end.t >= 0.0 && end.t + drop <= query.length {
            hit = ground_answer(end, org, up, drop);
        }
    }
    ground_hits[id.x] = hit;
}

// The ground at walk end `end` down `up` from `org` (`along` past the
// query's start): a top face's distance from the start, normal and
// material, else none (`ground_probe`).
fn ground_answer(end: WalkEnd, org: vec3<f32>, up: vec3<f32>, along: f32) -> GroundHit {
    var hit = GroundHit(bitcast<u32>(-1.0), 0u, 0xffffffffu, 0u);
    let f = cell_frame(end.slot, end.cell);
    let axis = select(select(f.m2.xyz, f.m1.xyz, end.axis == 1u), f.m0.xyz, end.axis == 0u);
    let n = normalize(axis) * select(-1.0, 1.0, end.sign == 1u);
    if dot(n, up) > 0.5 {
        let m = shown_material(end.material, end.slot, end.cell, true);
        // The turf's colour there as the draw shows it from a few metres: the
        // material's patches and clumps, without each cube's pigment. Grass of
        // ground-coloured types takes it, so past the last blades the ground
        // already reads as the meadow.
        let at = org - up * (end.t + 0.01 * length(f.m0.xyz));
        let p = params.eye_domain.xyz + vec3<i32>(floor(params.eye_domain_frac.xyz + at / DOMAIN_UNIT));
        let colour = material_albedo(min(m, 31u), p, 0.0, 0.4, 0.0);
        hit = GroundHit(bitcast<u32>(end.t + along), pack_unit(to_world(n)), min(m, 31u), pack4x8unorm(vec4<f32>(colour, 1.0)));
    }
    return hit;
}

// Foliage's ground across volumes: after the ground volume's probes
// (`ground_probe`), each other volume near the eye (`ChunkWorld::
// probe_ground`) takes the queries whose columns cross it. Where it holds
// solid within FOLIAGE_CLEARANCE over the ground found (it stands or lies
// on it, or sinks into it), its surface there is the ground (a top face;
// else none); with no ground found, its top. A volume floating higher
// leaves the ground under it its foliage: grass grows on a block set on a
// meadow, never inside or under it.
const FOLIAGE_CLEARANCE: f32 = 2.0;

@compute @workgroup_size(64)
fn ground_probe_cross(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= probe_frame.counts.x || id.x >= arrayLength(&ground_queries) || id.x >= arrayLength(&ground_hits) { return; }
    let query = ground_queries[id.x];
    let up = to_local(normalize(query.up));
    let ground = bitcast<f32>(ground_hits[id.x].distance);
    // From the clearance over the ground found (to a little into it), else
    // the whole segment.
    var band_top = 0.0;
    var band_end = query.length;
    if ground >= 0.0 {
        band_top = max(ground - FOLIAGE_CLEARANCE, 0.0);
        band_end = ground + 0.05;
    }
    let org = to_local(query.start - probe_frame.eye.xyz) - up * band_top;
    let q = vec2<u32>(id.x & 63u, id.x >> 6u);
    let e = enter_volume(org, -up, band_end - band_top, 0);
    if e.kind == 2u { return; }
    var hit = GroundHit(bitcast<u32>(-1.0), 0u, 0xffffffffu, 0u);
    if e.kind == 0u {
        let end = walk(e.walk, org, -up, e.t, band_end - band_top, 0, 96u, false, q);
        if !end.hit { return; }
        // Solid where the band starts reaches over it: no ground.
        if end.slot != NONE && end.t > 0.0 { hit = ground_answer(end, org, up, band_top); }
    }
    ground_hits[id.x] = hit;
}
