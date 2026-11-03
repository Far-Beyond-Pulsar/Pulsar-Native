// The terrain-stack interpreter of `helio.terrain`; mirror of src/landform.rs.
const LANDFORM_WARP: u32 = 6u; // leading domain-warp slots (two per axis)

struct LandformOctave { shift: u32, amplitude: i32, seed: u32, kind: u32 } // kind | layer << 8
struct StackLayer { kind: u32, mask: u32, a: i32, b: i32 }
// `landform::PackedRule`: material, speck share, patch shift and threshold;
// height and slope ranges; depth and moisture ranges; erosion range, band
// period and parity; temperature range and river beds (0 any, 1 only, 2
// never).
struct MaterialRule { head: vec4<i32>, height_slope: vec4<i32>, depth_moisture: vec4<i32>, surface_bands: vec4<i32>, climate: vec4<i32> }
struct TerrainConstants {
    header: vec4<i32>, // octave count, layer mm, soil depth (cells), seed
    levels: vec4<i32>, // pad, continents layer + 1, pad, low-basin height (mm)
    shape: vec4<i32>,  // ridge display layer + 1, steep slope (cells), ridge display flag, plane (+Y up)
    style: vec4<i32>,  // material style, Layered surface / soil / rock ids
    stack: vec4<i32>,  // layer count, erosion amplitude sum, domain radius, warp octaves
    materials: vec4<i32>, // rules style: rule count, fallback material; coarsest erosion octave's shift (-1: none)
    // Planet maps (`planet_maps::MAP_HEADER`): lookups and the climate.
    maps: array<vec4<i32>, 5>,
    layers: array<StackLayer, 8>,
    octaves: array<LandformOctave, 64>,
    rules: array<MaterialRule, 16>,
    ridge_suffix: array<vec4<i32>, 66>, // signed conditional suffix means
    // Per-level column programs (`landform::LandformPlan`): each level's
    // first op and count (`first | count << 16`), node words in use, ops.
    plan_lists: array<vec4<u32>, 8>,
    plan_nodes: array<vec4<u32>, 8>,
    plan_ops: array<vec4<u32>, 232>,
}

// Octave kinds (`landform::CONTINENT` ...).
const LF_CONTINENT: u32 = 0u;
const LF_REGION: u32 = 1u;
const LF_RIDGE: u32 = 2u;
const LF_HILLS: u32 = 3u;
const LF_WARP: u32 = 4u;
const LF_EROSION: u32 = 8u;
const LF_CRATER: u32 = 9u;
const LF_BASIN: u32 = 10u;
// Layer kinds (`StackLayer::CONTINENTS` ...).
const LAYER_CONTINENTS: u32 = 2u;
const LAYER_MOUNTAINS: u32 = 3u;
const LAYER_HILLS: u32 = 4u;
const LAYER_CRATERS: u32 = 7u;
const LAYER_BASINS: u32 = 8u;
const LAYER_PLATEAU: u32 = 9u;
const LAYER_CLIFFS: u32 = 10u;
const LAYER_RIVERS: u32 = 11u;
// Surface words of a river's channel and of sediment (`landform::RIVER_BED`,
// `landform::ALLUVIUM`).
const RIVER_BED: u32 = 0x80u;
const ALLUVIUM: u32 = 0x81u;
const ALLUVIUM_DEPTH: i32 = 1000;

fn landform_smooth_max(a: i32, b: i32, k: i32) -> i32 {
    return -landform_smooth_min(-a, -b, k);
}

// `landform::terrace`: terraces of `step` height units, rising over the last
// `riser_q10` / 1024 of each step.
fn landform_terrace(h: i32, step: i32, riser_q10: i32) -> i32 {
    var q = h / step;
    if h % step < 0 { q -= 1; }
    let f = h - q * step;
    let riser = max((step * riser_q10) >> 10u, 1);
    let t = (max(f - (step - riser), 0) << 10u) / riser;
    return q * step + ((min(t, 1024) * step) >> 10u);
}

// Masks are always evaluated; other detail finer than about four level
// cells is omitted (`landform::resolved`).
fn landform_resolved(o: LandformOctave, level: u32) -> bool {
    let kind = o.kind & 0xffu;
    return kind <= LF_REGION || kind == LF_BASIN || o.shift >= level + 5u; // landform::RESOLVED_SHIFT
}

fn landform_layer_count() -> u32 { return min(u32(max(terrain.stack.x, 0)), 8u); }

override RIDGE_DISPLAY_GENERATION: bool = false;

fn ridge_suffix_knot(row: u32, knot: u32) -> i32 {
    let index = row * 33u + knot;
    return terrain.ridge_suffix[index >> 2u][index & 3u];
}

fn ridge_suffix_mean(row: u32, incoming: i32) -> i32 {
    let weight = clamp(incoming, FINE_ONE / 4, FINE_ONE - 1);
    let delta = weight - FINE_ONE / 4;
    // (.75 * 2^24)/32 = 3*2^17 exactly. Last interval reaches weight1.
    let step = 393216;
    let knot = min(u32(delta / step), 31u);
    let remainder = delta - i32(knot) * step;
    let fraction = (remainder << 7u) / 3;
    let lo = ridge_suffix_knot(row, knot);
    let hi = ridge_suffix_knot(row, knot + 1u);
    return lo + mul_fine(hi - lo, fraction);
}

fn ridge_display_support(shift: u32, effective_level: u32) -> i32 {
    if shift >= effective_level + 5u { return FINE_ONE; }
    if shift == effective_level + 4u { return FINE_ONE / 2; }
    return 0;
}

const LANDFORM_GRAD_SHIFT: u32 = 18u; // landform::GRAD_SHIFT
const LANDFORM_STRIPES: i32 = 2;

fn landform_per_span(d: i32, shift: u32) -> i32 {
    if shift >= LANDFORM_GRAD_SHIFT { return d >> (shift - LANDFORM_GRAD_SHIFT); }
    return d << (LANDFORM_GRAD_SHIFT - shift);
}

fn landform_per_span3(d: vec3<i32>, shift: u32) -> vec3<i32> {
    return vec3<i32>(landform_per_span(d.x, shift), landform_per_span(d.y, shift), landform_per_span(d.z, shift));
}

fn mul_fine3(v: vec3<i32>, b: i32) -> vec3<i32> {
    return vec3<i32>(mul_fine(v.x, b), mul_fine(v.y, b), mul_fine(v.z, b));
}

// Q24 weight of a clamp's derivative inside (lo, hi) (`soft_inside`).
fn soft_inside(x: i32, lo: i32, hi: i32) -> i32 {
    return min(clamp((x - lo) * 8, 0, FINE_ONE), clamp((hi - x) * 8, 0, FINE_ONE));
}

// Continents base (`continent_base`).
fn landform_base(c: i32, floor: i32, lowland: i32) -> i32 {
    if c < 0 {
        let t = min(-c, FINE_ONE);
        return mul_fine(floor, t) + mul_fine(lowland / 8, FINE_ONE - t);
    }
    return mul_fine(lowland, min(c * 2, FINE_ONE));
}

// Land weight and deep-sea fade (`land_masks`).
fn landform_land(c: i32) -> vec2<i32> {
    return vec2<i32>(clamp(c * 3, 0, FINE_ONE), clamp(FINE_ONE + c * 2, FINE_ONE / 8, FINE_ONE));
}

fn landform_masked(mask: u32, x: i32, lw: vec2<i32>) -> i32 {
    if mask == 1u { return mul_fine(x, lw.x); }
    if mask == 2u { return mul_fine(x, lw.y); }
    return x;
}

// Per-layer accumulators (`Accumulators`), reset by every height evaluation.
var<private> lf_value: array<i32, 8>;
var<private> lf_region: array<i32, 8>;
var<private> lf_weight: array<i32, 8>;
var<private> lf_dvalue: array<vec3<i32>, 8>;
var<private> lf_dregion: array<vec3<i32>, 8>;
var<private> lf_dweight: array<vec3<i32>, 8>;

// Continents value and gradient, FINE_ONE (all land) without them.
fn landform_continent() -> vec4<i32> {
    let ci = terrain.levels.y - 1;
    if ci < 0 { return vec4<i32>(FINE_ONE, 0, 0, 0); }
    let l = u32(ci) & 7u;
    return vec4<i32>(lf_value[l], lf_dvalue[l]);
}

