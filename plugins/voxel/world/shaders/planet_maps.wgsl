// Planet maps: climate and river lookups; mirror of src/planet_maps.rs.
// The maps' header is `terrain.maps` (`planet_maps::MAP_HEADER` rows), their
// words the engine's `terrain_tables`.
const MAP_FLAG_CLIMATE: i32 = 1;
const MAP_FLAG_RIVERS: i32 = 2;
const MAP_FLAG_EROSION: i32 = 4;
const MAP_FLAG_DRAINAGE: i32 = 8;
const MAP_CUT_UNIT: i32 = 500;
const MAP_FILL_UNIT: i32 = 1000;
const MAP_GHOST: i32 = 2;
const MAP_RECORD_WORDS: u32 = 8u;

fn map_word(i: u32) -> u32 {
    return terrain_tables[i >> 2u][i & 3u];
}

// Slot (face) and Q16 texel coordinates of domain point `p` on the river
// grid (`planet_maps::coords`).
fn map_coords(p: vec3<i32>) -> vec3<i32> {
    let a = abs(p);
    var face = 0u;
    if a.x >= a.y && a.x >= a.z {
        face = select(0u, 1u, p.x < 0);
    } else if a.y >= a.z {
        face = select(2u, 3u, p.y < 0);
    } else {
        face = select(4u, 5u, p.z < 0);
    }
    return map_coords_on(p, face);
}

// The face whose normal is the signed axis `v` (`planet_maps::face_of_normal`).
fn map_face_of_normal(v: vec3<i32>) -> u32 {
    if v.x != 0 { return select(0u, 1u, v.x < 0); }
    if v.y != 0 { return select(2u, 3u, v.y < 0); }
    return select(4u, 5u, v.z < 0);
}

// `map_coords` of `p` on face `face`, maybe past its edges
// (`planet_maps::coords_on`).
fn map_coords_on(p: vec3<i32>, face: u32) -> vec3<i32> {
    let half = terrain.maps[2].x;
    var slot = 0u;
    var pn = half;
    var pa = p.x;
    var pb = -p.z;
    if half <= 0 {
        slot = face;
        pn = dot(face_axis(slot, 0u), p);
        pa = dot(face_axis(slot, 1u), p);
        pb = dot(face_axis(slot, 2u), p);
    }
    let n = u32(max(pn, 1));
    let bits = 32u - countLeadingZeros(n);
    let r = landform_recip_q30(map_norm(n, n, bits));
    let side = u32(terrain.maps[0].y) << 15u;
    return vec3<i32>(i32(slot), map_texel(pa, n, bits, r, side), map_texel(pb, n, bits, r, side));
}

fn map_norm(x: u32, n: u32, bits: u32) -> u32 {
    let c = min(x, n);
    if bits > 30u { return c >> (bits - 30u); }
    return c << (30u - bits);
}

fn map_texel(c: i32, n: u32, bits: u32, r: u32, side: u32) -> i32 {
    let m = i32(min(mul_shr(map_norm(u32(abs(c)), n, bits), r, 30u), Q30));
    let u = select(m, -m, c < 0);
    return i32(mul_shr(bitcast<u32>(u + i32(Q30)), side, 30u));
}

