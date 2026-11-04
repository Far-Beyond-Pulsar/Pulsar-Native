// World-space irradiance probes (README.md): the clipmap, a probe's record,
// rays and hits. Shared by the update, the resolve and the tracers, which
// bind `frame_gi` (a `GiFrame`).

const GI_LEVELS: u32 = 6u;
const GI_SIDE: u32 = 24u;
const GI_LEVEL_PROBES: u32 = 13824u; // GI_SIDE^3
const GI_PROBES: u32 = 82944u;
const GI_RAYS: u32 = 64u;
// Probes updated a frame at most (`gi_lists`' two update lists).
const GI_MAX_UPDATES: u32 = 2720u;
// A probe's record in `gi_probes` (words): irradiance / pi as L1 spherical
// harmonics (RGB, half floats in 6 words), its state, then its distance
// moments (8x8 octahedral texels: mean, mean^2 as half floats).
const GI_PROBE_WORDS: u32 = 72u;
const GI_STATE_WORD: u32 = 6u;
const GI_MOMENTS_WORD: u32 = 8u;
const GI_MOMENTS_SIDE: u32 = 8u;
// State: bit 0 active (not inside solid ground), bits 8.. the updates it
// has taken (saturating).
const GI_ACTIVE: u32 = 1u;

struct GiLevel {
    // The eye-relative position of the grid's first probe (logical index
    // 0), and the spacing.
    origin: vec4<f32>,
    // The toroidal base: the storage coordinates of logical index 0.
    base: vec4<u32>,
}

struct GiFrame {
    levels: array<GiLevel, 6>,
    // Without an atmosphere, the sky's radiance above the horizon and
    // below it (rgb).
    sky: vec4<f32>,
    ground: vec4<f32>,
    // Probes updated this frame, last frame, probes reset this frame, and
    // which of `gi_lists`' update lists is this frame's.
    counts: vec4<u32>,
    // This frame's ray rotation (a unit quaternion), and last frame's.
    rotation: vec4<f32>,
    prev_rotation: vec4<f32>,
}

// A ray (`gi_rays`): origin (eye-relative), the most distance; direction,
// then its probe's storage index.
struct GiRay {
    origin: vec4<f32>,
    dir: vec3<f32>,
    probe: u32,
}

// A tracer's hit (`gi_hits`, one a ray): its distance (0: none), its normal
// (packed, `gi_pack_unit`), albedo (RGB8) and the sun's visibility from it
// (a byte in the albedo word's alpha); and in `light`, bytes: the tracer
// (bits 0..8), the share of light the surface lets through from its far
// side, tinted by its albedo (bits 8..16: foliage), the sun's visibility
// from that far side (bits 16..24), and the light it gives off, its albedo
// times this (bits 24..32: sixteenths).
struct GiHit {
    t: f32,
    normal: u32,
    albedo: u32,
    light: u32,
}

fn gi_pack_unit(n: vec3<f32>) -> u32 {
    let e = clamp(n * 0.5 + 0.5, vec3<f32>(0.0), vec3<f32>(1.0));
    return u32(e.x * 1023.0 + 0.5) | (u32(e.y * 1023.0 + 0.5) << 10u) | (u32(e.z * 1023.0 + 0.5) << 20u);
}

fn gi_unpack_unit(p: u32) -> vec3<f32> {
    let e = vec3<f32>(f32(p & 1023u), f32((p >> 10u) & 1023u), f32((p >> 20u) & 1023u)) / 1023.0;
    let n = e * 2.0 - 1.0;
    return n / max(length(n), 1e-6);
}

// Probe `index`'s level, and its logical coordinates in its grid.
fn gi_probe_cell(index: u32) -> vec4<u32> {
    let level = index / GI_LEVEL_PROBES;
    let at = index % GI_LEVEL_PROBES;
    let s = vec3<u32>(at % GI_SIDE, (at / GI_SIDE) % GI_SIDE, at / (GI_SIDE * GI_SIDE));
    return vec4<u32>((s + GI_SIDE - frame_gi.levels[level].base.xyz) % GI_SIDE, level);
}

// Probe `index`'s eye-relative position.
fn gi_probe_position(index: u32) -> vec3<f32> {
    let c = gi_probe_cell(index);
    let level = frame_gi.levels[c.w];
    return level.origin.xyz + vec3<f32>(c.xyz) * level.origin.w;
}

// The storage index of logical probe `c` of `level`.
fn gi_probe_index(level: u32, c: vec3<u32>) -> u32 {
    let s = (c + frame_gi.levels[level].base.xyz) % GI_SIDE;
    return level * GI_LEVEL_PROBES + s.x + GI_SIDE * (s.y + GI_SIDE * s.z);
}

// Ray `k` of a probe's set (spherical Fibonacci) turned by quaternion `q`.
fn gi_ray_dir(k: u32, q: vec4<f32>) -> vec3<f32> {
    let i = f32(k) + 0.5;
    let z = 1.0 - 2.0 * i / f32(GI_RAYS);
    let r = sqrt(max(1.0 - z * z, 0.0));
    let phi = i * 2.39996323;
    let d = vec3<f32>(r * cos(phi), r * sin(phi), z);
    return d + 2.0 * cross(q.xyz, cross(q.xyz, d) + q.w * d);
}

// Octahedral mapping of a unit direction to [0, 1]^2 and back.
fn gi_oct_encode(d: vec3<f32>) -> vec2<f32> {
    var p = d.xy / (abs(d.x) + abs(d.y) + abs(d.z));
    if d.z < 0.0 {
        p = (1.0 - abs(p.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), p >= vec2<f32>(0.0));
    }
    return p * 0.5 + 0.5;
}

fn gi_oct_decode(e: vec2<f32>) -> vec3<f32> {
    let f = e * 2.0 - 1.0;
    var n = vec3<f32>(f, 1.0 - abs(f.x) - abs(f.y));
    if n.z < 0.0 {
        let xy = (1.0 - abs(n.yx)) * select(vec2<f32>(-1.0), vec2<f32>(1.0), n.xy >= vec2<f32>(0.0));
        n = vec3<f32>(xy, n.z);
    }
    return normalize(n);
}

// L1 spherical harmonics basis at unit direction `d`.
fn gi_sh_basis(d: vec3<f32>) -> vec4<f32> {
    return vec4<f32>(0.282095, 0.488603 * d.y, 0.488603 * d.z, 0.488603 * d.x);
}

fn gi_half2(w: u32) -> vec2<f32> {
    return unpack2x16float(w);
}