// Steering gradient of the continents and mountains w.r.t. the warped point
// (`steering`).
fn landform_steering() -> vec3<i32> {
    let cd = landform_continent();
    let c = cd.x;
    let dc = cd.yzw;
    let lw = landform_land(c);
    let dland = mul_fine3(dc * 3, soft_inside(c * 3, 0, FINE_ONE));
    let dwet = mul_fine3(dc * 2, soft_inside(FINE_ONE + c * 2, FINE_ONE / 8, FINE_ONE));
    var g = vec3<i32>(0);
    for (var l = 0u; l < landform_layer_count(); l++) {
        let layer = terrain.layers[l];
        var x = 0;
        var dx = vec3<i32>(0);
        if layer.kind == LAYER_CONTINENTS {
            if c < 0 {
                dx = mul_fine3(mul_fine3(-dc, soft_inside(-c, 0, FINE_ONE)), layer.a - layer.b / 8);
            } else {
                dx = mul_fine3(mul_fine3(dc * 2, soft_inside(c * 2, 0, FINE_ONE)), layer.b);
            }
            x = landform_base(c, layer.a, layer.b);
        } else if layer.kind == LAYER_MOUNTAINS {
            let m = (lf_region[l] - (layer.a << 8u)) * 3;
            let region = clamp(m, 0, FINE_ONE);
            let dregion = mul_fine3(lf_dregion[l] * 3, soft_inside(m, 0, FINE_ONE));
            x = mul_fine(lf_value[l], region);
            dx = mul_fine3(lf_dvalue[l], region) + mul_fine3(dregion, lf_value[l]);
        } else {
            continue;
        }
        if layer.mask == 1u {
            g += mul_fine3(dx, lw.x) + mul_fine3(dland, x);
        } else if layer.mask == 2u {
            g += mul_fine3(dx, lw.y) + mul_fine3(dwet, x);
        } else {
            g += dx;
        }
    }
    return g;
}

fn trilerp_q16(v: array<i32, 8>, w: vec3<i32>) -> i32 {
    let y0 = lerp_q16(lerp_q16(v[0], v[1], w.x), lerp_q16(v[2], v[3], w.x), w.y);
    let y1 = lerp_q16(lerp_q16(v[4], v[5], w.x), lerp_q16(v[6], v[7], w.x), w.y);
    return lerp_q16(y0, y1, w.z);
}

fn landform_lattice_q16(v: i32, mask: i32, s: u32) -> i32 {
    if s >= 16u { return (v & mask) >> (s - 16u); }
    return (v & mask) << (16u - s);
}

// One erosion octave (`erosion_octave`): `.x` height, `.yzw` gradient.
fn landform_erosion(saturation_slope: i32, o: LandformOctave, p: vec3<i32>, g: vec3<i32>, up: vec3<i32>) -> vec4<i32> {
    let t = vec3<i32>(
        mul_shr_signed(up.y, g.z, 30u) - mul_shr_signed(up.z, g.y, 30u),
        mul_shr_signed(up.z, g.x, 30u) - mul_shr_signed(up.x, g.z, 30u),
        mul_shr_signed(up.x, g.y, 30u) - mul_shr_signed(up.y, g.x, 30u));
    let tn = unit_q30(t);
    if all(tn == vec3<i32>(0)) { return vec4<i32>(0); }
    let slope = u32(mul_shr_signed(t.x, tn.x, 30u) + mul_shr_signed(t.y, tn.y, 30u) + mul_shr_signed(t.z, tn.z, 30u));
    let saturation = u32(max(saturation_slope, 1));
    let strength = i32(((min(slope, saturation) >> 5u) << 16u) / max(saturation >> 5u, 1u));
    let s = o.shift;
    let c = p >> vec3<u32>(s);
    let mask = (1 << s) - 1;
    let f = vec3<i32>(landform_lattice_q16(p.x, mask, s), landform_lattice_q16(p.y, mask, s), landform_lattice_q16(p.z, mask, s));
    let w = vec3<i32>(fade(f.x), fade(f.y), fade(f.z));
    let along0 = mul_shr_signed(f.x, tn.x, 30u) + mul_shr_signed(f.y, tn.y, 30u) + mul_shr_signed(f.z, tn.z, 30u);
    let step = tn >> vec3<u32>(14u);
    var cosv: array<i32, 8>;
    var sinv: array<i32, 8>;
    for (var index = 0; index < 8; index++) {
        let k = vec3<i32>(index & 1, (index >> 1u) & 1, index >> 2u);
        let corner = c + k;
        let along = along0 - dot(k, step);
        let phase = along * LANDFORM_STRIPES + i32(hash3(corner.x, corner.y, corner.z, o.seed) & 0xffffu);
        cosv[index] = mul16(abs(sin_turns((phase >> 1u) + 16384)), 102944) - NOISE_ONE;
        sinv[index] = sin_turns(phase);
    }
    let value = scale_q16(mul16(trilerp_q16(cosv, w), strength), o.amplitude);
    let m = scale_q16(mul16(trilerp_q16(sinv, w), strength), o.amplitude);
    let magnitude = landform_per_span(-((m * 3217) >> 8u), s);
    return vec4<i32>(value, mul_shr_signed(magnitude, tn.x, 30u), mul_shr_signed(magnitude, tn.y, 30u), mul_shr_signed(magnitude, tn.z, 30u));
}

const LANDFORM_CRATER_RADIUS: i32 = 157286; // landform::CRATER_RADIUS_Q19
const LANDFORM_REACH: i32 = 67108864; // squared ejecta reach in crater radii (Q24)

// 2^60 / d for d in [2^29, 2^30) (`recip_q30`).
fn landform_recip_q30(d: u32) -> u32 {
    var y = 3031741621u - mul_shr(d, 2021161080u, 30u);
    for (var step = 0u; step < 4u; step++) {
        y = mul_shr(y, 2u * Q30 - mul_shr(d, y, 30u), 30u);
    }
    return y;
}

// r2 / rad2 in Q24 (`squared_ratio`).
fn landform_squared_ratio(r2: u32, rad2: u32) -> u32 {
    let bits = 32u - countLeadingZeros(rad2);
    return mul_shr(r2, landform_recip_q30(rad2 << (30u - bits)), 6u + bits);
}

// Polynomial smooth minimum over a width `k` (`smooth_min`).
fn landform_smooth_min(a: i32, b: i32, k: i32) -> i32 {
    let m = min(a, b);
    let gap = abs(a - b);
    if k <= 0 || gap >= k { return m; }
    let n = (k - gap) << 8u;
    let q = ((n / k) << 8u) + ((n % k) << 8u) / k;
    return m - scale_q16(mul16(q, q), k) / 4;
}

