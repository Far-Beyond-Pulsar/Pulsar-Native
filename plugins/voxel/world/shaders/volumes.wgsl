// The terrain's volumetric shapes (docs/volumes.md): cave networks and
// cliff shapes; mirror of src/volumes.rs, bit for bit. Coordinates are a
// face's cells in quarters (Q2): horizontal base cells, vertical base
// layers. Generation keeps horizontal ones relative to a footprint's first
// base cell (`origin`), so every value fits 32 bits on any face.

struct VolumeConstants {
    head: vec4<i32>,        // flags (1 caves, 2 cliffs), region shift, reference level, seed
    shares: vec4<i32>,      // share, hub chambers, chambers, links (Q16)
    depths: vec4<i32>,      // entrances (Q16), depth low and high, entrance lift (Q2)
    radii: vec4<i32>,       // tunnel low and high, chamber low and high (Q2)
    shapes: vec4<i32>,      // tunnel and chamber (height | floor << 16, Q8), roughness (Q8), its shift
    bounds: vec4<i32>,      // cover, max depth (Q2), layer (mm), bend (Q2)
    cliff: vec4<i32>,       // amplitude (mm), slope low and high (eighths), region shift
    cliff_shape: vec4<i32>, // region threshold (Q16), noise shift, strata period (mm), wander shift
    cliff_mix: vec4<i32>,   // noise and strata weights (Q8), face cells, 0
}

@group(0) @binding(18) var<uniform> volumes: VolumeConstants;

const VOLUME_SITES: u32 = 6u;
const VOLUME_CHAMBER: u32 = 1u;
const VOLUME_ENTRANCE: u32 = 2u;
const CLIFF_LEVELS: u32 = 4u;
const SEED_CAVES: u32 = 0x5C4FE0A1u;
const SEED_ROUGH: u32 = 0x2D971E35u;
const SEED_CLIFF: u32 = 0x7A1BC9F3u;
const SEED_CLIFF_REGION: u32 = 0x41E806D7u;
const SEED_STRATA: u32 = 0x9B3C5A2Fu;
const H_ACTIVE: i32 = 0;
const H_DEPTH: i32 = 1;
const H_HUB_CHAMBER: i32 = 2;
const H_LINK: i32 = 3;
const H_ENTRANCE: i32 = 5;
const H_SITE: i32 = 6;
const H_CHAMBER: i32 = 14;
const H_RADIUS: i32 = 16;
const H_EDGE: i32 = 22;
const H_CHAMBER_DEPTH: i32 = 40;
const H_ENTRANCE_END: i32 = 44;

// A network capsule (`volumes::Primitive`): `a == b` is a chamber.
struct CavePrimitive {
    a: vec3<i32>,
    b: vec3<i32>,
    r: i32,
    rv: i32,
    inv_v: i32,
    floor: i32,
    flags: u32,
}

struct CaveSite { x: i32, y: i32, used: bool }

struct Cliff { rise: i32, cells: i32, noise: bool, strata: bool, wander: i32 }

fn volumes_caves() -> bool { return (volumes.head.x & 1) != 0; }
fn volumes_cliffs() -> bool { return (volumes.head.x & 2) != 0; }
fn volume_region_shift() -> u32 { return u32(volumes.head.y); }
fn volume_seed() -> u32 { return bitcast<u32>(volumes.head.w); }

// `ramp_q16`: `x / span` in Q16 for `x` clamped to `[0, span]`.
fn volume_ramp(x: i32, span: i32) -> i32 {
    if span <= 0 { return select(0, NOISE_ONE, x >= 0); }
    let xc = clamp(x, 0, span);
    let s = u32(max(i32(firstLeadingBit(u32(span))) + 1 - 15, 0));
    return ((xc >> s) << 16u) / max(span >> s, 1);
}

// The exact integer root of `x`.
fn volume_isqrt(x: u32) -> u32 {
    var r = min(u32(sqrt(f32(x))), 65535u);
    for (var n = 0u; n < 2u && r * r > x; n++) { r -= 1u; }
    for (var n = 0u; n < 2u && r < 65535u && (r + 1u) * (r + 1u) <= x; n++) { r += 1u; }
    return r;
}