// Climate at domain point `p`, height `height` (mm): temperature (1/128 °C)
// and moisture (Q16) (`planet_maps::climate`).
fn terrain_climate(p: vec3<i32>, height: i32) -> vec2<i32> {
    let h = terrain.maps;
    let fall = div_floor(height, 1000) * h[2].w / 1000;
    if (h[0].x & MAP_FLAG_CLIMATE) == 0 {
        return vec2<i32>(h[2].y - fall, h[2].z);
    }
    let c = map_coords(p);
    let nc = h[0].w;
    let sc = h[1].w;
    let f = (c.yz >> vec2<u32>(1u)) - vec2<i32>(32768);
    let i = clamp(f >> vec2<u32>(16u), vec2<i32>(-1), vec2<i32>(nc - 1));
    let w = clamp(f - (i << vec2<u32>(16u)), vec2<i32>(0), vec2<i32>(65536)) >> vec2<u32>(8u);
    let at = u32(h[1].y) + u32(c.x) * u32(sc * sc) + u32((i.y + 1) * sc + i.x + 1);
    let w00 = map_word(at);
    let w10 = map_word(at + 1u);
    let w01 = map_word(at + u32(sc));
    let w11 = map_word(at + u32(sc) + 1u);
    let t = vec4<i32>(bitcast<i32>(w00 << 16u) >> 16u, bitcast<i32>(w10 << 16u) >> 16u, bitcast<i32>(w01 << 16u) >> 16u, bitcast<i32>(w11 << 16u) >> 16u);
    let m = vec4<i32>(vec4<u32>(w00, w10, w01, w11) >> vec4<u32>(16u));
    let ta = t.x + (((t.y - t.x) * w.x) >> 8u);
    let tb = t.z + (((t.w - t.z) * w.x) >> 8u);
    let ma = m.x + (((m.y - m.x) * w.x) >> 8u);
    let mb = m.z + (((m.w - m.z) * w.x) >> 8u);
    return vec2<i32>(ta + (((tb - ta) * w.y) >> 8u) - fall, ma + (((mb - ma) * w.y) >> 8u));
}

// A vertex grid (`n + 1` a side) starting at `at`, both signed halves of
// its words in their units (mm), bilinear (`planet_maps::vertex_pair`).
fn map_vertex_pair(p: vec3<i32>, at: i32, units: vec2<i32>) -> vec2<i32> {
    let h = terrain.maps;
    let c = map_coords(p);
    let n = h[0].y;
    let v = n + 1;
    let i = clamp(c.yz >> vec2<u32>(16u), vec2<i32>(0), vec2<i32>(n - 1));
    let w = clamp(c.yz - (i << vec2<u32>(16u)), vec2<i32>(0), vec2<i32>(65536));
    let base = u32(at) + u32(c.x) * u32(v * v) + u32(i.y * v + i.x);
    let words = vec4<u32>(map_word(base), map_word(base + 1u), map_word(base + u32(v)), map_word(base + u32(v) + 1u));
    let lo = (bitcast<vec4<i32>>(words << vec4<u32>(16u)) >> vec4<u32>(16u)) * units.x;
    let hi = (bitcast<vec4<i32>>(words) >> vec4<u32>(16u)) * units.y;
    let lo_top = lo.x + mul_shr_signed(lo.y - lo.x, w.x, 16u);
    let lo_bottom = lo.z + mul_shr_signed(lo.w - lo.z, w.x, 16u);
    let hi_top = hi.x + mul_shr_signed(hi.y - hi.x, w.x, 16u);
    let hi_bottom = hi.z + mul_shr_signed(hi.w - hi.z, w.x, 16u);
    return vec2<i32>(lo_top + mul_shr_signed(lo_bottom - lo_top, w.y, 16u), hi_top + mul_shr_signed(hi_bottom - hi_top, w.y, 16u));
}

// The worn terrain at `p` (mm): the drainage's cut and its sediment's fill
// level (`planet_maps::erosion`).
fn terrain_erosion(p: vec3<i32>) -> vec2<i32> {
    if (terrain.maps[0].x & MAP_FLAG_EROSION) == 0 { return vec2<i32>(0, i32(0x80000000u)); }
    let w = map_vertex_pair(p, terrain.maps[4].x, vec2<i32>(MAP_CUT_UNIT, MAP_FILL_UNIT));
    return vec2<i32>(min(w.x, 0), w.y);
}

