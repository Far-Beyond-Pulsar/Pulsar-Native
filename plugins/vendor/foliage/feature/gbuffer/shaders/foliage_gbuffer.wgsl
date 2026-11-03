//!use helio_prelude
//!use helio_foliage_wind
//!use helio_foliage_common

// Foliage G-buffer rasterisation — tufts of blades, smooth or blocky, at four LODs.
//
// One pipeline, four `draw_indirect` calls, no vertex buffer and no index buffer. Every
// vertex is derived from `@builtin(vertex_index)` plus the packed 16-byte record fetched
// through `visible_blades[]`:
//
//   L0, L1  one blade of a near tuft (a record per blade): a 5- or 3-segment strip, or a
//           blocky blade's boxes (every run of cubes, or cubes merged in pairs).
//   L2      a whole tuft (one record): each blade a card, or one box.
//   L3      a whole tuft as one shape: a fan of three broad blades, or two crossed boxes.
//
// A blade's shape is a function of its tuft's seed, its index in the tuft and its type:
// the tuft's blades fan out from a tight base, lean out more at the rim, bend more towards
// the tip, some lie over, and they take the colour of the ground they grow from (the
// ground answered it at placement), darker at the root and lighter at the tip.
//
// Bands (`foliage_bands`) carry tufts out to the foliage's range: a tuft draws scaled by
// `foliage_tuft_scale` — its band's fade, shrinking out as the kept share falls to its
// rank, growing a little as its neighbours drop out — so the meadow thins with distance
// without an edge.
//
// ── Targets ─────────────────────────────────────────────────────────────────
//
// The pipeline declares all EIGHT G-buffer targets so it can be fused into the GBuffer
// subpass chain. Grass writes six of them; the other two carry `ColorWrites::empty()` on
// the Rust side and are simply absent from `FoliageGBufferOutput` below:
//
//   0 albedo       Rgba8Unorm   written
//   1 normal       Rgba16Float  written
//   2 orm          Rgba8Unorm   written
//   3 emissive     Rgba16Float  written
//   4 lightmap_uv  Rg16Float    written: (-1, -1.5), foliage's tag (no lightmap, no SSAO)
//   5 sss          Rgba16Float  NOT written
//   6 extra        Rgba16Float  NOT written
//   7 velocity     Rgba16Float  written
//
// The empty write masks are mandatory, not cosmetic: a fragment target with no
// corresponding shader output has an UNDEFINED value.
//
// ── Motion vectors ──────────────────────────────────────────────────────────
//
// The wind is evaluated for two times per vertex (`helio_wind_offset_pair`), at
// `globals.clock.x` and at `globals.clock.y` (the frame clock and its previous value), producing a current and
// a previous world position. Without the pair every blade reports zero motion and TAA
// smears moving grass.

// ═══════════════════════════════════════════════════════════════════════════════
// Constants
// ═══════════════════════════════════════════════════════════════════════════════

/// Interactors a vertex looks at, at most (`FOLIAGE_MAX_INTERACTORS`).
const FOLIAGE_MAX_INTERACTORS: u32 = 16u;
/// The angle (radians) foliage lies over at most under an interactor.
const FOLIAGE_PRESS_ANGLE: f32 = 1.35;

/// Mirrors `FLAG_DEBUG_LOD` in `src/lib.rs`. Tints blades by LOD instead of shading them.
const FOLIAGE_FLAG_DEBUG_LOD: u32 = 2u;

/// Shift/mask for the packed `visible_blades[]` entry. See `VISIBLE_TILE_SHIFT` in
/// `src/lib.rs` — this encoding is half of the contract with `FoliagePlacePass`.
const FOLIAGE_VISIBLE_TILE_SHIFT: u32 = 16u;
const FOLIAGE_VISIBLE_LOCAL_MASK: u32 = 0xffffu;

/// The boxes a blocky blade draws at most at L0 and L1, and the sideways steps an L1
/// blade takes at most (its runs fit 5 boxes; L0's are `FOLIAGE_STACK_STEPS`).
const FOLIAGE_L0_BOXES: u32 = 8u;
const FOLIAGE_L1_BOXES: u32 = 5u;
const FOLIAGE_L1_STEPS: u32 = 2u;
/// How far blocky foliage sits off its cube lattice (the terrain's), in cubes.
const FOLIAGE_LATTICE_OFFSET: f32 = 0.02;

/// How far a blocky blade's shading normal leans from the ground's up towards its face's
/// own: enough to tell the box's faces apart, little enough that a meadow lights like the
/// ground under it from any side.
const BLOCK_FACE_TILT: f32 = 0.4;

/// A smooth tuft's base: its blades root within this many metres of its centre.
const FOLIAGE_TUFT_BASE: f32 = 0.03;

/// Grass is a dielectric; 0.04 is the standard non-metal F0, packed into the spare alpha
/// channels exactly the way `gbuffer.wgsl` does.
const FOLIAGE_F0: f32 = 0.04;
const FOLIAGE_ROUGHNESS: f32 = 0.75;
const TAU: f32 = 6.283185307179586;

// ═══════════════════════════════════════════════════════════════════════════════
// Uniforms and buffers
// ═══════════════════════════════════════════════════════════════════════════════

/// Mirror of `FoliageGlobals` in `src/lib.rs` (48 bytes).
struct FoliageGlobals {
    /// Render target size in pixels; the velocity target is in pixels/frame.
    screen_size: vec2<f32>,
    /// Frame index.
    frame: u32,
    /// `FOLIAGE_FLAG_*` bits.
    flags: u32,
    /// xy = the wind clock (the frame clock t, and t - dt), zw unused.
    clock: vec4<f32>,
    /// `FoliageQuality::lod_distance_scale`, already sanitised on the CPU.
    lod_quality_scale: f32,
    /// Width of the band (metres) the near tufts switch from L0 to L1 across.
    lod_fade_band: f32,
    /// Global multiplier on how far interactors press foliage over.
    interaction_strength: f32,
    /// Elements per `visible_blades` region (a LOD's).
    region_stride: u32,
}

/// An interactor as `FoliagePlacePass` gathers them (`foliage_interactors.wgsl`):
/// render-space position and radius, velocity and wake (seconds), strength (x).
struct Interactor {
    at_radius: vec4<f32>,
    velocity_wake: vec4<f32>,
    strength: vec4<f32>,
}