fn volume_hash(face: u32, ri: i32, rj: i32, slot: i32) -> u32 {
    return hash3(ri, rj, i32(face) * 128 + slot, volume_seed() ^ SEED_CAVES);
}

fn volume_pick(lo: i32, hi: i32, h: u32) -> i32 {
    return lo + scale_q16(i32(h >> 16u), hi - lo);
}

fn volume_inside(ri: i32, rj: i32) -> bool {
    let n = volumes.cliff_mix.z >> volume_region_shift();
    return ri >= 0 && rj >= 0 && ri < n && rj < n;
}

fn volume_may_be_active(face: u32, ri: i32, rj: i32) -> bool {
    return volume_inside(ri, rj) && (volume_hash(face, ri, rj, H_ACTIVE) >> 16u) < u32(volumes.shares.x);
}

fn volume_gate_passes(face: u32, ri: i32, rj: i32, gate: i32) -> bool {
    return i32(volume_hash(face, ri, rj, H_ACTIVE) >> 16u) < ((volumes.shares.x * clamp(gate, 0, 256)) >> 8u);
}

fn volume_node(face: u32, ri: i32, rj: i32, n: i32) -> vec2<i32> {
    let s = 1 << volume_region_shift();
    let hx = i32(volume_hash(face, ri, rj, H_SITE + 2 * n) >> 16u);
    let hy = i32(volume_hash(face, ri, rj, H_SITE + 2 * n + 1) >> 16u);
    return vec2<i32>(ri * s + s / 4 + (((s / 2) * hx) >> 16u), rj * s + s / 4 + (((s / 2) * hy) >> 16u));
}

fn volume_hub_depth(face: u32, ri: i32, rj: i32) -> i32 {
    return volume_pick(volumes.depths.y, volumes.depths.z, volume_hash(face, ri, rj, H_DEPTH));
}

fn volume_entrance_end(face: u32, ri: i32, rj: i32) -> vec2<i32> {
    let s = 1 << volume_region_shift();
    let hub = volume_node(face, ri, rj, 0);
    let h = volume_hash(face, ri, rj, H_ENTRANCE_END);
    let dx = (((s / 2) * i32(h >> 16u)) >> 16u) - s / 4;
    let dy = (((s / 2) * i32(h & 0xffffu)) >> 16u) - s / 4;
    return vec2<i32>(clamp(hub.x + dx, ri * s + s / 4, ri * s + 3 * s / 4), clamp(hub.y + dy, rj * s + s / 4, rj * s + 3 * s / 4));
}

// Site `n` of region `(ri, rj)` (`VolumeLayer::sites`).
fn volume_site(face: u32, ri: i32, rj: i32, n: u32) -> CaveSite {
    var site = CaveSite(0, 0, false);
    if !volumes_caves() || !volume_may_be_active(face, ri, rj) { return site; }
    if n == 0u {
        let at = volume_node(face, ri, rj, 0);
        site = CaveSite(at.x, at.y, true);
    } else if n < 3u {
        let at = volume_node(face, ri, rj, i32(n));
        site = CaveSite(at.x, at.y, (volume_hash(face, ri, rj, H_CHAMBER + i32(n) - 1) >> 16u) < u32(volumes.shares.z));
    } else if n == 3u {
        let at = volume_entrance_end(face, ri, rj);
        site = CaveSite(at.x, at.y, (volume_hash(face, ri, rj, H_ENTRANCE) >> 16u) < u32(volumes.depths.x));
    } else {
        let axis = i32(n) - 4;
        let other = vec2<i32>(ri + 1 - axis, rj + axis);
        let at = volume_node(face, other.x, other.y, 0);
        let used = volume_may_be_active(face, other.x, other.y)
            && (volume_hash(face, ri, rj, H_LINK + axis) >> 16u) < u32(volumes.shares.w);
        site = CaveSite(at.x, at.y, used);
    }
    return site;
}

// The words of a primitive (`Primitive::words`).
fn cave_primitive_words(p: CavePrimitive) -> array<i32, 8> {
    return array<i32, 8>(p.a.x, p.a.y, p.a.z, p.b.x, p.b.y, p.b.z, p.r | (p.rv << 16u),
        p.inv_v | (p.floor << 16u) | i32(p.flags << 28u));
}