// The drainage at `p` (`planet_maps::drainage`): log2(1 + km²) of the wet
// area draining through it (1/2048), its texel's flags, the nearest river's
// distance (domain units) and bed (mm).
fn terrain_drainage(p: vec3<i32>) -> vec4<i32> {
    let h = terrain.maps;
    if (h[0].x & MAP_FLAG_DRAINAGE) == 0 { return vec4<i32>(0, 0, 0x7fffffff, 0); }
    let c = map_coords(p);
    let n = h[0].y;
    let s = h[0].z;
    let f = c.yz - vec2<i32>(32768);
    let i = clamp(f >> vec2<u32>(16u), vec2<i32>(-1), vec2<i32>(n - 1));
    let w = clamp(f - (i << vec2<u32>(16u)), vec2<i32>(0), vec2<i32>(65536)) >> vec2<u32>(8u);
    let at = u32(h[4].y) + u32(c.x) * u32(s * s) + u32((i.y + MAP_GHOST) * s + i.x + MAP_GHOST);
    let l = vec4<i32>(vec4<u32>(map_word(at), map_word(at + 1u), map_word(at + u32(s)), map_word(at + u32(s) + 1u)) & vec4<u32>(0xffffu));
    let la = l.x + (((l.y - l.x) * w.x) >> 8u);
    let lb = l.z + (((l.w - l.z) * w.x) >> 8u);
    let t = min(c.yz >> vec2<u32>(16u), vec2<i32>(n - 1));
    let flags = i32(map_word(u32(h[4].y) + u32(c.x) * u32(s * s) + u32((t.y + MAP_GHOST) * s + t.x + MAP_GHOST)) >> 16u);
    let near = map_nearest_river(p);
    return vec4<i32>(la + (((lb - la) * w.y) >> 8u), flags, near.x, near.y);
}

// The valley wall's rise `x` past a floodplain's edge, its foot curved
// (`planet_maps::wall_rise`).
fn map_wall_rise(x: i32, floodplain: i32, wall: i32) -> i32 {
    let w = clamp(floodplain / 2, 1, 32767);
    if x < w {
        return mul_shr_signed(mul_shr_signed(x, wall * 25, 7u), (x << 15u) / w, 16u);
    }
    return mul_shr_signed(x - w / 2, wall * 25, 7u);
}

// Meanders: the bend fields (`planet_maps::bends`).
const MEANDER_SHIFT_0: u32 = 20u;
const MEANDER_SHIFT_1: u32 = 18u;
const MEANDER_SHIFT_2: u32 = 16u;
const MEANDER_MAX_0: i32 = 67200;
const MEANDER_MAX_1: i32 = 32768;
const MEANDER_MAX_2: i32 = 8192;

fn map_bends(p: vec3<i32>, seed: u32) -> array<vec3<i32>, 3> {
    return array<vec3<i32>, 3>(
        vec3<i32>(noise(p, MEANDER_SHIFT_0, seed ^ 0x2f6b1c43u), noise(p, MEANDER_SHIFT_0, seed ^ 0x7a1d93e5u), noise(p, MEANDER_SHIFT_0, seed ^ 0x51c8e0a7u)),
        vec3<i32>(noise(p, MEANDER_SHIFT_1, seed ^ 0x4d9e83b1u), noise(p, MEANDER_SHIFT_1, seed ^ 0x16c2a5f7u), noise(p, MEANDER_SHIFT_1, seed ^ 0x63f04e29u)),
        vec3<i32>(noise(p, MEANDER_SHIFT_2, seed ^ 0x0b4e7f29u), noise(p, MEANDER_SHIFT_2, seed ^ 0x6c932d51u), noise(p, MEANDER_SHIFT_2, seed ^ 0x3e58a6bdu)));
}

// A segment's meander amplitudes (`planet_maps::meander_amplitudes`).
fn map_meanders(floodplain: i32, channel: i32) -> vec3<i32> {
    let room = max(floodplain - channel, 0) * 9 / 16;
    let w = clamp((channel - 2560) * 256 / 5120, 0, 256);
    let swing = min(channel * 3 / 2, room);
    return vec3<i32>(min(floodplain / 3, MEANDER_MAX_0), min((swing * w) >> 8u, MEANDER_MAX_1), min((swing * (256 - w)) >> 8u, MEANDER_MAX_2));
}

// `p` swung by field `bend` scaled to `a` (`planet_maps::swing`).
fn map_swing(p: vec3<i32>, bend: vec3<i32>, a: i32) -> vec3<i32> {
    return p + vec3<i32>(scale_q16(bend.x, a), scale_q16(bend.y, a), scale_q16(bend.z, a));
}