// One crater octave (`crater_octave`): height (mm), freshest ejecta.
fn landform_crater(layer: StackLayer, o: LandformOctave, p: vec3<i32>, up: vec3<i32>) -> vec2<i32> {
    let s = o.shift;
    let density = o.seed & 0xffffu;
    let radius_domain = u32(terrain.stack.z);
    let c0 = p >> vec3<u32>(s);
    var height = 0;
    var ejecta = 0u;
    for (var index = 0; index < 27; index++) {
        let cell = c0 + vec3<i32>(index % 3 - 1, (index / 3) % 3 - 1, index / 9 - 1);
        let a = hash3(cell.x, cell.y, cell.z, o.seed);
        if (a & 0xffffu) >= density { continue; }
        let b = hash3(cell.x, cell.y, cell.z, o.seed ^ 0x6C8E9CF5u);
        let jitter = vec3<i32>(i32(b & 1023u), i32((b >> 10u) & 1023u), i32((b >> 20u) & 1023u));
        let centre = (cell << vec3<u32>(s)) + (jitter << vec3<u32>(s - 10u));
        // Only centres within half a cell of the surface.
        if terrain.shape.w != 0 {
            if abs(centre.y) >= (1 << (s - 1u)) { continue; }
        } else {
            let ac = vec3<u32>(abs(centre));
            let len2 = mul_shr(ac.x, ac.x, 30u) + mul_shr(ac.y, ac.y, 30u) + mul_shr(ac.z, ac.z, 30u);
            let r2 = mul_shr(radius_domain, radius_domain, 30u);
            if u32(abs(i32(len2) - i32(r2))) >= (radius_domain >> (30u - s)) { continue; }
        }
        // Squared horizontal distance in Q28 lattice cells from Q(19 + e)
        // offsets, exact for coarse lattices (`crater_octave`).
        let e = select(0u, s - 19u, s >= 19u);
        var d = centre - p;
        if s < 19u { d = d << vec3<u32>(19u - s); }
        if max(max(abs(d.x), abs(d.y)), abs(d.z)) >= (1 << (19u + e)) { continue; }
        let along = mul_shr_signed(d.x, up.x, 20u + e) + mul_shr_signed(d.y, up.y, 20u + e) + mul_shr_signed(d.z, up.z, 20u + e);
        let ad = vec3<u32>(abs(d));
        let len2 = mul_shr(ad.x, ad.x, 10u + 2u * e) + mul_shr(ad.y, ad.y, 10u + 2u * e) + mul_shr(ad.z, ad.z, 10u + 2u * e);
        let aa = mul_shr(u32(abs(along)), u32(abs(along)), 30u);
        let r2 = select(0u, len2 - aa, len2 > aa);
        let size = 36045 + i32(((a >> 16u) * 29491u) >> 16u);
        let radius = max(mul16(LANDFORM_CRATER_RADIUS, size), 1);
        let rad2 = max(mul_shr(u32(radius), u32(radius), 10u), 1u);
        if r2 >= 4u * rad2 { continue; }
        let x2 = i32(landform_squared_ratio(r2, rad2));
        let depth = scale_q16(size, o.amplitude);
        let rim = scale_q16(layer.a, depth);
        let bowl = mul_fine(x2, depth + rim) - depth;
        let t = (LANDFORM_REACH - x2) / 3;
        let ejecta_height = mul_fine(mul_fine(mul_fine(t, t), t), rim);
        height += landform_smooth_min(bowl, ejecta_height, rim / 4);
        if (hash3(cell.x, cell.y, cell.z, o.seed ^ 0x1B873593u) & 0xffffu) < u32(layer.b) {
            ejecta = max(ejecta, 255u - u32((x2 >> 16u) * 255 / (LANDFORM_REACH >> 16u)));
        }
    }
    return vec2<i32>(height, i32(ejecta));
}

fn landform_noise(p: vec3<i32>, o: LandformOctave, gradient: bool) -> vec4<i32> {
    if gradient { return noise_fine_grad(p, o.shift, o.seed); }
    return vec4<i32>(noise_fine(p, o.shift, o.seed), 0, 0, 0);
}

// One ridge octave (noise `nd`) into layer `l`'s chain (`height_parts`),
// weighted by `gain`; `tracked` carries gradients.
fn landform_ridge(l: u32, o: LandformOctave, nd: vec4<i32>, tracked: bool, gain: i32) {
    let n = nd.x;
    let r = clamp(FINE_ONE - abs(n), 0, FINE_ONE - 1);
    let rr = mul_fine(r, r);
    let v = mul_fine(rr, lf_weight[l]);
    if tracked {
        // The crest's sign flip is softened over |n| < 1/4.
        let dr = mul_fine3(landform_per_span3(-nd.yzw, o.shift), clamp(n * 4, -FINE_ONE, FINE_ONE));
        let dv = mul_fine3(mul_fine3(dr, r) * 2, lf_weight[l]) + mul_fine3(lf_dweight[l], rr);
        lf_dweight[l] = mul_fine3(dv * 2, soft_inside(v * 2, FINE_ONE / 4, FINE_ONE - 1));
        lf_dvalue[l] += mul_fine3(dv, o.amplitude);
    }
    lf_weight[l] = clamp(v * 2, FINE_ONE / 4, FINE_ONE - 1);
    if gain == FINE_ONE {
        lf_value[l] += mul_fine(o.amplitude, v);
    } else {
        lf_value[l] += mul_fine(mul_fine(o.amplitude, v), gain);
    }
}

// ---- Two-rate field (`landform::MACRO_GAP`, docs/two-rate-field.md) ----
// Octaves MACRO_GAP shifts or more above a column's level (erosion and
// craters excepted) are evaluated at nodes (the columns of the level whose
// indices are multiples of 16) and interpolated; a column evaluates the
// finer ones. The engine places and interpolates the nodes
// (`terrain_node`, `generation_node_word`). A node's words
// (`landform::Node`): value[8] 0, region[8] 8, dvalue[8] 16 (x3),
// dregion[8] 40 (x3), the macro ridges' noise and gradient[10] 64 (x4),
// the macro warp 104 (x3), its Jacobian rows 107 (3 x3).
const MACRO_GAP: u32 = 15u;
const MACRO_RIDGES: u32 = 10u;
const TERRAIN_NODE_WORDS: u32 = 116u;
// The node being evaluated (`terrain_node`).
var<private> lf_macro: array<i32, 116>;

fn landform_macro_class(o: LandformOctave, level: u32) -> bool {
    let kind = o.kind & 0xffu;
    return o.shift >= level + MACRO_GAP && kind != LF_EROSION && kind != LF_CRATER;
}

// Per-level column programs (`landform::LandformPlan`, `OP_*`, `NODE_*`).
const PLAN_LEVELS: u32 = 32u;
const OP_INDEX: u32 = 63u;
const OP_MACRO: u32 = 64u;
const OP_DISPLAY: u32 = 128u;
const OP_DISPLAY_ONLY: u32 = 256u;
const OP_REGION_DONE: u32 = 512u;
const OP_RANK_SHIFT: u32 = 10u;
const OP_STEER: u32 = 16384u;
const NODE_REGION_SHIFT: u32 = 8u;
const NODE_DVALUE_SHIFT: u32 = 16u;
const NODE_WARP: u32 = 0x1000000u;
const NODE_ANY: u32 = 0x80000000u;

// `level`'s first op and op count.
fn landform_plan_list(level: u32) -> vec2<u32> {
    let l = min(level, PLAN_LEVELS - 1u);
    let e = terrain.plan_lists[l >> 2u][l & 3u];
    return vec2<u32>(e & 0xffffu, e >> 16u);
}

// The node words `level`'s columns read (0: no nodes).
fn landform_plan_nodes(level: u32) -> u32 {
    let l = min(level, PLAN_LEVELS - 1u);
    return terrain.plan_nodes[l >> 2u][l & 3u];
}

fn landform_plan_op(n: u32) -> u32 {
    return (terrain.plan_ops[n >> 3u][(n >> 1u) & 3u] >> ((n & 1u) * 16u)) & 0xffffu;
}

// Whether any octave of `level` is a macro one (`landform::has_macro`).
fn landform_has_macro(level: u32) -> bool {
    return (landform_plan_nodes(level) & NODE_ANY) != 0u;
}

fn landform_macro_add3(slot: u32, v: vec3<i32>) {
    lf_macro[slot] += v.x;
    lf_macro[slot + 1u] += v.y;
    lf_macro[slot + 2u] += v.z;
}

// A column's interpolated node words `slot..slot + 3`.
fn landform_macro3(slot: u32) -> vec3<i32> {
    return vec3<i32>(generation_node_word(slot), generation_node_word(slot + 1u), generation_node_word(slot + 2u));
}