struct Interactors {
    count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    rows: array<Interactor, FOLIAGE_MAX_INTERACTORS>,
}

/// Mirror of `FoliageLodUniform` in `src/lib.rs` (32 bytes), bound with a dynamic offset
/// so the four draws share one pipeline and one buffer.
struct FoliageLod {
    lod: u32,
    segments: u32,
    vertex_count: u32,
    /// First element of this LOD's region in `visible_blades[]`.
    region_base: u32,
    width_scale: f32,
    height_scale: f32,
    is_card: u32,
    _pad: u32,
}

@group(0) @binding(0) var<storage, read> cameras: array<Camera, 2>;
@group(0) @binding(1) var<uniform> globals: FoliageGlobals;
@group(0) @binding(2) var<storage, read> wind: Wind;
@group(0) @binding(3) var<storage, read> foliage_types: array<FoliageType>;
@group(0) @binding(4) var<storage, read> blade_arena: array<BladeInstance>;
@group(0) @binding(5) var<storage, read> tile_table: array<FoliageTile>;
@group(0) @binding(6) var<storage, read> visible_blades: array<u32>;
@group(0) @binding(7) var<storage, read> interactors: Interactors;
@group(0) @binding(9) var<storage, read> tile_frames: array<TileFrame>;
@group(0) @binding(10) var<uniform> foliage_bands: FoliageBands;

@group(1) @binding(0) var<uniform> lod_info: FoliageLod;

/// Blocky foliage as boxes: `cs_boxes` writes them (group 2), `vs_box` draws them.
struct FoliageBox {
    /// Render-space centre across the ground, at the shape's root height; its tile (the
    /// basis).
    centre: vec3<f32>,
    slot: u32,
    /// Half extents along the tile's u and v.
    half: vec2<f32>,
    /// Bottom and top over the root (f16 pair).
    heights: u32,
    /// The shape's height and the height fraction its tip colour starts at (f16 pair).
    shape: u32,
    /// Displacement this frame (wind, interactors), render space.
    now: vec3<f32>,
    /// The ground's colour (RGB565; 0 none), the type (bits 16..24), the LOD (24..26).
    look: u32,
    /// Displacement last frame.
    prev: vec3<f32>,
    /// Tint (two unorm8).
    tint: u32,
}

/// The boxes so far, those that did not fit, and the box draw's indirect arguments.
struct BoxState {
    count: atomic<u32>,
    overflow: atomic<u32>,
    _pad0: u32,
    _pad1: u32,
    vertex_count: u32,
    instance_count: u32,
    first_vertex: u32,
    first_instance: u32,
}

@group(0) @binding(11) var<storage, read> boxes: array<FoliageBox>;
@group(2) @binding(0) var<storage, read_write> boxes_out: array<FoliageBox>;
@group(2) @binding(1) var<storage, read_write> box_state: BoxState;
/// The cull's draws (`DrawIndirectArgs` per LOD): each LOD's visible records.
@group(2) @binding(2) var<storage, read> lod_draws: array<vec4<u32>>;

// ═══════════════════════════════════════════════════════════════════════════════
// Small helpers
// ═══════════════════════════════════════════════════════════════════════════════

/// A blade's frame in render space: the tile's along-u direction, its up, and their cross
/// product (world X, Y and Z on a heightfield).
struct BladeBasis {
    tangent: vec3<f32>,
    up: vec3<f32>,
    bitangent: vec3<f32>,
}

fn foliage_basis(frame: TileFrame) -> BladeBasis {
    let up = normalize(frame.up);
    let along = frame.axis_u - up * dot(frame.axis_u, up);
    let tangent = normalize(select(vec3<f32>(1.0, 0.0, 0.0), along, dot(along, along) > 1.0e-12));
    return BladeBasis(tangent, up, cross(tangent, up));
}

fn foliage_to_world(b: BladeBasis, local: vec3<f32>) -> vec3<f32> {
    return b.tangent * local.x + b.up * local.y + b.bitangent * local.z;
}