// Distance from `p` to the segment from `node` along `e` (Q30) for `length`,
// and its foot's position along it (`planet_maps::axis`).
fn map_axis(p: vec3<i32>, node: vec3<i32>, e: vec3<i32>, length: i32) -> vec2<i32> {
    let d = p - node;
    let along = mul_shr_signed(d.x, e.x, 30u) + mul_shr_signed(d.y, e.y, 30u) + mul_shr_signed(d.z, e.z, 30u);
    let s = clamp(along, 0, max(length, 0));
    let w = d - vec3<i32>(mul_shr_signed(s, e.x, 30u), mul_shr_signed(s, e.y, 30u), mul_shr_signed(s, e.z, 30u));
    let n = unit_q30(w);
    let dist = max(mul_shr_signed(w.x, n.x, 30u) + mul_shr_signed(w.y, n.y, 30u) + mul_shr_signed(w.z, n.z, 30u), 0);
    return vec2<i32>(dist, s);
}

// One river segment at `p` (`planet_maps::segment`), for cells `cell`
// domain units wide: its carving surface, whether `p` lies in its channel,
// the distance to its axis and its bed.
fn map_segment(at: u32, column: vec3<i32>, bend: array<vec3<i32>, 3>, cell: i32) -> vec4<i32> {
    let a = terrain_tables[at >> 2u];
    let b = terrain_tables[(at >> 2u) + 1u];
    let node = bitcast<vec3<i32>>(a.xyz);
    let geometry = a.w;
    let wall = i32((geometry >> 2u) & 255u);
    let floodplain = i32((geometry >> 10u) & 2047u) * 640;
    let channel = i32((geometry >> 21u) & 2047u) * 40;
    let e = vec3<i32>(bitcast<i32>(b.x << 16u) >> 16u, bitcast<i32>(b.x) >> 16u, bitcast<i32>(b.y << 16u) >> 16u) << vec3<u32>(15u);
    let length = i32(b.y >> 16u) << 6u;
    // Meanders (`planet_maps::segment`): the valley swings, its channel on
    // its floodplain.
    let m = map_meanders(floodplain, channel);
    let valley = map_swing(column, bend[0], m.x);
    let foot = map_axis(valley, node, e, length);
    let dist = foot.x;
    let s = foot.y;
    var t = 0;
    if length > 0 { t = min((s << 8u) / max(length >> 8u, 1), 65536); }
    let bed = bitcast<i32>(b.z) - mul_shr_signed(i32(b.w & 0x3fffffu), t, 16u);
    var z = bed + map_wall_rise(max(dist - floodplain, 0), floodplain, wall);
    var inside = 0;
    var flow = 0x7fffffff;
    if dist < floodplain && channel > 0 {
        flow = map_axis(map_swing(map_swing(valley, bend[1], m.y), bend[2], m.z), node, e, length).x;
    }
    if flow < channel {
        let q = (flow << 14u) / channel;
        let depth = i32(b.w >> 22u) * 100;
        let cut = depth - mul_shr_signed(depth, (q * q) >> 14u, 14u);
        let resolved = clamp(((channel << 7u) / cell) - 256, 0, 256);
        z -= (cut * resolved) >> 8u;
        inside = select(0, 1, resolved > 0);
    }
    return vec4<i32>(z, inside, dist, bed);
}

// Height `h` of the column at `p` (reference level `level`) carved by the
// rivers, and whether it lies in a channel (`planet_maps::carve`).
fn map_carve(p: vec3<i32>, level: u32, h: i32) -> vec2<i32> {
    let header = terrain.maps;
    if (header[0].x & MAP_FLAG_RIVERS) == 0 { return vec2<i32>(h, 0); }
    let c = map_coords(p);
    let n = header[0].y;
    let s = header[0].z;
    let ix = min(c.y >> 16u, n - 1);
    let iy = min(c.z >> 16u, n - 1);
    let index = u32(header[1].x) + u32(c.x) * u32(s * s);
    let cell = 8 << min(level, 24u);
    var best = 0x7fffffff;
    var inside = 0;
    var bent = false;
    var bend = array<vec3<i32>, 3>(vec3<i32>(0), vec3<i32>(0), vec3<i32>(0));
    for (var dy = -MAP_GHOST; dy <= MAP_GHOST; dy++) {
        for (var dx = -MAP_GHOST; dx <= MAP_GHOST; dx++) {
            let tx = ix + dx;
            let ty = iy + dy;
            let word = map_word(index + u32((ty + MAP_GHOST) * s + tx + MAP_GHOST));
            if word == 0u { continue; }
            let reach = i32(word >> 24u) << 12u;
            if abs(c.y - ((tx << 16u) + 32768)) > reach || abs(c.z - ((ty << 16u) + 32768)) > reach { continue; }
            let at = u32(header[1].z) + ((word & 0xffffffu) - 1u) * MAP_RECORD_WORDS;
            if !bent {
                bend = map_bends(p, bitcast<u32>(header[3].w));
                bent = true;
            }
            let r = map_segment(at, p, bend, cell);
            best = min(best, r.x);
            if r.y != 0 && r.x <= h { inside = 1; }
        }
    }
    if best == 0x7fffffff { return vec2<i32>(h, 0); }
    return vec2<i32>(landform_smooth_min(h, best, header[3].z), inside);
}