// The node at domain point `node` of `level` into `lf_macro`
// (`landform::macro_node`): the whole warp places the octaves, its macro
// octaves' part is carried.
fn landform_macro_node(node: vec3<i32>, level: u32, erosion: bool) {
    for (var slot = 0u; slot < TERRAIN_NODE_WORDS; slot++) { lf_macro[slot] = 0; }
    var warp = vec3<i32>(0);
    let warps = min(u32(max(terrain.stack.w, 0)), LANDFORM_WARP);
    for (var index = 0u; index < warps; index++) {
        let o = terrain.octaves[index];
        let nd = landform_noise(node, o, erosion);
        let n = mul_fine(o.amplitude, nd.x);
        let axis = min(o.kind - LF_WARP, 2u);
        warp[axis] += n;
        if o.shift >= level + MACRO_GAP {
            lf_macro[104u + axis] += n;
            if erosion {
                let m = mul_fine3(nd.yzw, o.amplitude);
                var jac = vec3<i32>(0);
                if o.shift <= 24u { jac = m << vec3<u32>(24u - o.shift); } else { jac = m >> vec3<u32>(o.shift - 24u); }
                landform_macro_add3(107u + axis * 3u, jac);
            }
        }
    }
    let q = node + warp;
    var ridges = 0u;
    let count = min(u32(max(terrain.header.x, 0)), 64u);
    for (var index = LANDFORM_WARP; index < count; index++) {
        let o = terrain.octaves[index];
        if !landform_resolved(o, level) || !landform_macro_class(o, level) { continue; }
        let l = (o.kind >> 8u) & 7u;
        let kind = o.kind & 0xffu;
        if kind <= LF_REGION {
            let nd = landform_noise(q, o, erosion);
            let v = mul_fine(nd.x, o.amplitude << 8u);
            let dv = mul_fine3(landform_per_span3(nd.yzw, o.shift), o.amplitude << 8u);
            if kind == LF_CONTINENT {
                lf_macro[l] += v;
                landform_macro_add3(16u + l * 3u, dv);
            } else {
                lf_macro[8u + l] += v;
                landform_macro_add3(40u + l * 3u, dv);
            }
        } else if kind == LF_RIDGE {
            if ridges < MACRO_RIDGES {
                let nd = landform_noise(q, o, erosion);
                let at = 64u + ridges * 4u;
                lf_macro[at] = nd.x;
                lf_macro[at + 1u] = nd.y;
                lf_macro[at + 2u] = nd.z;
                lf_macro[at + 3u] = nd.w;
            }
            ridges += 1u;
        } else if kind == LF_HILLS {
            lf_macro[l] += mul_fine(o.amplitude, noise_fine(q, o.shift, o.seed));
        } else if kind == LF_BASIN {
            lf_macro[l] += noise_fine(node, o.shift, o.seed);
        } else {
            lf_macro[l] += scale_q16(noise(node, o.shift, o.seed), o.amplitude);
        }
    }
}

// Whether columns of `level` read nodes (`landform::has_macro`).
fn terrain_has_nodes(level: u32) -> bool {
    return landform_has_macro(level);
}

// The node at domain point `p` of `level`; its words by `terrain_node_word`.
fn terrain_node(p: vec3<i32>, level: u32) {
    landform_macro_node(p, level, terrain.materials.z >= i32(level) + 5);
}

fn terrain_node_word(slot: u32) -> i32 {
    return lf_macro[slot];
}

// Height and surface word (`height_parts`). The false mode preserves the
// canonical operation order. Only generation compiles the display
// capability; climate, shade and verify_field stay exact.
//
// Compilers inline every call: each octave reaches one fine-noise site and
// one ridge site, and generation evaluates this once per column
// (`terrain_column_nodes`). Three inlined copies took 22 s to compile.
// The level's program (`landform_plan_list`) runs its ops; levels with
// nodes read the words in use (`landform_plan_nodes`) of the column's
// blend (`generation_node_word`).
fn landform_column(p: vec3<i32>, level: u32, display: bool) -> vec2<i32> {
    // Erosion follows the larger terrain's slope (`height_parts`), where its
    // coarsest octave is resolved (`landform_resolved`).
    let erosion = terrain.materials.z >= i32(level) + 5;
    let nodes = landform_plan_nodes(level);
    var warp = vec3<i32>(0);
    var jx = vec3<i32>(0);
    var jy = vec3<i32>(0);
    var jz = vec3<i32>(0);
    if (nodes & NODE_WARP) != 0u {
        warp = landform_macro3(104u);
        if erosion {
            jx = landform_macro3(107u);
            jy = landform_macro3(110u);
            jz = landform_macro3(113u);
        }
    }
    let warps = min(u32(max(terrain.stack.w, 0)), LANDFORM_WARP);
    for (var index = 0u; index < warps; index++) {
        let o = terrain.octaves[index];
        if nodes != 0u && o.shift >= level + MACRO_GAP { continue; }
        let nd = landform_noise(p, o, erosion);
        let n = mul_fine(o.amplitude, nd.x);
        var jac = vec3<i32>(0);
        if erosion {
            let m = mul_fine3(nd.yzw, o.amplitude);
            if o.shift <= 24u { jac = m << vec3<u32>(24u - o.shift); } else { jac = m >> vec3<u32>(o.shift - 24u); }
        }
        let axis = min(o.kind - LF_WARP, 2u);
        if axis == 0u { warp.x += n; jx += jac; } else if axis == 1u { warp.y += n; jy += jac; } else { warp.z += n; jz += jac; }
    }
    let q = p + warp;
    var up = vec3<i32>(0, 1073741824, 0);
    if terrain.shape.w == 0 { up = unit_q30(p); }
    // The macro octaves' sums come first; their ridges join the chains in
    // table order below.
    for (var l = 0u; l < 8u; l++) {
        lf_value[l] = 0;
        lf_region[l] = 0;
        lf_weight[l] = FINE_ONE - 1;
        lf_dvalue[l] = vec3<i32>(0);
        lf_dregion[l] = vec3<i32>(0);
        lf_dweight[l] = vec3<i32>(0);
        if ((nodes >> l) & 1u) != 0u { lf_value[l] = generation_node_word(l); }
        if erosion && ((nodes >> (NODE_DVALUE_SHIFT + l)) & 1u) != 0u { lf_dvalue[l] = landform_macro3(16u + l * 3u); }
        if ((nodes >> (NODE_REGION_SHIFT + l)) & 1u) != 0u {
            lf_region[l] = generation_node_word(8u + l);
            if erosion { lf_dregion[l] = landform_macro3(40u + l * 3u); }
        }
    }
    var eroded = 0;
    var deroded = vec3<i32>(0);
    var ejecta = 0;
    var ridge_gain = FINE_ONE;
    var ridge_row = 0u;
    // Steering reads only continents and mountains (`OP_STEER` ops change
    // them): the erosion octaves between share one.
    var steer = vec3<i32>(0);
    var steered = false;
    let plan = landform_plan_list(level);
    for (var n = 0u; n < plan.y; n++) {
        let op = landform_plan_op(plan.x + n);
        let o = terrain.octaves[op & OP_INDEX];
        let l = (o.kind >> 8u) & 7u;
        let kind = o.kind & 0xffu;
        let shown = RIDGE_DISPLAY_GENERATION && display && (op & OP_DISPLAY) != 0u;
        if (op & OP_DISPLAY_ONLY) != 0u && !shown { continue; }
        if (op & OP_STEER) != 0u { steered = false; }
        // A macro ridge's noise comes from the nodes.
        let from_macro = (op & OP_MACRO) != 0u;
        var macro_nd = vec4<i32>(0);
        if from_macro {
            let at = 64u + ((op >> OP_RANK_SHIFT) & 15u) * 4u;
            macro_nd = vec4<i32>(generation_node_word(at), generation_node_word(at + 1u), generation_node_word(at + 2u), generation_node_word(at + 3u));
        }
        // Outside a mountain range (its mask and the mask's slope zero:
        // m <= 0) its ridges change neither the height nor the steering:
        // the layer's value only ever reaches them times the mask.
        if kind == LF_RIDGE && (op & OP_REGION_DONE) != 0u && terrain.layers[l].kind == LAYER_MOUNTAINS && (lf_region[l] - (terrain.layers[l].a << 8u)) * 3 <= 0 { continue; }
        var gain = FINE_ONE;
        var gradient = erosion;
        if shown {
            // Expand B[k](w)=(1-s)*F[k](w)+s*(A[k]*v+B[k+1](next(w))).
            // Repeated lattice shifts may have several partial octaves.
            var partial = ridge_gain;
            if ridge_gain != 0 {
                let support = ridge_display_support(o.shift, level);
                if support != FINE_ONE {
                    let mean_gain = mul_fine(ridge_gain, FINE_ONE - support);
                    lf_value[l] += mul_fine(ridge_suffix_mean(ridge_row, lf_weight[l]), mean_gain);
                    ridge_gain = mul_fine(ridge_gain, support);
                }
                partial = ridge_gain;
            }
            ridge_row += 1u;
            if partial == 0 { continue; }
            // Resolved erosion octaves precede every partial ridge: only
            // fully supported ridges carry gradients.
            gain = ridge_gain;
            gradient = erosion && ridge_gain == FINE_ONE;
        }
        if kind <= LF_HILLS || kind == LF_BASIN {
            // The octave's one fine-noise evaluation (basins: unwarped), or
            // a macro ridge's from the nodes.
            var nd = macro_nd;
            if !from_macro { nd = landform_noise(select(q, p, kind == LF_BASIN), o, gradient && kind <= LF_RIDGE); }
            if kind == LF_RIDGE {
                landform_ridge(l, o, nd, gradient, gain);
            } else if kind <= LF_REGION {
                let v = mul_fine(nd.x, o.amplitude << 8u);
                let dv = mul_fine3(landform_per_span3(nd.yzw, o.shift), o.amplitude << 8u);
                if kind == LF_CONTINENT { lf_value[l] += v; lf_dvalue[l] += dv; } else { lf_region[l] += v; lf_dregion[l] += dv; }
            } else if kind == LF_HILLS {
                lf_value[l] += mul_fine(o.amplitude, nd.x);
            } else {
                lf_value[l] += nd.x;
            }
        } else if kind == LF_EROSION {
            if !steered {
                let gq = landform_steering();
                // (I + J)^T: column j gathers every warp axis' dependence on p_j.
                steer = gq + vec3<i32>(
                    mul_fine(gq.x, jx.x) + mul_fine(gq.y, jy.x) + mul_fine(gq.z, jz.x),
                    mul_fine(gq.x, jx.y) + mul_fine(gq.y, jy.y) + mul_fine(gq.z, jz.y),
                    mul_fine(gq.x, jx.z) + mul_fine(gq.y, jy.z) + mul_fine(gq.z, jz.z));
                steered = true;
            }
            let e = landform_erosion(terrain.layers[l].a, o, p, steer + deroded, up);
            lf_value[l] += e.x;
            eroded += e.x;
            deroded += e.yzw;
        } else if kind == LF_CRATER {
            let c = landform_crater(terrain.layers[l], o, p, up);
            lf_value[l] += c.x;
            ejecta = max(ejecta, c.y);
        } else {
            lf_value[l] += scale_q16(noise(p, o.shift, o.seed), o.amplitude);
        }
    }
    // Compose the layers in stack order.
    let c = landform_continent().x;
    let lw = landform_land(c);
    var h = 0;
    var basin = 0;
    var river_bed = false;
    var alluvium = false;
    for (var l = 0u; l < landform_layer_count(); l++) {
        let layer = terrain.layers[l];
        var x = 0;
        if layer.kind == LAYER_RIVERS {
            // The worn terrain (`terrain_erosion`), then valleys and channels
            // along the river segments (`map_carve`).
            if (terrain.maps[0].x & MAP_FLAG_EROSION) != 0 {
                let e = terrain_erosion(p);
                let worn = h + e.x;
                h = landform_smooth_max(worn, e.y, terrain.maps[4].z);
                alluvium = e.y > worn + ALLUVIUM_DEPTH;
            }
            let carved = map_carve(p, level, h);
            h = carved.x;
            river_bed = river_bed || carved.y != 0;
            // The streams below the maps' texels (`map_streams`).
            let streams = map_streams(p, level, h);
            h = streams.x;
            river_bed = river_bed || streams.y != 0;
            continue;
        }
        if layer.kind == LAYER_CONTINENTS {
            x = landform_base(c, layer.a, layer.b);
        } else if layer.kind == LAYER_MOUNTAINS {
            x = mul_fine(lf_value[l], clamp((lf_region[l] - (layer.a << 8u)) * 3, 0, FINE_ONE));
        } else if layer.kind == LAYER_BASINS {
            // Basins flatten what lies below them in the stack by half and
            // sink it by their depth.
            let m = landform_masked(layer.mask, clamp((lf_value[l] - layer.b) * 4, 0, FINE_ONE), lw);
            basin = max(basin, m);
            h = mul_fine(h, FINE_ONE - m / 2) - mul_fine(layer.a, m);
            continue;
        } else if layer.kind == LAYER_PLATEAU {
            x = layer.a;
        } else if (layer.kind & 0xffffu) == LAYER_CLIFFS {
            // Escarpments: inside their regions, terraces of what lies below
            // them in the stack.
            let m = landform_masked(layer.mask, clamp((lf_value[l] - layer.b) * 4, 0, FINE_ONE), lw);
            h += mul_fine(landform_terrace(h, layer.a, i32(layer.kind >> 16u)) - h, m);
            continue;
        } else if layer.kind >= LAYER_HILLS && layer.kind <= LAYER_CRATERS {
            x = lf_value[l];
        }
        h += landform_masked(layer.mask, x, lw);
    }
    var surface = 0;
    if (terrain.style.x == 0 || terrain.style.x == 3) && river_bed {
        surface = i32(RIVER_BED);
    } else if (terrain.style.x == 0 || terrain.style.x == 3) && alluvium {
        surface = i32(ALLUVIUM);
    } else if terrain.style.x == 0 || terrain.style.x == 3 {
        surface = clamp(eroded * 126 / max(terrain.stack.y, 1), -126, 126) & 0xff;
    } else if terrain.style.x == 1 {
        surface = (ejecta >> 1u) | (select(0, 1, basin >= FINE_ONE / 2) << 7u);
    }
    return vec2<i32>(h, surface);
}