fn cave_primitive_of(w: array<i32, 8>) -> CavePrimitive {
    return CavePrimitive(vec3<i32>(w[0], w[1], w[2]), vec3<i32>(w[3], w[4], w[5]), w[6] & 0xffff, w[6] >> 16u,
        w[7] & 0xffff, (w[7] >> 16u) & 0xfff, u32(w[7]) >> 28u);
}

// `Primitive::reach`: past its axis, horizontally and vertically (Q2).
fn cave_reach(p: CavePrimitive) -> vec2<i32> {
    let rough = volumes.shapes.z;
    return vec2<i32>(p.r + ((p.r * rough * 3) >> 9u) + 2, p.rv + ((p.rv * rough * 3) >> 9u) + (p.rv >> 5u) + 2);
}

fn cave_covers(p: CavePrimitive, x: i32, y: i32) -> bool {
    let h = cave_reach(p).x;
    return x >= min(p.a.x, p.b.x) - h && x <= max(p.a.x, p.b.x) + h
        && y >= min(p.a.y, p.b.y) - h && y <= max(p.a.y, p.b.y) + h;
}

fn cave_z_range(p: CavePrimitive) -> vec2<i32> {
    let v = cave_reach(p).y;
    return vec2<i32>(min(p.a.z, p.b.z) - v, max(p.a.z, p.b.z) + v);
}

// `VolumeLayer::resolved`.
fn cave_resolved(p: CavePrimitive, level: u32) -> bool {
    let chamber = (p.flags & VOLUME_CHAMBER) != 0u;
    return min(p.r, p.rv) >= (4 << level) && i32(level) <= volumes.head.y - select(6, 5, chamber);
}

// The deepest level tunnels (`false`) or chambers are resolved at.
fn cave_deepest_level(chamber: bool) -> i32 {
    return volumes.head.y - select(6, 5, chamber);
}

// Level cells under a column's top caves may carve at `level`.
fn cave_max_depth_cells(level: u32) -> i32 {
    return (volumes.bounds.y >> level) / 4 + 2;
}

// Whether level cell `k` (centre `(x, y)`, Q2 from the footprint's first
// base cell `origin`) of a column of field height `height` is carved by
// `p` (`VolumeLayer::carves`).
fn volume_carves(p: CavePrimitive, x: i32, y: i32, origin: vec2<i32>, k: i32, level: u32, height: i32) -> bool {
    let zc = (4 * k + 2) << level;
    let range = cave_z_range(p);
    if !cave_covers(p, x, y) || zc < range.x || zc > range.y { return false; }
    let hq = div_floor(height * 4, volumes.bounds.z);
    let depth = hq - zc;
    let cover = volumes.bounds.x;
    let max_depth = volumes.bounds.y;
    if depth > max_depth { return false; }
    var s = 256;
    if (p.flags & VOLUME_ENTRANCE) == 0u && cover > 0 {
        let half = max(cover / 2, 1);
        s = (clamp(depth - half, 0, half) << 8u) / half;
    }
    let taper = max(max_depth / 16, 4);
    s = min(s, (min(max_depth - depth, taper) << 8u) / taper);
    let r_eff = (p.r * s) >> 8u;
    if r_eff <= 0 { return false; }
    let e = vec3<i32>(x - p.a.x, y - p.a.y, zc - p.a.z);
    let d = p.b - p.a;
    let len2 = d.x * d.x + d.y * d.y + d.z * d.z;
    var t12 = 0;
    if len2 > 0 {
        let t = clamp(e.x * d.x + e.y * d.y + e.z * d.z, 0, len2);
        let sh = u32(max(i32(firstLeadingBit(u32(len2))) + 1 - 18, 0));
        t12 = min(((t >> sh) << 12u) / max(len2 >> sh, 1), 4096);
    }
    let axis = (d * t12) >> vec3<u32>(12u);
    if p.floor < 256 && e.z < axis.z - ((p.rv * p.floor) >> 8u) { return false; }
    let q = vec3<i32>(e.x - axis.x, e.y - axis.y, ((e.z - axis.z) * p.inv_v) >> 8u);
    let ra = (r_eff * volumes.shapes.z) >> 8u;
    let outer = r_eff + ((ra * 3) >> 1u) + 1;
    if any(abs(q) >= vec3<i32>(outer)) { return false; }
    let dist2 = q.x * q.x + q.y * q.y + q.z * q.z;
    let inner = r_eff - ra;
    if inner > 0 && dist2 < inner * inner { return true; }
    if dist2 >= outer * outer { return false; }
    let n = noise(vec3<i32>(x + 4 * origin.x, y + 4 * origin.y, zc), u32(volumes.shapes.w), volume_seed() ^ SEED_ROUGH);
    let r2 = r_eff + ((ra * (n >> 4u)) >> 11u);
    return r2 > 0 && dist2 < r2 * r2;
}