// Streams (`crate::streams`): the program's regions hold each parent cell's
// stream records (`streams::RECORD_WORDS` words: node, target, beds,
// floodplain, channel | depth << 17, reach, cap | wall << 24).
const MAP_FLAG_STREAMS: i32 = 16;
const STREAM_RECORD_WORDS: u32 = 13u;

// Whether a point `d` (Q16 texel units) from a level's cell lies in a
// record's box (`streams::in_box`).
fn stream_in_box(b: u32, d: vec2<i32>, shift: u32) -> bool {
    let u = (d.x << 5u) >> shift;
    let v = (d.y << 5u) >> shift;
    let lo = vec2<i32>(bitcast<i32>(b << 24u) >> 24u, bitcast<i32>(b << 8u) >> 24u);
    let hi = vec2<i32>(bitcast<i32>(b << 16u) >> 24u, bitcast<i32>(b) >> 24u);
    return u >= lo.x && u <= hi.x && v >= lo.y && v <= hi.y;
}

// `a / b` in Q16, exactly in 32 bits (`streams::ratio_q16`).
fn stream_ratio(a: i32, b: i32) -> i32 {
    let n = a << 8u;
    return ((n / b) << 8u) + ((n % b) << 8u) / b;
}

// How much of a stream of reach `reach` shows at cells `cell` wide (Q8,
// `streams::resolved`).
fn stream_resolved(reach: i32, cell: i32) -> i32 {
    return clamp((min(reach, 1 << 23u) << 6u) / max(cell, 1) - 256, 0, 256);
}

// The carve's weight (Q16) at distance `d` (`streams::weight`).
fn stream_weight(d: i32, reach: i32) -> i32 {
    let half = reach / 2;
    if d <= half { return 65536; }
    let x = stream_ratio(min(d - half, reach - half), max(reach - half, 1));
    let s = mul_shr_signed(mul_shr_signed(x, x, 16u), 3 * 65536 - 2 * x, 16u);
    return 65536 - s;
}

// Whether `p` lies within a stream record's box: its segment's bounds
// grown by its reach and meanders (`streams::near_box`).
fn stream_near_box(slot: u32, at: u32, p: vec3<i32>, reach: i32) -> bool {
    let node = vec3<i32>(bitcast<i32>(terrain_region_word(slot, at)), bitcast<i32>(terrain_region_word(slot, at + 1u)), bitcast<i32>(terrain_region_word(slot, at + 2u)));
    let tip = vec3<i32>(bitcast<i32>(terrain_region_word(slot, at + 3u)), bitcast<i32>(terrain_region_word(slot, at + 4u)), bitcast<i32>(terrain_region_word(slot, at + 5u)));
    let grow = vec3<i32>(reach);
    return all(p >= min(node, tip) - grow) && all(p <= max(node, tip) + grow);
}

struct StreamAt {
    z: i32,
    inside: bool,
    dist: i32,
}