// Height and surface word of the column at `p` (`landform::column_parts`;
// `display`: its coarse ridges keep their envelope).
fn terrain_column_nodes(p: vec3<i32>, level: u32, display: bool) -> vec2<i32> {
    return landform_column(p, level, display && terrain.shape.z != 0);
}

// Snowline (mm) where the sea-level temperature is `t0` (`landform::snowline`).
fn landform_snowline(t0: i32) -> i32 {
    let snow = terrain.maps[3].x;
    let per = terrain.maps[3].y;
    if per == 0 { return select(-1073741824, 1073741823, t0 > snow); }
    return clamp(t0 - snow, -8192, 8192) * per;
}

// The Earthlike snowline's spread by moisture and its rock band (`landform::SNOW_*`).
const SNOW_WET: i32 = 750000;
const SNOW_BAND: i32 = 500000;

// Strata altitude (mm): layers undulate +-8 m over ~100 m, so cuts through
// them never show flat rings (`strata` in landform.rs).
fn landform_strata(p: vec3<i32>, altitude: i32) -> i32 {
    return altitude + scale_q16(noise(p, 13u, bitcast<u32>(terrain.header.w) ^ 0x9B05688Cu), 8000);
}

// Material noise is defined in the fixed 1.25 cm domain unit.
// Four samples per lattice spacing retain contrast; below two samples the
// unresolved octave contributes its coverage instead of aliased class noise.
fn material_noise_support(shift: u32, pixel: f32) -> f32 {
    if pixel <= 0.0 { return 1.0; }
    let wavelength = 0.0125 * f32(1u << shift);
    return 1.0 - smoothstep(wavelength * 0.25, wavelength * 0.5, pixel);
}

// Empirical CDF of the project's integer noise on cube-face/plane slices:
// 1.2M samples, four seeds, three axes. The normalized two-octave sum agrees
// within 0.0035 coverage at these knots; this is an appearance approximation.
// Same measured noise distribution as snow, plus its piecewise-linear
// integral. This permits a box convolution without new noise samples.
const MATERIAL_NOISE_CDF = array<f32, 33>(0.00000000, 0.00000250, 0.00002000, 0.00006917, 0.00032333, 0.00097500, 0.00300250, 0.00770250, 0.01944750, 0.04330750, 0.07070667, 0.10610750, 0.16352167, 0.23430250, 0.31787500, 0.40675833, 0.50058917, 0.59367667, 0.68297667, 0.76579917, 0.83619500, 0.89390500, 0.92928833, 0.95633250, 0.98046750, 0.99220417, 0.99696000, 0.99898750, 0.99968417, 0.99994000, 0.99998250, 0.99999750, 1.00000000);
const MATERIAL_NOISE_AREA = array<f32, 33>(0.0000000000, 0.0000012500, 0.0000125000, 0.0000570850, 0.0002533350, 0.0009025000, 0.0028912500, 0.0082437500, 0.0218187500, 0.0531962500, 0.1102033350, 0.1986104200, 0.3334250050, 0.5323370900, 0.8084258400, 1.1707425050, 1.6244162550, 2.1715491750, 2.8098758450, 3.5342637650, 4.3352608500, 5.2003108500, 6.1119075150, 7.0547179300, 8.0231179300, 9.0094537650, 10.0040358500, 11.0020096000, 12.0013454350, 13.0011575200, 14.0011187700, 15.0011087700, 16.0011075200);