/// How the foliage rooted at `root` (render space, `up` its ground's up) lies over under
/// the interactors `back` seconds ago: xyz the unit direction across the ground it leans
/// to, w the angle (radians). Each interactor presses a capsule: from where it is back
/// along its wake (`velocity × wake`), fully at its centre and less along the wake, so
/// grass stays down in its track and rises behind it. Plain arithmetic in render space
/// about the ground's own up: any ground, any planet.
fn foliage_press(root: vec3<f32>, up: vec3<f32>, back: f32, response: f32) -> vec4<f32> {
    var push = vec3<f32>(0.0);
    let count = min(interactors.count, FOLIAGE_MAX_INTERACTORS);
    for (var i = 0u; i < count; i++) {
        let it = interactors.rows[i];
        let radius = it.at_radius.w;
        if radius <= 0.0 {
            continue;
        }
        let velocity = it.velocity_wake.xyz;
        let at = it.at_radius.xyz - velocity * back;
        let track = -velocity * clamp(it.velocity_wake.w, 0.0, 10.0);
        let length2 = dot(track, track);
        let t = select(0.0, clamp(dot(root - at, track) / max(length2, 1.0e-8), 0.0, 1.0), length2 > 1.0e-6);
        let d = root - (at + track * t);
        let rise = dot(d, up);
        let across = d - up * rise;
        let far = length(across);
        let weight = (1.0 - smoothstep(0.3 * radius, radius, far))
            * (1.0 - smoothstep(0.5 * radius, 1.5 * radius, abs(rise)))
            * (1.0 - smoothstep(0.0, 1.0, t));
        if weight <= 0.0 {
            continue;
        }
        // Away from its path, and on along its motion where it walks over the blade.
        let moving = velocity - up * dot(velocity, up);
        let ahead = select(vec3<f32>(0.0), normalize(moving), dot(moving, moving) > 1.0e-6);
        let away = select(vec3<f32>(0.0), across / far, far > 1.0e-4) * min(far / (0.3 * radius), 1.0);
        var direction = away + 0.6 * ahead;
        if dot(direction, direction) < 1.0e-6 {
            direction = cross(up, vec3<f32>(0.37, 0.0, 0.93));
        }
        push += normalize(direction) * weight * it.strength.x;
    }
    let amount = length(push);
    if amount <= 0.0 {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(push / amount, FOLIAGE_PRESS_ANGLE * min(amount * response, 1.0));
}

/// Where a point `height` metres over its root goes as its blade lies over by `press`
/// (`foliage_press`): round the root, over towards the press, never lower than `lowest`
/// under where it stands.
fn foliage_pressed(press: vec4<f32>, up: vec3<f32>, height: f32, lowest: f32) -> vec3<f32> {
    if press.w <= 0.0 {
        return vec3<f32>(0.0);
    }
    return press.xyz * (height * sin(press.w)) + up * max(height * (cos(press.w) - 1.0), -lowest);
}

// ═══════════════════════════════════════════════════════════════════════════════
// Blades of a tuft
// ═══════════════════════════════════════════════════════════════════════════════

// ── Smooth blades ───────────────────────────────────────────────────────────────

/// A point of a smooth blade's spine at `s` (0 root, 1 tip) in its own frame (+z its lean
/// direction, +y up), and the spine's unit tangent there: it leans by `lean` and bends a
/// further `curve` towards the tip, keeping its length.
struct Spine {
    at: vec3<f32>,
    tangent: vec3<f32>,
}

fn blade_spine(b: Blade, height: f32, s: f32) -> Spine {
    let slope = b.lean + 2.0 * b.curve * s;
    let reach = height * (b.lean * s + b.curve * s * s);
    let rise = height * s / sqrt(1.0 + (b.lean + b.curve * s) * (b.lean + b.curve * s));
    return Spine(vec3<f32>(0.0, rise, reach), normalize(vec3<f32>(0.0, 1.0, slope)));
}

/// Turn a blade-frame offset (+z its lean direction) by its angle about +y.
fn blade_turn(local: vec3<f32>, angle: f32) -> vec3<f32> {
    let s = sin(angle);
    let c = cos(angle);
    return vec3<f32>(local.x * c + local.z * s, local.y, -local.x * s + local.z * c);
}

/// Vertex `k` of a smooth blade drawn as a strip of `segments` (a collapsed tip), or as a
/// straight card (4 vertices) when `card`: tile-local offset from the tuft's root, normal,
/// height fraction.
struct SmoothVertex {
    local: vec3<f32>,
    normal: vec3<f32>,
    height_frac: f32,
}

fn smooth_blade_vertex(b: Blade, scale: f32, segments: u32, card: bool, k: u32) -> SmoothVertex {
    let height = b.height * scale;
    let width = b.width * scale;
    let tip = 2u * segments;
    var row = min(k >> 1u, segments);
    var side = select(-1.0, 1.0, (k & 1u) == 1u);
    if !card && k >= tip {
        row = segments;
        side = 0.0;
    }
    let s = f32(row) / f32(max(segments, 1u));
    var spine = blade_spine(b, height, s);
    var taper = 1.0 - s * s;
    if card {
        // A card stands straight from root to tip, its width tapering to half.
        let tip_at = blade_spine(b, height, 1.0).at;
        spine.at = tip_at * s;
        spine.tangent = normalize(tip_at);
        taper = 1.0 - 0.5 * s;
    }
    // The root sits out from the tuft's centre along the blade's direction.
    let root = vec3<f32>(0.0, 0.0, FOLIAGE_TUFT_BASE * b.rim * scale);
    let local = root + spine.at + vec3<f32>(side * 0.5 * width * taper, 0.0, 0.0);
    // Normal: across the blade (x) crossed with its spine, facing forward and up.
    let normal = normalize(vec3<f32>(0.0, -spine.tangent.z, spine.tangent.y));
    return SmoothVertex(blade_turn(local, b.angle), blade_turn(normal, b.angle), s);
}

// ── Blocky blades ───────────────────────────────────────────────────────────────

/// A grid compass step: the eight neighbours of a cube, axis steps first.
fn compass_step(i: u32) -> vec2<f32> {
    var steps = array<vec2<f32>, 8>(
        vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0), vec2<f32>(-1.0, 0.0), vec2<f32>(0.0, -1.0),
        vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0), vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0),
    );
    return steps[i & 7u];
}

/// One box of a blocky shape, in tile-local metres: its centre across the ground (x, z),
/// its half extents across it, its bottom and top.
struct Box {
    centre: vec2<f32>,
    half: vec2<f32>,
    y0: f32,
    y1: f32,
}

/// The sideways step the path has reached after rising to cube `v` of `rise`: it leans
/// out by `lean` and bends by `curve` over the rise, reaching `side` steps at the top.
fn block_target(b: Blade, v: u32, rise: u32, side: u32) -> u32 {
    if rise <= 1u || side == 0u {
        return select(0u, side, rise <= 1u);
    }
    let t = f32(v) / f32(rise - 1u);
    let g = (b.lean * t + b.curve * t * t) / max(b.lean + b.curve, 1.0e-4);
    return min(u32(round(f32(side) * clamp(g, 0.0, 1.0))), side);
}

/// The level (cube up) a blocky blade takes its `h`-th sideways step at: the first whose
/// target (`block_target`) reaches `h`, else the top. From the lean curve's inverse, then
/// checked against the target itself, so it is exactly where a cube-by-cube walk steps
/// (`geometry::block_level`).
fn block_level(b: Blade, h: u32, rise: u32, side: u32) -> u32 {
    if rise <= 1u {
        return 0u;
    }
    let top = rise - 1u;
    let q = (f32(h) - 0.5) / f32(max(side, 1u)) * max(b.lean + b.curve, 1.0e-4);
    var t = 1.0;
    if b.curve > 1.0e-6 {
        t = (-b.lean + sqrt(max(b.lean * b.lean + 4.0 * b.curve * q, 0.0))) / (2.0 * b.curve);
    } else if b.lean > 1.0e-6 {
        t = q / b.lean;
    }
    var v = min(u32(max(ceil(t * f32(top)), 0.0)), top);
    loop {
        if v == 0u || block_target(b, v - 1u, rise, side) < h { break; }
        v -= 1u;
    }
    loop {
        if v >= top || block_target(b, v, rise, side) >= h { break; }
        v += 1u;
    }
    return v;
}