// The cliff shape of a column (`VolumeLayer::cliff`).
fn volume_cliff(p: vec3<i32>, slope: i32, level: u32) -> Cliff {
    var none = Cliff(0, 0, false, false, 0);
    if !volumes_cliffs() || level >= CLIFF_LEVELS { return none; }
    let c = volumes.cliff;
    let gate = volume_ramp(slope - c.y, c.z - c.y);
    if gate == 0 { return none; }
    let seed = volume_seed();
    let n = noise(p, u32(c.w), seed ^ SEED_CLIFF_REGION);
    let region = clamp((n - volumes.cliff_shape.x) * 4, 0, NOISE_ONE);
    let a = scale_q16(scale_q16(region, c.x), gate);
    let cell = volumes.bounds.z << level;
    if a < cell { return none; }
    let s = clamp(slope, 0, 1000);
    let nq = i32(min(volume_isqrt(u32(64 + s * s)), 32u));
    let rise = (a * nq) >> 3u;
    let k = volumes.cliff_shape;
    let cell_du = (cell * 2) / 25;
    return Cliff(rise, rise / cell + 2, volumes.cliff_mix.x > 0 && (1 << u32(k.y)) >= 2 * cell_du,
        volumes.cliff_mix.y > 0 && k.z >= 2 * cell, scale_q16(noise(p, u32(k.w), seed ^ SEED_STRATA), k.z));
}

// Whether level cell `k` of a column with cliff `c` is solid, its volume
// point `q` (`VolumeLayer::cliff_solid`).
fn volume_cliff_solid(c: Cliff, q: vec3<i32>, height: i32, k: i32, level: u32) -> bool {
    let cell = volumes.bounds.z << level;
    let z = k * cell + cell / 2;
    let d = height - z;
    var g = 0;
    if c.noise {
        let n = noise(q, u32(volumes.cliff_shape.y), volume_seed() ^ SEED_CLIFF);
        g += (n >> 4u) * volumes.cliff_mix.x;
    }
    if c.strata {
        let period = volumes.cliff_shape.z;
        let f = rem_floor(z + c.wander, period);
        let tri = 2 * volume_ramp(abs(2 * f - period), period) - NOISE_ONE;
        g += (tri >> 4u) * volumes.cliff_mix.y;
    }
    let gc = clamp(g >> 4u, -NOISE_ONE, NOISE_ONE);
    return d + ((c.rise * (gc >> 4u)) >> 12u) > 0;
}

// A region's network (`VolumeLayer::network`): at most 13 primitives.
struct CaveNetwork { count: u32, prims: array<CavePrimitive, 13> }

fn volume_tunnel(net: ptr<function, CaveNetwork>, face: u32, ri: i32, rj: i32, a: vec3<i32>, b: vec3<i32>, e: i32, flags: u32) {
    let h0 = volume_hash(face, ri, rj, H_EDGE + 3 * e);
    let h1 = volume_hash(face, ri, rj, H_EDGE + 3 * e + 1);
    let h2 = volume_hash(face, ri, rj, H_EDGE + 3 * e + 2);
    let r = volume_pick(volumes.radii.x, volumes.radii.y, h0);
    let span = (1 << volume_region_shift()) / 4;
    let bend = volumes.bounds.w;
    let m = vec3<i32>(
        div_floor(a.x + b.x, 2) + ((2 * span * i32(h1 & 0xffffu)) >> 16u) - span,
        div_floor(a.y + b.y, 2) + ((2 * span * i32((h1 >> 16u) & 0xffffu)) >> 16u) - span,
        div_floor(a.z + b.z, 2) + ((2 * bend * i32(h2 & 0xffffu)) >> 16u) - bend);
    let shape = volumes.shapes.x;
    let rv = max((r * (shape & 0xffff)) >> 8u, 4);
    let inv_v = (r << 8u) / max(rv, 1);
    (*net).prims[(*net).count] = CavePrimitive(a, m, r, rv, inv_v, shape >> 16u, flags);
    (*net).prims[(*net).count + 1u] = CavePrimitive(m, b, r, rv, inv_v, shape >> 16u, flags);
    (*net).count += 2u;
}