fn material_noise_integral(value: f32) -> f32 {
    let x = clamp(value / 4096.0 + 16.0, 0.0, 32.0);
    let i = min(u32(x), 31u);
    let t = x - f32(i);
    return 4096.0 * (MATERIAL_NOISE_AREA[i] + MATERIAL_NOISE_CDF[i] * t
        + 0.5 * (MATERIAL_NOISE_CDF[i + 1u] - MATERIAL_NOISE_CDF[i]) * t * t)
        + max(value - 65536.0, 0.0);
}

// Integral of the unresolved phase CDF, conditioned on natural rock exposure.
// The CDF is an appearance approximation; canonical integer IDs never use it.
fn rock_phase_integral(value: f32, noise_scale: f32, cutoff: f32, cut_cdf: f32, cutoff_integral: f32) -> f32 {
    let x = max(value / noise_scale, cutoff);
    return noise_scale * max(material_noise_integral(x) - cutoff_integral
        - cut_cdf * (x - cutoff), 0.0) / (1.0 - cut_cdf);
}

// Stable integration in the rare upper CDF tail. Its tiny probability cannot
// be recovered by subtracting the two large integrated CDF values in f32.
fn weathered_tail_interval(index: u32, cut: f32, lo: f32, hi: f32) -> vec2<f32> {
    let left = max(-65536.0 + f32(index) * 4096.0, cut);
    let right = -65536.0 + f32(index + 1u) * 4096.0;
    if left >= right { return vec2<f32>(0.0); }
    let density = MATERIAL_NOISE_CDF[index + 1u] - MATERIAL_NOISE_CDF[index];
    let start = max(left, lo);
    let end = min(right, hi);
    let linear = max(end - start, 0.0) * 0.5
        * (clamp((start - lo) / (hi - lo), 0.0, 1.0) + clamp((end - lo) / (hi - lo), 0.0, 1.0));
    let saturated = max(right - max(left, hi), 0.0);
    return density * vec2<f32>(linear + saturated, right - left);
}
// A restrained world-space coating on exposed natural rock, independent of
// geological IDs. Reuse the outcrop samples; unresolved noise contributes the
// conditional mean of a linear weathering ramp instead of altitude contours.
fn weathered_stone_coverage(resolved: f32, deviation: f32, cutoff: f32) -> f32 {
    let lower = -16384.0;
    let upper = 16384.0;
    var mean = clamp((resolved - lower) / (upper - lower), 0.0, 1.0);
    if deviation > 0.0 {
        let lo = (lower - resolved) / deviation;
        let hi = (upper - resolved) / deviation;
        if hi <= max(cutoff, -65536.0) { return 0.65; }
        if lo >= 65536.0 { return 0.35; }
        let cut = clamp(cutoff, -65536.0, 65536.0);
        if cut >= 49152.0 && cut < 65536.0 {
            let tail = weathered_tail_interval(28u, cut, lo, hi)
                + weathered_tail_interval(29u, cut, lo, hi)
                + weathered_tail_interval(30u, cut, lo, hi)
                + weathered_tail_interval(31u, cut, lo, hi);
            return 0.35 + 0.30 * clamp(tail.x / tail.y, 0.0, 1.0);
        }
        let cut_cdf = material_noise_cdf(cut);
        if cut_cdf < 1.0 {
            let cut_integral = material_noise_integral(cut);
            let area = rock_phase_integral(upper - resolved, deviation, cut, cut_cdf, cut_integral)
                - rock_phase_integral(lower - resolved, deviation, cut, cut_cdf, cut_integral);
            mean = clamp(1.0 - area / (upper - lower), 0.0, 1.0);
        }
    }
    return 0.35 + 0.30 * mean;
}

fn rock_box_cdf(value: f32, left: f32, width: f32,
    noise_scale: f32, cutoff: f32, cut_cdf: f32, cutoff_integral: f32) -> f32 {
    if noise_scale <= 0.0 { return clamp((value - left) / width, 0.0, 1.0); }
    let width_q16 = width / noise_scale;
    let hi = (value - left) / noise_scale;
    let lo = hi - width_q16;
    // Saturated endpoints need no CDF primitive, including wide boxes.
    if hi <= max(cutoff, -65536.0) { return 0.0; }
    if lo >= 65536.0 { return 1.0; }
    if width_q16 <= 4096.0 {
        // A narrow box meets at most one CDF knot. Integrate its local
        // trapezoids instead of cancelling two large cumulative areas.
        let a = max(lo, cutoff);
        let split = min((floor(a / 4096.0) + 1.0) * 4096.0, hi);
        let fa = max(material_noise_cdf(a) - cut_cdf, 0.0) / (1.0 - cut_cdf);
        let fs = max(material_noise_cdf(split) - cut_cdf, 0.0) / (1.0 - cut_cdf);
        let fh = max(material_noise_cdf(hi) - cut_cdf, 0.0) / (1.0 - cut_cdf);
        return clamp(0.5 * ((fa + fs) * (split - a) + (fs + fh) * (hi - split)) / width_q16, 0.0, 1.0);
    }
    return clamp((rock_phase_integral(value - left, noise_scale, cutoff, cut_cdf, cutoff_integral)
        - rock_phase_integral(value - left - width, noise_scale, cutoff, cut_cdf, cutoff_integral)) / width, 0.0, 1.0);
}

fn rock_band_coverage(phase: f32, span: f32, deviation: f32, cutoff: f32) -> f32 {
    let noise_scale = deviation * (3000.0 / 65536.0);
    if noise_scale <= 0.0 {
        if span <= 0.0 { return select(0.0, 1.0, fract(phase / 9000.0) < 0.5); }
        if span < 1.0 {
            // Centre on the nearest band boundary before adding a tiny span.
            // Avoid cancelling a sub-mm pixel against a 4.5 m global phase.
            let p = phase - floor(phase / 9000.0) * 9000.0;
            if p < 2250.0 || p >= 6750.0 {
                let edge = select(p, p - 9000.0, p >= 6750.0);
                return clamp(0.5 + edge / span, 0.0, 1.0);
            }
            return clamp(0.5 - (p - 4500.0) / span, 0.0, 1.0);
        }
        let lo = phase - span * 0.5;
        let hi = phase + span * 0.5;
        let lo_period = floor(lo / 9000.0);
        let hi_period = floor(hi / 9000.0);
        let measure = 4500.0 * (hi_period - lo_period)
            + min(hi - hi_period * 9000.0, 4500.0)
            - min(lo - lo_period * 9000.0, 4500.0);
        return clamp(measure / span, 0.0, 1.0);
    }
    let cut_cdf = material_noise_cdf(cutoff);
    if cut_cdf > 0.999 { return -1.0; }
    // The empirical noise has bounded support. Conditioning only truncates
    // its lower end; a support box inside one band has exact 0/1 coverage.
    let noise_low = max(cutoff, -65536.0);
    let radial_width = max(span, 0.0);
    if radial_width + noise_scale * (65536.0 - noise_low) < 4500.0 {
        let lower = phase + noise_scale * noise_low - radial_width * 0.5;
        let upper = phase + noise_scale * 65536.0 + radial_width * 0.5;
        let lower_band = floor(lower / 4500.0);
        if lower_band == floor(upper / 4500.0) {
            return select(0.0, 1.0, (i32(lower_band) & 1) == 0);
        }
    }
    if span <= 0.0 || (span < min(1.0, noise_scale * 65.536) && noise_scale > 0.0) {
        let first = i32(floor((phase - 4000.0) / 9000.0));
        var coverage = 0.0;
        for (var b = first; b < first + 3; b++) {
            let lo = max((f32(b) * 9000.0 - phase) / noise_scale, cutoff);
            let hi = max((f32(b) * 9000.0 + 4500.0 - phase) / noise_scale, cutoff);
            coverage += (material_noise_cdf(hi) - material_noise_cdf(lo)) / (1.0 - cut_cdf);
        }
        return clamp(coverage, 0.0, 1.0);
    }
    // Whole 9 m periods have exactly half light stone, for any noise phase.
    // Only a remainder below 9 m needs integration: bounded even at orbit.
    let whole = floor(span / 9000.0) * 9000.0;
    let remainder = max(span - whole, 0.0);
    if remainder < 0.01 && whole > 0.0 { return 0.5; }
    // A partial period differs from its half-coverage mean by at most
    // min(r, period-r)/2. Positive mixtures of noise phases, including
    // conditional ones, keep that bound. Skip integration below 1/1024
    // absolute coverage error; the near/resolved path stays exact.
    if min(remainder, 9000.0 - remainder) <= span * (1.0 / 512.0) { return 0.5; }
    let start = phase - span * 0.5;
    let left = start - floor(start / 9000.0) * 9000.0;
    let first = i32(floor((left - 4000.0) / 9000.0));
    var cutoff_integral = 0.0;
    if remainder / noise_scale > 4096.0 {
        // Identical for all eight endpoints; narrow boxes never use it.
        cutoff_integral = material_noise_integral(cutoff);
    }
    var coverage = 0.0;
    for (var b = first; b < first + 4; b++) {
        let lo = f32(b) * 9000.0;
        coverage += rock_box_cdf(lo + 4500.0, left, remainder, noise_scale, cutoff, cut_cdf, cutoff_integral)
            - rock_box_cdf(lo, left, remainder, noise_scale, cutoff, cut_cdf, cutoff_integral);
    }
    return clamp((whole * 0.5 + remainder * coverage) / span, 0.0, 1.0);
}