// A stream record (at word `at` of region `slot`) at `column`: its valley
// straight, its channel meandering, the bend fields (`bend`, made once
// `bent`) only where a channel may be (`streams::surface`).
fn stream_surface(slot: u32, at: u32, column: vec3<i32>, bend: ptr<function, array<vec3<i32>, 3>>, bent: ptr<function, bool>, cell: i32) -> StreamAt {
    let node = vec3<i32>(bitcast<i32>(terrain_region_word(slot, at)), bitcast<i32>(terrain_region_word(slot, at + 1u)), bitcast<i32>(terrain_region_word(slot, at + 2u)));
    let tip = vec3<i32>(bitcast<i32>(terrain_region_word(slot, at + 3u)), bitcast<i32>(terrain_region_word(slot, at + 4u)), bitcast<i32>(terrain_region_word(slot, at + 5u)));
    let bed0 = bitcast<i32>(terrain_region_word(slot, at + 6u));
    let bed1 = bitcast<i32>(terrain_region_word(slot, at + 7u));
    let floodplain = bitcast<i32>(terrain_region_word(slot, at + 8u));
    let w9 = terrain_region_word(slot, at + 9u);
    let channel = i32(w9 & 0x1ffffu);
    let depth = i32(w9 >> 17u);
    let w11 = terrain_region_word(slot, at + 11u);
    let wall = i32(w11 >> 24u);
    let span = tip - node;
    let e = unit_q30(span);
    let length = mul_shr_signed(span.x, e.x, 30u) + mul_shr_signed(span.y, e.y, 30u) + mul_shr_signed(span.z, e.z, 30u);
    let m = map_meanders(floodplain, channel);
    let foot = map_axis(column, node, e, length);
    let dist = foot.x;
    let s = foot.y;
    var t = 0;
    if length > 0 { t = stream_ratio(s, max(length, s)); }
    let bed = bed0 - mul_shr_signed(bed0 - bed1, t, 16u);
    var z = bed + map_wall_rise(max(dist - floodplain, 0), floodplain, wall);
    var inside = false;
    var flow = 0x7fffffff;
    if dist < floodplain && channel > 0 {
        if !*bent {
            *bend = map_bends(column, bitcast<u32>(terrain.maps[3].w));
            *bent = true;
        }
        let b = *bend;
        flow = map_axis(map_swing(map_swing(column, b[1], m.y), b[2], m.z), node, e, length).x;
    }
    if flow < channel {
        let q = (flow << 14u) / max(channel, 1);
        let cut = depth - mul_shr_signed(depth, (q * q) >> 14u, 14u);
        let resolved = clamp(((channel << 7u) / max(cell, 1)) - 256, 0, 256);
        z -= (cut * resolved) >> 8u;
        inside = resolved > 0;
    }
    return StreamAt(z, inside, dist);
}