/// Box `want` of a blocky blade (at most `max_steps` sideways), and how many there are.
/// The blade is a connected staircase of cubes: from the root cube, each next one up or
/// one step along its compass direction, stepping out as its lean demands. A run up a
/// column is one box (no hidden tops inside it), an axis step's cubes at one level one
/// box, diagonal steps a cube each. Blades past the tuft's first share its root column,
/// so they start at their first sideways step (no two blades draw one cube). Found from
/// the levels of the (at most four) sideways steps, not by walking cube by cube
/// (`geometry::block_boxes_by_levels`).
struct BlockPath {
    found: Box,
    boxes: u32,
}

fn block_path(b: Blade, j: u32, cube: f32, scale: f32, leafy: bool, want: u32, max_steps: u32) -> BlockPath {
    var out = BlockPath(Box(vec2<f32>(0.0), vec2<f32>(0.0), 0.0, 0.0), 0u);
    let steps = block_steps(b, cube, scale, max_steps);
    let rise = steps.x;
    // A blade past the first steps out of the shared root column at least once, or it
    // would hide in it.
    let side = select(steps.y, max(steps.y, 1u), j > 0u);
    if rise == 0u {
        return out;
    }
    let dir = compass_step(b.compass);
    let diagonal = b.compass >= 4u;
    // Blades past the eighth nest inside the first eight's cubes: the same cube a hair
    // smaller never fights it for depth.
    let size = cube * select(1.0, 0.86, j >= 8u);
    let height = cube * select(1.0, 0.6, leafy);
    // A leaf is a plate across its direction (the type's width), a blade a cube; along
    // the direction a run up is a cube.
    let across = select(size, max(b.width, size), leafy);
    let perp = vec2<f32>(abs(dir.y), abs(dir.x)) * 0.5 * across;
    let column_half = select(max(abs(dir) * (0.5 * size), perp), vec2<f32>(0.5 * size), diagonal);
    var h = 0u;
    var start_v = 0u;
    var step = 1u;
    var level = 0u;
    if side > 0u {
        level = block_level(b, 1u, rise, side);
    }
    loop {
        if step > side {
            break;
        }
        // Up the column to this step's level (the shared root column is the first
        // blade's).
        if h > 0u || j == 0u {
            if out.boxes == want {
                out.found = Box(dir * f32(h) * size, column_half, f32(start_v) * height, f32(level + 1u) * height);
            }
            out.boxes += 1u;
        }
        // The steps taken at this level, and the next level.
        var last = step;
        var next = rise;
        loop {
            if last >= side {
                break;
            }
            next = block_level(b, last + 1u, rise, side);
            if next != level {
                break;
            }
            last += 1u;
        }
        let y0 = f32(level) * height;
        if diagonal {
            if want >= out.boxes && want <= out.boxes + (last - step) {
                out.found = Box(dir * f32(step + want - out.boxes) * size, vec2<f32>(0.5 * size), y0, y0 + height);
            }
            out.boxes += last - step + 1u;
        } else {
            if out.boxes == want {
                let first = dir * f32(step) * size;
                let extent = dir * f32(last - step) * size;
                let along = abs(dir) * (0.5 * size) + abs(extent) * 0.5;
                out.found = Box(first + extent * 0.5, max(along, perp), y0, y0 + height);
            }
            out.boxes += 1u;
        }
        h = last;
        start_v = level + 1u;
        step = last + 1u;
        level = next;
    }
    // Up to the top.
    if start_v < rise && (h > 0u || j == 0u) {
        if out.boxes == want {
            out.found = Box(dir * f32(h) * size, column_half, f32(start_v) * height, f32(rise) * height);
        }
        out.boxes += 1u;
    }
    return out;
}

/// Corner `k` (0..8) of the three faces of `bx` turned to the eye (`eye`, tile-local
/// x and z): `Q0 Q1 P0 P1 R0 R1 P1 S1 Q1` — P the edge between the two sides turned to
/// the eye, Q and R their far edges, S the top's far corner — and the normal of the
/// triangle the vertex provokes (the first of each: flat faces).
struct BoxVertex {
    local: vec3<f32>,
    normal: vec3<f32>,
}

fn box_vertex(bx: Box, eye: vec2<f32>, k: u32) -> BoxVertex {
    let sx = select(-1.0, 1.0, eye.x >= bx.centre.x);
    let sz = select(-1.0, 1.0, eye.y >= bx.centre.y);
    var corners = array<vec3<f32>, 9>(
        vec3<f32>(-sx, 0.0, sz), vec3<f32>(-sx, 1.0, sz),
        vec3<f32>(sx, 0.0, sz), vec3<f32>(sx, 1.0, sz),
        vec3<f32>(sx, 0.0, -sz), vec3<f32>(sx, 1.0, -sz),
        vec3<f32>(sx, 1.0, sz), vec3<f32>(-sx, 1.0, -sz),
        vec3<f32>(-sx, 1.0, sz),
    );
    var normals = array<vec3<f32>, 9>(
        vec3<f32>(0.0, 0.0, sz), vec3<f32>(0.0, 0.0, sz),
        vec3<f32>(sx, 0.0, 0.0), vec3<f32>(sx, 0.0, 0.0), vec3<f32>(sx, 0.0, 0.0),
        vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 1.0, 0.0),
        vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 1.0, 0.0),
    );
    let c = corners[min(k, 8u)];
    return BoxVertex(
        vec3<f32>(bx.centre.x + c.x * bx.half.x, mix(bx.y0, bx.y1, c.y), bx.centre.y + c.z * bx.half.y),
        normals[min(k, 8u)],
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Vertex stage
// ═══════════════════════════════════════════════════════════════════════════════

struct VertexOutput {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) world_normal: vec3<f32>,
    @location(1) height_frac: f32,
    @location(2) prev_clip_position: vec4<f32>,
    @location(3) tint: vec2<f32>,
    /// How far the vertex moved by itself since last frame (m): in the wind,
    /// pushed aside. 0: still, it moves with the camera.
    @location(4) moved: f32,
    /// LOD this instance was drawn at, for the `FOLIAGE_FLAG_DEBUG_LOD` view.
    @location(6) @interpolate(flat) lod: u32,
    /// The type's colour word (RGBA8; 0: procedural) and roughness/metallic word.
    @location(7) @interpolate(flat) material: vec2<u32>,
    /// A blocky shape's face normal (the provoking vertex's), and whether it is blocky
    /// (then `world_normal` is the ground's up, the two shading it together).
    @location(8) @interpolate(flat) face_normal: vec3<f32>,
    @location(9) @interpolate(flat) blocky: u32,
    /// The ground's colour under the tuft (w 1) when the type takes it, and the tip colour
    /// (RGBA8; 0: none) with the height fraction it starts at (f32 bits).
    @location(10) @interpolate(flat) ground_colour: vec4<f32>,
    @location(11) @interpolate(flat) tip: vec2<u32>,
}