fn material_noise_cdf(value: f32) -> f32 {
    let cdf = MATERIAL_NOISE_CDF;
    let x = clamp(value / 65536.0 * 16.0 + 16.0, 0.0, 32.0);
    let i = min(u32(x), 31u);
    return mix(cdf[i], cdf[i + 1u], x - f32(i));
}

fn snow_material_coverage(resolved: f32, deviation: f32, threshold: f32,
    altitude: i32, dry: bool, depth: i32) -> vec4<f32> {
    let snow = material_noise_cdf((threshold - resolved) / deviation);
    let rock = 1.0 - snow;
    if dry { return vec4<f32>(snow, rock, 0.0, 0.0); }
    if material_weathered_skin {
        let stone = rock * weathered_stone_coverage(resolved, deviation, (threshold - resolved) / deviation);
        let dirt = select(0.0, rock * 0.125, depth < 1);
        let remaining = select(1.0, 0.875, depth < 1);
        return vec4<f32>(snow, stone * remaining, (rock - stone) * remaining, dirt);
    }
    // Integrate the existing alternating 4.5m stone bands too: replacing
    // unresolved snow with a single rock colour would bias their mean.
    // Radial support filters exposed rock conditioned on this snow threshold;
    // snow probability itself keeps the existing outcrop-noise coverage.
    let band_weight = smoothstep(1.125, 2.25, material_radial_span);
    var filtered_stone = -1.0;
    if band_weight > 0.0 {
        let phase = f32(rem_floor(altitude, 9000)) + resolved * (3000.0 / 65536.0);
        let coverage = rock_band_coverage(phase, material_radial_span * 1000.0,
            deviation, (threshold - resolved) / deviation);
        if coverage >= 0.0 { filtered_stone = rock * coverage; }
    }
    var stone = 0.0;
    if band_weight < 1.0 || filtered_stone < 0.0 {
        let mean_altitude = f32(altitude) + resolved * (3000.0 / 65536.0);
        let first = i32(floor((mean_altitude - 4000.0) / 4500.0));
        for (var b = first; b < first + 4; b++) {
            if (b & 1) != 0 { continue; }
            let lo = max(threshold, (f32(b) * 4500.0 - f32(altitude)) * (65536.0 / 3000.0));
            let hi = (f32(b + 1) * 4500.0 - f32(altitude)) * (65536.0 / 3000.0);
            if hi > lo {
                stone += material_noise_cdf((hi - resolved) / deviation)
                    - material_noise_cdf((lo - resolved) / deviation);
            }
        }
    }
    stone = clamp(stone, 0.0, rock);
    if filtered_stone >= 0.0 { stone = mix(stone, filtered_stone, band_weight); }
    let dirt = select(0.0, rock * 0.125, depth < 1);
    let remaining = select(1.0, 0.875, depth < 1);
    return vec4<f32>(snow, stone * remaining, (rock - stone) * remaining, dirt);
}

// Material of a solid ground cell by material style (`ground_material`).
fn ground_material(p: vec3<i32>, surface: u32, top_height: i32, depth: i32, slope: i32, layer: i32) -> u32 {
    return column_ground_material(ground_rules(surface, top_height, slope), p, surface, top_height, depth, slope, layer);
}

// What `ground_material` reads of a column alone (its surface word, top
// height and slope), for its cells to share: the rules style's rules whose
// height, slope and erosion tests pass (bit by rule; 0 in other styles).
fn ground_rules(surface: u32, top_height: i32, slope: i32) -> u32 {
    if terrain.style.x != 3 { return 0u; }
    // A river bed counts as the deepest gully.
    let river = surface == RIVER_BED;
    // Sediment is no gully.
    let erosion = select(max(i32(surface << 24u) >> 24u, -126), 0, surface == ALLUVIUM);
    let count = min(u32(max(terrain.materials.x, 0)), 16u);
    var rules = 0u;
    for (var index = 0u; index < count; index++) {
        let height_slope = terrain.rules[index].height_slope;
        let bands = terrain.rules[index].surface_bands.xy;
        if top_height < height_slope.x || top_height > height_slope.y || slope < height_slope.z || slope > height_slope.w || erosion < bands.x || erosion > bands.y { continue; }
        let mode = terrain.rules[index].climate.z;
        if (mode == 1 && !river) || (mode == 2 && river) { continue; }
        rules |= 1u << index;
    }
    return rules;
}

// `ground_material` of a cell of a column whose `ground_rules` are `rules`.
fn column_ground_material(rules: u32, p: vec3<i32>, surface: u32, top_height: i32, depth: i32, slope: i32, layer: i32) -> u32 {
    material_mix = vec4<f32>(-1.0, 0.0, 0.0, 0.0);
    material_fleck_base = M_AIR;
    material_coverage = -1.0;
    material_coverage_ids = vec2<u32>(M_DARK_STONE, M_STONE);
    if terrain.style.x == 1 { return lunar_material(p, surface, depth, slope, layer); }
    if terrain.style.x == 3 { return rules_material(rules, p, depth, layer); }
    if terrain.style.x == 2 {
        if depth == 0 { return u32(terrain.style.y); }
        if depth <= terrain.header.z { return u32(terrain.style.z); }
        return u32(terrain.style.w);
    }
    return earthlike_material(p, surface, top_height, depth, slope, layer);
}

// The first rule whose conditions hold, else the fallback (`rules_material`):
// of the column's `rules` (`ground_rules`: its height, slope and erosion
// tests passed), those whose cell tests pass.
fn rules_material(rules: u32, p: vec3<i32>, depth: i32, layer: i32) -> u32 {
    let altitude = layer * terrain.header.y;
    let seed = bitcast<u32>(terrain.header.w);
    // The climate once a rule reaches it (most fail on depth first); each
    // rule's words as its tests read them.
    var climate = vec2<i32>(0);
    var climate_known = false;
    var left = rules;
    while left != 0u {
        let index = firstTrailingBit(left);
        left &= left - 1u;
        let depth_moisture = terrain.rules[index].depth_moisture;
        if depth < depth_moisture.x || depth > depth_moisture.y { continue; }
        if !climate_known {
            climate = terrain_climate(p, altitude);
            climate_known = true;
        }
        if climate.y < depth_moisture.z || climate.y > depth_moisture.w { continue; }
        let temperature = terrain.rules[index].climate.xy;
        if climate.x < temperature.x || climate.x > temperature.y { continue; }
        let bands = terrain.rules[index].surface_bands;
        let salt = index * 0x9E3779B9u;
        if bands.z > 0 && (div_floor(landform_strata(p, altitude), bands.z) & 1) != bands.w { continue; }
        let head = terrain.rules[index].head;
        if head.z > 0 && noise(p, u32(head.z), seed ^ 0x68E31DA4u ^ salt) <= head.w { continue; }
        if head.y > 0 {
            if i32(hash3(p.x, p.y, p.z ^ (layer * 0x9e37), 0x5f356495u ^ salt) & 255u) >= head.y { continue; }
            return u32(head.x) | select(0u, M_SPECK, depth == 0);
        }
        return u32(head.x);
    }
    return u32(terrain.materials.y);
}