fn volume_chamber(net: ptr<function, CaveNetwork>, face: u32, ri: i32, rj: i32, c: vec3<i32>, slot: i32) {
    let r = volume_pick(volumes.radii.z, volumes.radii.w, volume_hash(face, ri, rj, H_RADIUS + slot));
    let shape = volumes.shapes.y;
    let rv = max((r * (shape & 0xffff)) >> 8u, 4);
    (*net).prims[(*net).count] = CavePrimitive(c, c, r, rv, (r << 8u) / max(rv, 1), shape >> 16u, VOLUME_CHAMBER);
    (*net).count += 1u;
}

// Region `(ri, rj)`'s network, horizontal coordinates from base cell
// `origin`; `ground[s]` is site `s`'s reference height (mm) and gate (Q8).
fn volume_network(face: u32, ri: i32, rj: i32, at_sites: array<vec2<i32>, 6>, origin: vec2<i32>) -> CaveNetwork {
    var ground = at_sites;
    var net: CaveNetwork;
    net.count = 0u;
    var sites: array<CaveSite, 6>;
    for (var s = 0u; s < VOLUME_SITES; s++) { sites[s] = volume_site(face, ri, rj, s); }
    if !sites[0].used || !volume_gate_passes(face, ri, rj, ground[0].y) { return net; }
    let layer = volumes.bounds.z;
    let hub_depth = volume_hub_depth(face, ri, rj);
    let hub = vec3<i32>(4 * (sites[0].x - origin.x) + 2, 4 * (sites[0].y - origin.y) + 2, div_floor(ground[0].x * 4, layer) - hub_depth);
    if (volume_hash(face, ri, rj, H_HUB_CHAMBER) >> 16u) < u32(volumes.shares.y) {
        volume_chamber(&net, face, ri, rj, hub, 0);
    }
    let spread = (volumes.depths.z - volumes.depths.y) / 4;
    for (var m = 1u; m < 3u; m++) {
        if !sites[m].used { continue; }
        let h = volume_hash(face, ri, rj, H_CHAMBER_DEPTH + i32(m));
        let depth = clamp(hub_depth + volume_pick(-spread, spread, h), volumes.depths.y, volumes.depths.z);
        let c = vec3<i32>(4 * (sites[m].x - origin.x) + 2, 4 * (sites[m].y - origin.y) + 2, div_floor(ground[m].x * 4, layer) - depth);
        volume_chamber(&net, face, ri, rj, c, i32(m));
        volume_tunnel(&net, face, ri, rj, hub, c, i32(m) - 1, 0u);
    }
    for (var axis = 0u; axis < 2u; axis++) {
        let other = vec2<i32>(ri + 1 - i32(axis), rj + i32(axis));
        let s = sites[4u + axis];
        if !s.used || !volume_gate_passes(face, other.x, other.y, ground[4u + axis].y) { continue; }
        let b = vec3<i32>(4 * (s.x - origin.x) + 2, 4 * (s.y - origin.y) + 2, div_floor(ground[4u + axis].x * 4, layer) - volume_hub_depth(face, other.x, other.y));
        volume_tunnel(&net, face, ri, rj, hub, b, 2 + i32(axis), 0u);
    }
    if sites[3].used {
        let end = vec3<i32>(4 * (sites[3].x - origin.x) + 2, 4 * (sites[3].y - origin.y) + 2, div_floor(ground[3].x * 4, layer) + volumes.depths.w);
        volume_tunnel(&net, face, ri, rj, hub, end, 4, VOLUME_ENTRANCE);
    }
    return net;
}