/// The vertex every position past a strip's end takes: all of them one point, a zero-area
/// tail.
fn tail() -> VertexOutput {
    var out: VertexOutput;
    out.clip_position = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    return out;
}

/// A visible record and what every shape drawn from it starts from.
struct Instance {
    drawn: bool,
    record: BladeInstance,
    slot: u32,
    basis: BladeBasis,
    root: vec3<f32>,
    ty: FoliageType,
    blocky: bool,
    n: u32,
    scale: f32,
}

/// Visible record `index` of LOD `lod` (its region at `region_base`): its tile, frame
/// and type, and the scale it draws at; not drawn where its band has faded it, the kept
/// share has dropped it, the type's reach has ended, or (the near band) the other LOD
/// draws it: tufts switch from L0 to L1 a tuft at a time, by their own hash against the
/// share of the switch band behind them (no dithered, see-through blades).
fn foliage_instance(lod: u32, region_base: u32, index: u32) -> Instance {
    var out: Instance;
    let reference = visible_blades[region_base + index];
    out.slot = reference >> FOLIAGE_VISIBLE_TILE_SHIFT;
    let tile = tile_table[out.slot];
    out.record = blade_arena[tile.blade_offset + (reference & FOLIAGE_VISIBLE_LOCAL_MASK)];
    let frame = tile_frames[out.slot];
    out.basis = foliage_basis(frame);
    out.root = foliage_blade_root(tile, frame, out.record);
    out.ty = foliage_types[(out.record.packed_scale_type >> 16u) & 0xffu];
    out.blocky = (out.ty.kind_and_flags & 0xffu) == FOLIAGE_KIND_BLOCK;
    out.n = foliage_tuft_blades(out.record);
    let band = foliage_bands.bands[foliage_band_of_slot(out.slot)];
    let d = distance(out.root, cameras[0].position_near.xyz);
    out.scale = foliage_tuft_scale(band, d, foliage_tuft_rank(out.record)) * foliage_type_reach(out.ty, d);
    out.drawn = out.scale > 0.0;
    if lod <= 1u {
        let boundary = out.ty.lod0 * globals.lod_quality_scale;
        let f = smoothstep(boundary - globals.lod_fade_band, boundary, d);
        let t = foliage_unit(foliage_hash((out.record.packed_colour_seed >> 16u) ^ 0x6c8e9cf5u));
        out.drawn = out.drawn && ((lod == 0u) == (t < 1.0 - f));
    }
    return out;
}

/// Smooth foliage, a strip a record: L0 and L1 a blade, L2 a tuft of cards, L3 a tuft as
/// three broad blades. Blocky foliage is drawn as boxes (`cs_boxes`, `vs_box`).
@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    // `instance_index` is relative to this draw (the producer writes `first_instance =
    // 0`); the LOD's region base comes from the per-LOD uniform.
    let lod = lod_info.lod;
    let it = foliage_instance(lod, lod_info.region_base, instance_index);
    if it.blocky || !it.drawn {
        return tail();
    }
    let ty = it.ty;
    let record = it.record;
    let basis = it.basis;
    let root = it.root;
    let n = it.n;
    let scale = it.scale;

    var local = vec3<f32>(0.0);
    var normal = vec3<f32>(0.0, 1.0, 0.0);
    var height_frac = 0.0;
    var blade_seed = record.packed_colour_seed >> 16u;
    var tint = vec2<f32>(0.5);

    if lod <= 1u {
        // One smooth blade of the tuft.
        let j = foliage_blade_index(record);
        let b = tuft_blade(ty, record, j, n);
        let segments = select(3u, 5u, lod == 0u);
        let own = 2u * segments + 1u;
        if vertex_index > own {
            return tail();
        }
        let v = smooth_blade_vertex(b, scale, segments, false, min(vertex_index, own - 1u));
        local = v.local;
        normal = v.normal;
        height_frac = v.height_frac;
        blade_seed = b.seed;
        tint = b.tint;
    } else if lod == 2u {
        // A whole smooth tuft, each blade a card: 4 vertices and 2 joins each.
        let j = vertex_index / 6u;
        var k = vertex_index % 6u;
        if j >= n || (j + 1u == n && k > 4u) {
            return tail();
        }
        var blade_j = j;
        if k == 4u {
            k = 3u;
        } else if k == 5u {
            blade_j = j + 1u;
            k = 0u;
        }
        let b = tuft_blade(ty, record, blade_j, n);
        let v = smooth_blade_vertex(b, scale, 1u, true, k);
        local = v.local;
        normal = v.normal;
        height_frac = v.height_frac;
        tint = b.tint;
    } else {
        // A smooth tuft as one shape: three broad blades fanned out (3 vertices each, 2
        // joins between).
        let j = vertex_index / 5u;
        var k = vertex_index % 5u;
        if j >= 3u || (j == 2u && k > 2u) {
            return tail();
        }
        var fan = j;
        if k == 3u {
            k = 2u;
        } else if k == 4u {
            fan = j + 1u;
            k = 0u;
        }
        let tuft_height = mix(ty.height_min, ty.height_max, f32(record.packed_scale_type & 0xffu) / 255.0) * scale;
        let spread = (FOLIAGE_TUFT_BASE + 0.25 * tuft_height * mix(ty.lean_min, ty.lean_max, 0.6)) * sqrt(f32(n)) + mix(ty.width_min, ty.width_max, 0.5) * scale;
        let angle = TAU * (f32((record.packed_scale_type >> 8u) & 0xffu) / 256.0 + f32(fan) / 3.0);
        var corners = array<vec3<f32>, 3>(
            vec3<f32>(-spread, 0.0, 0.0),
            vec3<f32>(spread, 0.0, 0.0),
            vec3<f32>(0.0, tuft_height * 0.85, 0.25 * tuft_height),
        );
        local = blade_turn(corners[k], angle);
        normal = blade_turn(normalize(vec3<f32>(0.0, 0.3, 1.0)), angle);
        height_frac = select(0.0, 1.0, k == 2u);
    }

    let world_base = root + foliage_to_world(basis, local);
    // The wind at the vertex, never lowering it under the ground (its arc-length drop
    // assumes an upright stem; a blade lying over would sink through in a gust).
    let response = vec3<f32>(ty.wind_trunk, ty.wind_branch, ty.wind_leaf);
    let sway = helio_wind_offset_pair(wind, world_base, root, height_frac, blade_seed, response, globals.clock.x, globals.clock.y);
    var sway_now = sway.now;
    var sway_prev = sway.prev;
    sway_now.y = max(sway_now.y, -local.y);
    sway_prev.y = max(sway_prev.y, -local.y);
    // Pushed aside by interactors, now and last frame (for the motion vectors): the blade
    // bends round its root.
    var bend = vec3<f32>(0.0);
    var bend_prev = vec3<f32>(0.0);
    if (ty.kind_and_flags & FOLIAGE_TYPE_FLAG_RECEIVES_INTERACTION) != 0u {
        let response = globals.interaction_strength / max(ty.interaction_stiffness, 0.25);
        bend = foliage_pressed(foliage_press(root, basis.up, 0.0, response), basis.up, local.y, local.y);
        bend_prev = foliage_pressed(foliage_press(root, basis.up, globals.clock.x - globals.clock.y, response), basis.up, local.y, local.y);
    }

    var out: VertexOutput;
    out.clip_position = cameras[0].view_proj * vec4<f32>(world_base + foliage_to_world(basis, sway_now) + bend, 1.0);
    out.prev_clip_position = cameras[0].prev_view_proj * vec4<f32>(world_base + foliage_to_world(basis, sway_prev) + bend_prev, 1.0);
    out.moved = length(foliage_to_world(basis, sway_now - sway_prev) + bend - bend_prev);
    out.world_normal = foliage_to_world(basis, normal);
    out.face_normal = out.world_normal;
    out.blocky = 0u;
    out.height_frac = clamp(height_frac, 0.0, 1.0);
    out.tint = tint;
    out.lod = lod;
    out.material = vec2<u32>(ty.base_color, ty.roughness_metallic);
    out.ground_colour = select(vec4<f32>(0.0), foliage_ground_colour(record), (ty.kind_and_flags & FOLIAGE_TYPE_FLAG_GROUND_COLOUR) != 0u);
    out.tip = vec2<u32>(ty.tip_color, bitcast<u32>(0.72));
    return out;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Blocky foliage: boxes