// Lunar materials (`lunar_material`).
fn lunar_material(p: vec3<i32>, surface: u32, depth: i32, slope: i32, layer: i32) -> u32 {
    let ejecta = (surface & 0x7fu) << 1u;
    let mare = (surface & 0x80u) != 0u;
    let h = hash3(p.x, p.y, p.z ^ (layer * 0x9e37), 0x2545F491u);
    if depth == 0 {
        if slope >= 12 || (slope >= 6 && (h & 3u) == 0u) { return M_BOULDER; }
        if ejecta > (h & 0xffu) { return M_EJECTA; }
        return select(M_REGOLITH, M_MARE, mare);
    }
    if depth < terrain.header.z { return select(M_REGOLITH, M_MARE, mare); }
    return select(M_ANORTHOSITE, M_BASALT, mare);
}

// Earthlike materials (`earthlike_material`).
fn earthlike_material(p: vec3<i32>, surface: u32, top_height: i32, depth: i32, slope: i32, layer: i32) -> u32 {
    let dirt = terrain.header.z;
    let steep = slope >= terrain.shape.y;
    let climate = terrain_climate(p, 0);
    let wet = climate.y << 8u;
    // Hash every domain axis: on a face one of them is nearly constant.
    let h = hash3(p.x, p.y, p.z ^ (layer * 0x9e37), 0x2545F491u);
    let altitude = layer * terrain.header.y;
    if surface == RIVER_BED && depth < dirt * 2 {
        // River beds: gravel and sand over silt (`earthlike_material`).
        if depth == 0 && (h & 3u) == 0u { return M_SAND; }
        return select(M_CLAY, M_GRAVEL, depth < dirt);
    }
    if top_height < terrain.levels.w {
        // Low basins: meadow with mud patches of a few metres (position
        // noise, never aligned with height contours) over silt, gravel and
        // stone (`earthlike_material`).
        if depth == 0 {
            let mud = noise(p, 8u, bitcast<u32>(terrain.header.w) ^ 0x5f356495u);
            return select(M_GRASS, M_DIRT, mud > NOISE_ONE / 2);
        }
        if depth < dirt * 2 {
            return select(M_CLAY, M_GRAVEL, (h & 3u) == 0u);
        }
        return M_STONE;
    }
    // A wandering snowline (the climate's) over a narrow rock band
    // (`earthlike_material`).
    let wander = scale_q16(noise(p, 17u, bitcast<u32>(terrain.header.w) ^ 0x3C6EF372u), SNOW_BAND);
    let snowline = landform_snowline(climate.x) + mul_fine(SNOW_WET, wet - FINE_ONE / 2) + wander;
    // Alpine weight: 0 below the rockline, NOISE_ONE at the snowline.
    let rockline = snowline - SNOW_BAND;
    let band = SNOW_BAND;
    let alpine = (clamp(top_height - rockline, 0, band) / 256) * NOISE_ONE / (band / 256);
    // Rock patches (~50 m and ~6 m octaves), also breaking up snow edges.
    // Noise is clamped to +-NOISE_ONE, so below the rockline on gentler slopes no
    // outcrop can reach the rock fringe: skip it (same result).
    let seed = bitcast<u32>(terrain.header.w);
    var outcrop = -NOISE_ONE;
    var resolved_outcrop = f32(outcrop);
    var outcrop_deviation = 0.0;
    var outcrop_support = 1.0;
    if alpine > 0 || slope >= 5 {
        // Preserve the original integer samples and canonical classification.
        let broad = noise(p, 12u, seed ^ 0x1B56C4E9u);
        let fine = noise(p, 9u, seed ^ 0x6A09E667u) / 3;
        outcrop = broad + fine;
        resolved_outcrop = f32(outcrop);
        if material_footprint > 1.6 {
            let broad_support = material_noise_support(12u, material_footprint);
            let fine_support = material_noise_support(9u, material_footprint);
            outcrop_support = min(broad_support, fine_support);
            if outcrop_support < 1.0 {
                resolved_outcrop = f32(broad) * broad_support + f32(fine) * fine_support;
                outcrop_deviation = sqrt((1.0 - broad_support * broad_support)
                    + (1.0 - fine_support * fine_support) / 9.0);
            }
            if outcrop_deviation > 0.0 && top_height > snowline && depth < dirt {
                let resolved = resolved_outcrop;
                let deviation = outcrop_deviation;
                // Match truncating integer division at negative slopes too.
                let q = 6 - slope;
                let threshold = select(f32((q - 1) * 8192) + 0.5, f32(q * 8192) - 0.5, q > 0);
                let dry = wet < FINE_ONE / 10 * 3 && !steep;
                material_mix_ids = vec4<u32>(M_SNOW, select(M_STONE, M_SAND, dry), M_DARK_STONE, M_DIRT);
                material_mix = snow_material_coverage(resolved, deviation, threshold, altitude, dry, depth);
            }
        }
    }
    // Snow does not hold on faces steeper than ~37 degrees: rock streaks the snowfields.
    if top_height > snowline && depth < dirt && slope + outcrop / 8192 < 6 { return M_SNOW; }
    if surface == ALLUVIUM && !steep && depth < dirt * 3 {
        // Sediment: dry fans of sand and gravel, wet plains of meadow over
        // soil and silt (`earthlike_material`).
        if wet < FINE_ONE / 10 * 3 { return select(M_GRAVEL, M_SAND, depth == 0 && (h & 3u) != 0u); }
        if depth == 0 { return M_GRASS; }
        return select(M_CLAY, M_DIRT, depth < dirt);
    }
    if wet < FINE_ONE / 10 * 3 {
        if depth < dirt && !steep { return M_SAND; }
        let band = rem_floor(div_floor(landform_strata(p, altitude), 2100), 5);
        return select(M_SANDSTONE, M_CLAY, band == 1 || band == 3);
    }
    // Rock shows through the turf in the outcrop patches, which grow up the
    // alpine band below the snowline and on hillsides over ~32 degrees; scree and
    // bare soil fringe them. Deeper cells keep the strata.
    var exposed = outcrop + 2 * alpine - NOISE_ONE;
    if slope >= 5 { exposed += NOISE_ONE / 2; }
    let gully = i32(surface << 24u) >> 24u;
    if slope >= 2 { exposed += gully * (NOISE_ONE / 256); }
    if steep || (exposed > 0 && depth < dirt) {
        let rock = select(M_DARK_STONE, M_STONE, (div_floor(altitude + scale_q16(outcrop, 3000), 4500) & 1) == 0);
        if depth < 1 { material_fleck_base = rock; }
        // Four samples per 4.5 m band retain its resolved contrast, matching
        // the noise support gate. No coverage work on fully resolved rock.
        let band_weight = max(smoothstep(1.125, 2.25, material_radial_span), 1.0 - outcrop_support);
        // Snow/rock coverage already includes its conditioned stone bands;
        // the shader consumes that vector instead of this separate metadata.
        // Natural rock surfaces take their colour variation from the stone's
        // patches (shade); band coverage filters cuts and buried strata.
        if material_weathered_skin && depth < dirt {
        } else if material_footprint > 0.0 && band_weight > 0.0 && material_mix.x < 0.0 {
            // Exposure conditions the phase distribution on non-steep patches.
            let exposure = f32(NOISE_ONE - 2 * alpine - select(0, NOISE_ONE / 2, slope >= 5)) + 0.5;
            let cutoff = select(-65536.0, (exposure - resolved_outcrop) / max(outcrop_deviation, 0.0001), !steep);
            let phase = f32(rem_floor(altitude, 9000)) + resolved_outcrop * (3000.0 / 65536.0);
            let coverage = rock_band_coverage(phase, material_radial_span * 1000.0, outcrop_deviation, cutoff);
            if coverage >= 0.0 {
                material_coverage = mix(select(0.0, 1.0, rock == M_STONE), coverage, band_weight);
            }
        }
        if depth < 1 && (h & 7u) == 0u { return M_DIRT; }
        return rock;
    }
    if depth == 0 {
        if exposed > -NOISE_ONE / 16 || (gully < -64 && slope >= 3) { return M_GRAVEL; }
        if exposed > -NOISE_ONE / 8 { return M_DIRT; }
        return M_GRASS;
    }
    if depth < dirt { return M_DIRT; }
    if depth < dirt * 3 && (h & 3u) == 0u { return M_GRAVEL; }
    return select(M_DARK_STONE, M_STONE, (div_floor(landform_strata(p, altitude), 12000) & 1) == 0);
}