// The streams' carve of column height `h` at `p` (reference level `level`)
// and whether it lies in a channel (`streams::Streams::carve`).
fn map_streams(p: vec3<i32>, level: u32, h: i32) -> vec2<i32> {
    let header = terrain.maps;
    if (header[0].x & MAP_FLAG_STREAMS) == 0 { return vec2<i32>(h, 0); }
    let levels = u32(header[0].x >> 8u) & 3u;
    let cell = 8 << min(level, 24u);
    let n0 = header[0].y;
    var out = h;
    var inside = false;
    var bent = false;
    var bend = array<vec3<i32>, 3>(vec3<i32>(0), vec3<i32>(0), vec3<i32>(0));
    var last_key = vec2<u32>(0xffffffffu);
    var last_region = -1;
    for (var r = 0u; r <= levels; r++) {
        if stream_resolved(header[4].w >> (2u * r), cell) == 0 { continue; }
        let shift = 16u - 2u * r;
        let n = n0 << (2u * r);
        // The cells around `p` on its face and the faces it lies within a
        // cell of (`streams::cells_around`).
        let own = map_coords(p);
        var faces = array<vec3<i32>, 3>(own, vec3<i32>(-1), vec3<i32>(-1));
        if header[2].x <= 0 {
            let margin = 1 << shift;
            let last = (n0 << 16u) - margin;
            let ex = select(select(0, 1, own.y >= last), -1, own.y < margin);
            let ey = select(select(0, 1, own.z >= last), -1, own.z < margin);
            var k = 1;
            if ex != 0 {
                faces[k] = map_coords_on(p, map_face_of_normal(face_axis(u32(own.x), 1u) * ex));
                k++;
            }
            if ey != 0 {
                faces[k] = map_coords_on(p, map_face_of_normal(face_axis(u32(own.x), 2u) * ey));
            }
        }
        for (var f = 0; f < 3; f++) {
            let fc = faces[f];
            if fc.x < 0 { continue; }
            let slot = u32(fc.x);
            let ci = fc.y >> shift;
            let cj = fc.z >> shift;
            for (var dj = -1; dj <= 1; dj++) {
                for (var di = -1; di <= 1; di++) {
                    let x = ci + di;
                    let y = cj + dj;
                    if x < 0 || y < 0 || x >= n || y >= n { continue; }
                    // A texel's stem in its own region, a refined cell's
                    // record in its parent's.
                    var key = vec2<u32>((slot << 2u) | (u32(x) << 5u), u32(y));
                    var record = 16u;
                    if r > 0u {
                        key = vec2<u32>((r - 1u) | (slot << 2u) | (u32(x >> 2u) << 5u), u32(y >> 2u));
                        record = u32(x & 3) + 4u * u32(y & 3);
                    }
                    // Neighbouring cells share their parents' regions.
                    if key.x != last_key.x || key.y != last_key.y {
                        last_key = key;
                        last_region = terrain_region(key);
                    }
                    let region = last_region;
                    if region < 0 { continue; }
                    let at = record * STREAM_RECORD_WORDS;
                    let reach = bitcast<i32>(terrain_region_word(u32(region), at + 10u));
                    if reach == 0 { continue; }
                    // The record's box first: one word, most cells' last.
                    let d = vec2<i32>(fc.y - (x << shift), fc.z - (y << shift));
                    if !stream_in_box(terrain_region_word(u32(region), at + 12u), d, shift) { continue; }
                    if !stream_near_box(u32(region), at, p, reach) { continue; }
                    let s = stream_surface(u32(region), at, p, &bend, &bent, cell);
                    if s.dist >= reach { continue; }
                    let cap = bitcast<i32>(terrain_region_word(u32(region), at + 11u) & 0xffffffu);
                    let carved = landform_smooth_min(h, s.z, cap / 8);
                    let cut = min(h - carved, cap);
                    let w = mul_shr_signed(stream_weight(s.dist, reach), stream_resolved(reach, cell), 8u);
                    let lowered = h - mul_shr_signed(cut, w, 16u);
                    if lowered < out { out = lowered; }
                    if s.inside && s.z <= out { inside = true; }
                }
            }
        }
    }
    return vec2<i32>(out, select(0, 1, inside));
}

// The nearest river or lake segment whose valley reaches `p`: its axis's
// distance (domain units, i32 max: none) and bed (`planet_maps::nearest_river`).
fn map_nearest_river(p: vec3<i32>) -> vec2<i32> {
    let header = terrain.maps;
    var best = vec2<i32>(0x7fffffff, 0);
    if (header[0].x & MAP_FLAG_RIVERS) == 0 { return best; }
    let c = map_coords(p);
    let n = header[0].y;
    let s = header[0].z;
    let ix = min(c.y >> 16u, n - 1);
    let iy = min(c.z >> 16u, n - 1);
    let index = u32(header[1].x) + u32(c.x) * u32(s * s);
    var found = false;
    var bent = false;
    var bend = array<vec3<i32>, 3>(vec3<i32>(0), vec3<i32>(0), vec3<i32>(0));
    for (var dy = -MAP_GHOST; dy <= MAP_GHOST; dy++) {
        for (var dx = -MAP_GHOST; dx <= MAP_GHOST; dx++) {
            let tx = ix + dx;
            let ty = iy + dy;
            let word = map_word(index + u32((ty + MAP_GHOST) * s + tx + MAP_GHOST));
            if word == 0u { continue; }
            let reach = i32(word >> 24u) << 12u;
            if abs(c.y - ((tx << 16u) + 32768)) > reach || abs(c.z - ((ty << 16u) + 32768)) > reach { continue; }
            let at = u32(header[1].z) + ((word & 0xffffffu) - 1u) * MAP_RECORD_WORDS;
            if !bent {
                bend = map_bends(p, bitcast<u32>(header[3].w));
                bent = true;
            }
            let r = map_segment(at, p, bend, 1 << 24u);
            if !found || r.z < best.x {
                best = r.zw;
                found = true;
            }
        }
    }
    return best;
}