// ═══════════════════════════════════════════════════════════════════════════════
//
// A blocky shape is boxes (a blade's runs of cubes, a tuft's blades, a far tuft's
// crossed pair), each drawn as the three faces turned to the eye. Everything a box shares
// (the record, the blade, its staircase, the wind and interactors at its centre) is
// worked out once a box in `cs_boxes`, not once a vertex: `vs_box` reads it and places
// nine corners.

/// `count` boxes' room in `boxes_out` (its first), or the overflow counted.
fn foliage_box_room(count: u32) -> u32 {
    let first = atomicAdd(&box_state.count, count);
    let room = arrayLength(&boxes_out);
    if first + count > room {
        atomicAdd(&box_state.overflow, first + count - min(first + count, max(first, room)));
    }
    return first;
}

/// Box `bx` of a shape of record `it` (its height `total`, its tip colour from
/// `tip_from`), at `at`: its centre in render space and its displacement by the wind (at
/// the box's centre, whole, never under the ground) and the interactors, now and last
/// frame.
fn foliage_box_write(at: u32, it: Instance, bx: Box, total: f32, tip_from: f32, seed: u32, tint: vec2<f32>, lod: u32) {
    if at >= arrayLength(&boxes_out) {
        return;
    }
    let ty = it.ty;
    let anchor = vec3<f32>(bx.centre.x, 0.5 * (bx.y0 + bx.y1), bx.centre.y);
    let response = vec3<f32>(ty.wind_trunk, ty.wind_branch, ty.wind_leaf);
    let sway = helio_wind_offset_pair(wind, it.root + foliage_to_world(it.basis, anchor), it.root, anchor.y / max(total, 1.0e-3), seed, response, globals.clock.x, globals.clock.y);
    var now = sway.now;
    var prev = sway.prev;
    now.y = max(now.y, -bx.y0);
    prev.y = max(prev.y, -bx.y0);
    var moved = foliage_to_world(it.basis, now);
    var moved_prev = foliage_to_world(it.basis, prev);
    if (ty.kind_and_flags & FOLIAGE_TYPE_FLAG_RECEIVES_INTERACTION) != 0u {
        let pressed = globals.interaction_strength / max(ty.interaction_stiffness, 0.25);
        moved += foliage_pressed(foliage_press(it.root, it.basis.up, 0.0, pressed), it.basis.up, anchor.y, bx.y0);
        moved_prev += foliage_pressed(foliage_press(it.root, it.basis.up, globals.clock.x - globals.clock.y, pressed), it.basis.up, anchor.y, bx.y0);
    }
    var ground = 0u;
    if (ty.kind_and_flags & FOLIAGE_TYPE_FLAG_GROUND_COLOUR) != 0u {
        ground = it.record.packed_colour_seed & 0xffffu;
    }
    // Off the cube lattice by a hair: a box's faces lie on its grid, which is the
    // terrain's, and a face coplanar with the turf's (a blade leaning into a step, its top
    // level with the step's) would win and lose the depth test as the TSR jitter moves.
    let off = FOLIAGE_LATTICE_OFFSET * max(ty.width_min, 1.0e-3);
    boxes_out[at] = FoliageBox(
        it.root + foliage_to_world(it.basis, vec3<f32>(bx.centre.x + off, off, bx.centre.y + off)),
        it.slot,
        bx.half,
        pack2x16float(vec2<f32>(bx.y0, bx.y1)),
        pack2x16float(vec2<f32>(total, tip_from)),
        moved,
        ground | (((it.record.packed_scale_type >> 16u) & 0xffu) << 16u) | ((lod & 3u) << 24u),
        moved_prev,
        pack4x8unorm(vec4<f32>(tint, 0.0, 0.0)),
    );
}

/// The boxes of visible record `index` of LOD `lod`, if its type is blocky: L0 and L1 a
/// blade's runs of cubes (at L1 with fewer jogs sideways), L2 a tuft's blades a box each
/// from root to top, L3 a tuft as two crossed boxes as wide as its blades reach.
fn foliage_expand(lod: u32, index: u32) {
    let it = foliage_instance(lod, lod * globals.region_stride, index);
    if !it.blocky || !it.drawn {
        return;
    }
    let ty = it.ty;
    let cube = max(ty.width_min, 1.0e-3);
    let leafy = ty.width_max > 1.5 * ty.width_min;
    let tall = select(1.0, 0.6, leafy);
    if lod <= 1u {
        let j = foliage_blade_index(it.record);
        let b = tuft_blade(ty, it.record, j, it.n);
        let max_steps = select(FOLIAGE_L1_STEPS, FOLIAGE_STACK_STEPS, lod == 0u);
        let steps = block_steps(b, cube, it.scale, max_steps);
        if steps.x == 0u {
            return;
        }
        let count = min(block_path(b, j, cube, it.scale, leafy, 0xffffffffu, max_steps).boxes, select(FOLIAGE_L1_BOXES, FOLIAGE_L0_BOXES, lod == 0u));
        let total = f32(steps.x) * cube * tall;
        let tip_from = 1.0 - 1.0 / f32(steps.x) - 1.0e-3;
        let first = foliage_box_room(count);
        for (var k = 0u; k < count; k++) {
            foliage_box_write(first + k, it, block_path(b, j, cube, it.scale, leafy, k, max_steps).found, total, tip_from, b.seed, b.tint, lod);
        }
    } else if lod == 2u {
        let first = foliage_box_room(it.n);
        for (var s = 0u; s < it.n; s++) {
            let b = tuft_blade(ty, it.record, s, it.n);
            let steps = block_steps(b, cube, it.scale, FOLIAGE_STACK_STEPS);
            let dir = compass_step(b.compass);
            let half_out = 0.5 * f32(steps.y) * cube;
            let across = select(cube, max(b.width, cube), leafy);
            let top = f32(steps.x) * cube * tall;
            let bx = Box(
                dir * half_out,
                max(abs(dir) * (0.5 * cube + half_out * 0.5), vec2<f32>(abs(dir.y), abs(dir.x)) * 0.5 * across) * select(0.0, 1.0, steps.x > 0u),
                0.0,
                top,
            );
            foliage_box_write(first + s, it, bx, top, 1.0 - 1.0 / f32(max(steps.x, 1u)) - 1.0e-3, it.record.packed_colour_seed >> 16u, b.tint, lod);
        }
    } else {
        let tuft_height = mix(ty.height_min, ty.height_max, f32(it.record.packed_scale_type & 0xffu) / 255.0) * it.scale;
        // At least a pixel and a half across: thinner, a far tuft is a sliver the
        // jittered samples find on some frames and miss on others (shimmer at the
        // horizon); as wide, it covers the share of the ground its tufts would.
        let pixel = distance(it.root, cameras[0].position_near.xyz) * 2.0 / max(cameras[0].proj[1][1] * globals.screen_size.y, 1.0e-6);
        let reach = max(0.5 * cube + tuft_height * mix(ty.lean_min, ty.lean_max, 0.6) * 0.8, 0.75 * pixel);
        let first = foliage_box_room(2u);
        for (var k = 0u; k < 2u; k++) {
            let half = select(vec2<f32>(0.5 * cube, reach), vec2<f32>(reach, 0.5 * cube), k == 0u);
            foliage_box_write(first + k, it, Box(vec2<f32>(0.0), half, 0.0, tuft_height * 0.8), tuft_height * 0.8, 0.72, it.record.packed_colour_seed >> 16u, vec2<f32>(0.5), lod);
        }
    }
}

/// Every LOD's visible records (`lod_draws`' instance counts, one after another), the
/// grid's threads striding over them.
@compute @workgroup_size(64)
fn cs_boxes(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) groups: vec3<u32>) {
    if arrayLength(&lod_draws) < 4u {
        return;
    }
    let counts = vec4<u32>(lod_draws[0].y, lod_draws[1].y, lod_draws[2].y, lod_draws[3].y);
    let total = counts.x + counts.y + counts.z + counts.w;
    let stride = groups.x * 64u;
    for (var i = id.x; i < total; i += stride) {
        var lod = 0u;
        var index = i;
        if index >= counts.x {
            index -= counts.x;
            lod = 1u;
            if index >= counts.y {
                index -= counts.y;
                lod = 2u;
                if index >= counts.z {
                    index -= counts.z;
                    lod = 3u;
                }
            }
        }
        foliage_expand(lod, index);
    }
}

/// The box draw's arguments: nine corners a box, the boxes that fit.
@compute @workgroup_size(1)
fn cs_boxes_finish() {
    box_state.vertex_count = 9u;
    box_state.instance_count = min(atomicLoad(&box_state.count), arrayLength(&boxes_out));
    box_state.first_vertex = 0u;
    box_state.first_instance = 0u;
}

/// A box (`cs_boxes`): its three faces turned to the eye, a strip of nine corners.
@vertex
fn vs_box(@builtin(vertex_index) vertex_index: u32, @builtin(instance_index) instance_index: u32) -> VertexOutput {
    let bx = boxes[instance_index];
    let basis = foliage_basis(tile_frames[bx.slot]);
    let heights = unpack2x16float(bx.heights);
    let shape = unpack2x16float(bx.shape);
    let eye = cameras[0].position_near.xyz - bx.centre;
    let corner = box_vertex(Box(vec2<f32>(0.0), bx.half, heights.x, heights.y), vec2<f32>(dot(eye, basis.tangent), dot(eye, basis.bitangent)), min(vertex_index, 8u));
    let world = bx.centre + foliage_to_world(basis, corner.local);
    let ty = foliage_types[(bx.look >> 16u) & 0xffu];
    var out: VertexOutput;
    out.clip_position = cameras[0].view_proj * vec4<f32>(world + bx.now, 1.0);
    out.prev_clip_position = cameras[0].prev_view_proj * vec4<f32>(world + bx.prev, 1.0);
    out.moved = length(bx.now - bx.prev);
    // A blocky shape lights as the ground it grows on, its faces telling apart.
    out.world_normal = basis.up;
    out.face_normal = foliage_to_world(basis, corner.normal);
    out.blocky = 1u;
    out.height_frac = clamp(corner.local.y / max(shape.x, 1.0e-3), 0.0, 1.0);
    out.tint = unpack4x8unorm(bx.tint).xy;
    out.lod = (bx.look >> 24u) & 3u;
    out.material = vec2<u32>(ty.base_color, ty.roughness_metallic);
    let ground = bx.look & 0xffffu;
    out.ground_colour = select(vec4<f32>(0.0), vec4<f32>(f32(ground & 31u) / 31.0, f32((ground >> 5u) & 63u) / 63.0, f32(ground >> 11u) / 31.0, 1.0), ground != 0u);
    out.tip = vec2<u32>(ty.tip_color, bitcast<u32>(shape.y));
    return out;
}

// ═══════════════════════════════════════════════════════════════════════════════
// Fragment stage
// ═══════════════════════════════════════════════════════════════════════════════

/// Locations 5 and 6 are deliberately absent — see the file header. wgpu's
/// fragment-interface check iterates the shader's outputs and does not demand one per
/// declared target, so the pipeline still validates against all eight attachments while
/// the two unwritten ones carry `ColorWrites::empty()`.
struct FoliageGBufferOutput {
    @location(0) albedo: vec4<f32>,
    @location(1) normal: vec4<f32>,
    @location(2) orm: vec4<f32>,
    @location(3) emissive: vec4<f32>,
    /// `(-1, -1.5)`: foliage's tag. Negative x: no lightmap (the target's clear value
    /// `(0, 0)` would read as a lightmapped surface); y -1.5 (clear of the voxel pass's
    /// -2 and of portal chains, below -2.5): the deferred pass leaves screen-space AO
    /// off it (dense blades overwhelm the kernel; they carry their own height AO).
    @location(4) lightmap_uv: vec2<f32>,
    @location(7) velocity: vec4<f32>,
}

fn foliage_velocity(clip_position: vec2<f32>, prev_clip: vec4<f32>) -> vec2<f32> {
    let prev_ndc = prev_clip.xy / prev_clip.w;
    let prev_pixel = vec2<f32>(
        (prev_ndc.x * 0.5 + 0.5) * globals.screen_size.x,
        (0.5 - prev_ndc.y * 0.5) * globals.screen_size.y,
    );
    return clip_position - prev_pixel;
}

@fragment
fn fs_main(input: VertexOutput, @builtin(front_facing) front_facing: bool) -> FoliageGBufferOutput {
    // ── Colour ────────────────────────────────────────────────────────────────
    // The type's authored colour (a multiplier on the ground's for ground-coloured types,
    // so grass is the turf's colour), else the procedural root-to-tip green; varied per
    // blade, darker at the root and lighter at the tip; the tip colour (flower heads) on
    // the tip.
    var albedo: vec3<f32>;
    var roughness = FOLIAGE_ROUGHNESS;
    var metallic = 0.0;
    if input.material.x != 0u {
        albedo = unpack4x8unorm(input.material.x).rgb;
        let roughness_metallic = unpack2x16unorm(input.material.y);
        roughness = roughness_metallic.x;
        metallic = roughness_metallic.y;
    } else {
        albedo = mix(vec3<f32>(0.055, 0.115, 0.030), vec3<f32>(0.180, 0.320, 0.075), input.height_frac);
    }
    // A ground-coloured type's colour is a multiplier on the turf's: twice it, so 0.5
    // keeps the turf's own colour and brighter is lighter.
    if input.ground_colour.w > 0.0 {
        albedo = input.ground_colour.rgb * albedo * 2.0;
    }
    albedo *= vec3<f32>(mix(0.86, 1.14, input.tint.x), mix(0.92, 1.08, input.tint.y), mix(0.84, 1.16, input.tint.x));
    albedo *= mix(0.55, 1.2, input.height_frac);
    if input.tip.x != 0u && input.height_frac >= bitcast<f32>(input.tip.y) {
        albedo = unpack4x8unorm(input.tip.x).rgb * mix(0.9, 1.1, input.tint.y);
    }
    albedo = clamp(albedo, vec3<f32>(0.0), vec3<f32>(1.0));

    // Blades are two-sided (`cull_mode: None`): the geometric normal flips for back
    // faces. A blocky shape lights as the ground it grows on, its faces telling apart:
    // the up tilted towards each face's own normal (its sides are the two facing the
    // eye, so lit by the faces alone a meadow looked at towards the sun is its unlit
    // sides).
    var normal = normalize(select(-input.world_normal, input.world_normal, front_facing));
    if input.blocky != 0u {
        normal = normalize(input.world_normal + BLOCK_FACE_TILT * input.face_normal);
    }

    // Ambient occlusion from the blade's own height: the base of a sward is deep in its
    // own canopy.
    let ao = mix(0.45, 1.0, input.height_frac);

    var out: FoliageGBufferOutput;
    if (globals.flags & FOLIAGE_FLAG_DEBUG_LOD) != 0u {
        var lod_colour = vec3<f32>(1.0, 0.0, 0.0);
        if input.lod == 1u {
            lod_colour = vec3<f32>(0.0, 1.0, 0.0);
        } else if input.lod == 2u {
            lod_colour = vec3<f32>(0.0, 0.4, 1.0);
        } else if input.lod >= 3u {
            lod_colour = vec3<f32>(1.0, 0.9, 0.0);
        }
        albedo = lod_colour;
    }

    // Metallic workflow, as `gbuffer.wgsl` resolves it: F0 = mix(0.04, albedo, metallic).
    let f0 = mix(vec3<f32>(FOLIAGE_F0), albedo, metallic);
    out.albedo = vec4<f32>(albedo, 1.0);
    out.normal = vec4<f32>(normal, f0.r);
    out.orm = vec4<f32>(ao, roughness, metallic, f0.g);
    out.emissive = vec4<f32>(0.0, 0.0, 0.0, f0.b);
    // w: 1 where the blade moved by itself, in the wind or pushed aside
    // (helio-pass-tsr MOTION_VECTORS); still, it moves with the camera.
    out.velocity = vec4<f32>(foliage_velocity(input.clip_position.xy, input.prev_clip_position), 0.0,
        select(0.0, 1.0, input.moved > 0.0));
    out.lightmap_uv = vec2<f32>(-1.0, -1.5);
    return out;
}
