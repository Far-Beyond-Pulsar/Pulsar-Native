//!use atmosphere
// The planet's weather (`clouds` module docs): a cube map over the
// directions from the planet's centre, the large scales of the clouds'
// cover, which `clouds_common.wgsl` refines with the mesoscale cells, the
// cumulus groups and the billows.
//
// - r: the weather's anomaly, about -1..1: how much cloudier (positive) or
//   clearer than the layer's mean coverage the region is.
// - g: the regime, 0..1 about 0.5: stratiform (decks, fronts, polar stratus)
//   below, convective (cumulus fields, the tropical convergence) above.
// - b: the cirrus the weather brings, 0..1.
// - a: how open the mesoscale cells are: 0 closed (cloudy cells parted by
//   clear lanes, as stratocumulus decks), 1 open (clear cells ringed by
//   cumulus, as the cold air behind fronts).
//
// Three scales make it, after Earth's (the latitudes of each set by the
// number of circulation cells, the sizes by the synoptic scale):
// - the circulation: bands of rising (cloudy) and sinking (clear) air by
//   latitude, the tropical convergence and the storm tracks rising, the
//   subtropics sinking;
// - the synoptic systems: cyclones (spiralling the cover round them, with
//   fronts trailing from them and open cells in the cold air behind) and
//   highs (clearing it), scattered over the sphere, the cyclones in the
//   storm tracks and rarely in the tropics, each living a few days;
// - the sub-synoptic variety of the cover.
// Every scale is noise over the sphere itself (never a tile), so nothing
// repeats. The weather drifts with the westerlies outside the tropics and
// the trades inside them, and its systems grow and decay, over a period of
// WEATHER_PERIOD_S (`frame.cloud_groups_offset.w` is its phase), so the
// clock never loses precision.
//
// Planets too small to hold a weather system (a few synoptic scales round)
// have none: their weather fades to the layer's mean.

struct WeatherBake {
    // The rows of each face this dispatch writes: those whose index is
    // `phase` modulo `stride` (1: all of them).
    phase: u32,
    stride: u32,
    _pad: vec2<u32>,
}

@group(0) @binding(0) var<storage, read> frame: AtmosphereFrame;
@group(0) @binding(1) var<uniform> bake: WeatherBake;
@group(0) @binding(2) var weather_out: texture_storage_2d_array<rgba16float, write>;

const PI: f32 = 3.14159265;
// Systems per cell of the lattice they are scattered on, its side in
// synoptic scales.
const SYSTEM_SPACING: f32 = 1.6;
// An orthonormal turn between octaves.
const OCTAVE_TURN: mat3x3<f32> = mat3x3<f32>(0.0, 0.8, 0.6, -0.8, 0.36, -0.48, -0.6, -0.48, 0.64);

fn hash4(cell: vec3<i32>, seed: u32) -> vec4<f32> {
    var v = vec4<u32>(bitcast<vec3<u32>>(cell), seed) * 1664525u + 1013904223u;
    v.x += v.y * v.w;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v.w += v.y * v.z;
    v ^= v >> vec4<u32>(16u);
    v.x += v.y * v.w;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v.w += v.y * v.z;
    return vec4<f32>(v >> vec4<u32>(8u)) / 16777216.0;
}

// Gradient noise, about -1..1.
fn gradient_noise(p: vec3<f32>, seed: u32) -> f32 {
    let i = vec3<i32>(floor(p));
    let f = p - floor(p);
    let u = f * f * f * (f * (f * 6.0 - 15.0) + 10.0);
    var c: array<f32, 8>;
    for (var k = 0; k < 8; k++) {
        let o = vec3<i32>(k & 1, (k >> 1) & 1, (k >> 2) & 1);
        let g = normalize(hash4(i + o, seed).xyz * 2.0 - 1.0 + vec3<f32>(1e-4));
        c[k] = dot(g, f - vec3<f32>(o));
    }
    return mix(mix(mix(c[0], c[1], u.x), mix(c[2], c[3], u.x), u.y),
               mix(mix(c[4], c[5], u.x), mix(c[6], c[7], u.x), u.y), u.z) * 1.6;
}

// Fractal noise, `octaves` from wavelength 1, about -1..1.
fn fbm(p: vec3<f32>, octaves: i32, seed: u32) -> f32 {
    var sum = 0.0;
    var amplitude = 0.5;
    var total = 0.0;
    var q = p;
    for (var o = 0; o < octaves; o++) {
        sum += gradient_noise(q, seed + u32(o) * 101u) * amplitude;
        total += amplitude;
        amplitude *= 0.5;
        // Twice the frequency, turned so the octaves' lattices never line
        // up.
        q = OCTAVE_TURN * q * 2.0;
    }
    return sum / total;
}

// `v` turned by `angle` about unit `axis` (Rodrigues).
fn rotate(v: vec3<f32>, axis: vec3<f32>, angle: f32) -> vec3<f32> {
    let s = sin(angle);
    let c = cos(angle);
    return v * c + cross(axis, v) * s + axis * (dot(axis, v) * (1.0 - c));
}

// The direction of texel `id` of cube face `face` (WebGPU's faces: +X -X
// +Y -Y +Z -Z).
fn cube_direction(face: u32, id: vec2<u32>, size: u32) -> vec3<f32> {
    let uv = (vec2<f32>(id) + 0.5) / f32(size) * 2.0 - 1.0;
    var d: vec3<f32>;
    switch face {
        case 0u: { d = vec3<f32>(1.0, -uv.y, -uv.x); }
        case 1u: { d = vec3<f32>(-1.0, -uv.y, uv.x); }
        case 2u: { d = vec3<f32>(uv.x, 1.0, uv.y); }
        case 3u: { d = vec3<f32>(uv.x, -1.0, -uv.y); }
        case 4u: { d = vec3<f32>(uv.x, -uv.y, 1.0); }
        default: { d = vec3<f32>(-uv.x, -uv.y, -1.0); }
    }
    return normalize(d);
}

struct Weather {
    // The planet's parameters as the weather reads them.
    radius: f32,
    pole: vec3<f32>,
    // Latitude of the circulation's cells' boundaries: 90 degrees / cells.
    cell_latitude: f32,
    cells: f32,
    synoptic: f32,
    circulation: f32,
    storminess: f32,
    seed: u32,
    phase: f32,
}

// Rising (1) or sinking (-1) air at latitude `lat` (radians): rising at the
// equator and at every second boundary between cells.
fn circulation(w: Weather, lat: f32) -> f32 {
    return cos(2.0 * w.cells * lat);
}

struct Systems {
    // The point after the systems' spirals (unit direction).
    warped: vec3<f32>,
    // Pressure: negative in lows, positive in highs.
    pressure: f32,
    // How near a cyclone is, and how much of that is its cold, western side.
    cyclone: f32,
    cold: f32,
    // Tropical cyclones' cores: 1 in their eye.
    eye: f32,
}

// The synoptic systems over unit direction `n` (in the frame the weather
// drifts with).
fn systems(w: Weather, n: vec3<f32>) -> Systems {
    var out: Systems;
    out.warped = n;
    out.pressure = 0.0;
    out.cyclone = 0.0;
    out.cold = 0.0;
    out.eye = 0.0;
    let cell_km = w.synoptic * SYSTEM_SPACING;
    let shell = w.radius / cell_km;
    let p = n * shell;
    let base = vec3<i32>(floor(p));
    let east = normalize(cross(w.pole, n) + vec3<f32>(1e-6, 0.0, 0.0));
    for (var k = 0; k < 27; k++) {
        let cell = base + vec3<i32>(k % 3 - 1, (k / 3) % 3 - 1, k / 9 - 1);
        let r0 = hash4(cell, w.seed * 7919u + 3u);
        let centre = vec3<f32>(cell) + r0.xyz;
        // Only the lattice's points near the sphere: about evenly spread
        // over it.
        if abs(length(centre) - shell) > 0.6 { continue; }
        let at = normalize(centre);
        let distance = w.radius * acos(clamp(dot(n, at), -1.0, 1.0));
        let r1 = hash4(cell, w.seed * 7919u + 11u);
        let lat = asin(clamp(dot(at, w.pole), -1.0, 1.0));
        let rising = circulation(w, lat);
        let tropical = abs(lat) < w.cell_latitude * 0.85;
        // Lows where the air rises (the storm tracks), highs where it sinks;
        // in the tropics lows are rare.
        let low_chance = select(0.5 + 0.35 * rising, 0.12 * w.storminess, tropical);
        let low = r0.w < low_chance;
        // Each system grows and decays: lows over 2-5 days, highs over
        // longer.
        let cycles = select(floor(3.0 + r1.x * 4.0), floor(6.0 + r1.x * 8.0), low);
        let life = sin(PI * fract(cycles * w.phase + r1.y));
        var strength = life * life * (0.55 + 0.45 * r1.z);
        var radius = cell_km * (0.35 + 0.25 * r1.w);
        // Coriolis spins lows anticlockwise in the north, clockwise in the
        // south, highs the other way; none at the equator.
        let hemisphere = clamp(sin(lat) * 4.0, -1.0, 1.0);
        var spin: f32;
        if low {
            strength *= w.storminess;
            spin = 5.0 * hemisphere;
            if tropical {
                // A tropical cyclone: small, tight and fierce, round a
                // clear eye.
                radius *= 0.35;
                spin *= 1.6;
                strength = min(strength * 1.8, 1.0);
            }
        } else {
            strength *= max(w.storminess, w.circulation);
            spin = -1.5 * hemisphere;
        }
        let x = distance / radius;
        let core = exp(-x * x);
        if core > 1e-3 {
            out.warped = rotate(out.warped, at, spin * strength * core);
        }
        let reach = exp(-x * x * 0.5);
        if low {
            out.pressure -= strength * reach;
            let near = strength * exp(-x * x * 0.25);
            out.cyclone += near;
            // Behind the cold front: west of the low and towards the
            // equator from it.
            let away = n - at;
            let west = -dot(away, east) * shell;
            let equatorward = -dot(away, w.pole) * sign(dot(at, w.pole)) * shell;
            out.cold += near * smoothstep(0.0, 0.6, west + 0.5 * equatorward);
            if tropical {
                out.eye = max(out.eye, strength * exp(-x * x * 60.0));
            }
        } else {
            out.pressure += strength * reach * 0.8;
        }
    }
    return out;
}

// The weather (see the header) at unit direction `n` in the frame the
// weather drifts with; `tropics` weighs the tropical regimes. The
// circulation's terms scale with `w.circulation`, the systems' with
// `w.storminess`.
fn weather_at(w: Weather, n: vec3<f32>, tropics: f32) -> vec4<f32> {
    let s = systems(w, n);
    let shell = w.radius / w.synoptic;
    let lat = asin(clamp(dot(n, w.pole), -1.0, 1.0));
    // The circulation's boundaries meander (the convergence zone, the
    // jets).
    let meander = fbm(n * (shell * 0.4), 2, w.seed + 501u) * w.cell_latitude * 0.15;
    let band = circulation(w, lat + meander) * w.circulation;
    // Polar air sinks, but its cold stratus keeps it grey.
    let polar = smoothstep(w.cell_latitude * (w.cells - 1.0), w.cell_latitude * w.cells, abs(lat)) * w.circulation;
    // Fronts: long ridges of the spiralled noise, near cyclones.
    let ridge = 1.0 - abs(gradient_noise(s.warped * (shell * 1.4), w.seed + 601u));
    let fronts = ridge * ridge * ridge * ridge * clamp(s.cyclone * 1.5, 0.0, 1.0);
    let synoptic = fbm(s.warped * shell, 4, w.seed + 701u);
    let regional = fbm(n * (shell * 3.0), 3, w.seed + 801u);
    let local = fbm(n * (shell * 8.0), 2, w.seed + 851u);
    // Stratocumulus decks under the subtropical highs' sinking air, in
    // patches, and polar stratus.
    let decks = smoothstep(0.05, 0.35, fbm(n * (shell * 0.7), 3, w.seed + 901u))
        * clamp(-band, 0.0, 1.0) * (1.0 - tropics) + polar * 0.6;
    let convergence = clamp(band, 0.0, 1.0) * tropics;
    // The weather's own variety: some even where no system or band is.
    let variety = (0.45 * synoptic + 0.45 * regional + 0.3 * local) * max(w.storminess, 0.5 * w.circulation);
    var anomaly = 0.55 * band * (1.0 - polar * 0.5) + 0.6 * decks
        - 0.9 * s.pressure + 1.1 * fronts + variety - 3.0 * s.eye;
    let stratiform = clamp(max(fronts * 1.6 + clamp(-s.pressure, 0.0, 1.0) * 0.6, decks), 0.0, 1.0);
    let convective = clamp(convergence + s.cold * 0.8 + clamp(variety, 0.0, 1.0) * 0.3, 0.0, 1.0);
    let regime = clamp(0.5 + 0.5 * convective - 0.5 * stratiform, 0.0, 1.0);
    let jets = 1.0 - abs(gradient_noise(s.warped * (shell * 2.5), w.seed + 1001u));
    let cirrus = clamp(fronts * 1.3 + s.cyclone * 0.35 + convergence * 0.8
        + jets * jets * jets * 0.35 * max(w.storminess, w.circulation), 0.0, 1.0);
    // Neutral (0.5: neither) without a reason for either.
    let open = clamp(0.5 + 0.7 * s.cold + 0.4 * convergence - 0.7 * decks + 0.3 * regional * w.storminess, 0.0, 1.0);
    return vec4<f32>(anomaly, regime, cirrus, open);
}

@compute @workgroup_size(8, 8, 1)
fn planet_weather(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(weather_out).x;
    let row = id.y * bake.stride + bake.phase;
    if id.x >= size || row >= size || id.z >= 6u { return; }
    if !atmosphere_has_clouds(frame) { return; }
    let p = frame.params;
    // While the weather holds still, rows are written only when the map is
    // rebuilt whole.
    if bake.stride > 1u && p.cloud_evolution <= 0.0 { return; }
    var w: Weather;
    w.radius = p.bottom_radius;
    let pole = p.cloud_pole;
    w.pole = select(vec3<f32>(0.0, 1.0, 0.0), normalize(pole), dot(pole, pole) > 1e-8);
    w.cells = max(round(p.cloud_circulation_cells), 1.0);
    w.cell_latitude = 0.5 * PI / w.cells;
    w.synoptic = max(p.cloud_synoptic_scale, 1.0);
    // None on planets too small to hold a few systems.
    let room = smoothstep(1.0, 4.0, w.radius / w.synoptic);
    w.circulation = clamp(p.cloud_circulation, 0.0, 1.0) * room;
    w.storminess = clamp(p.cloud_storminess, 0.0, 1.0) * room;
    w.seed = p.cloud_seed;
    w.phase = frame.cloud_groups_offset.w;
    let n = cube_direction(id.z, vec2<u32>(id.x, row), size);
    let lat = asin(clamp(dot(n, w.pole), -1.0, 1.0));
    let tropics = 1.0 - smoothstep(0.7, 1.1, abs(lat) / w.cell_latitude);
    // Outside the tropics the westerlies carry the weather east, inside
    // them the trades west: once round the planet each period.
    let turn = 2.0 * PI * w.phase;
    var weather = weather_at(w, rotate(n, w.pole, -turn), 0.0);
    if tropics > 0.0 {
        weather = mix(weather, weather_at(w, rotate(n, w.pole, turn), tropics), tropics);
    }
    textureStore(weather_out, vec2<u32>(id.x, row), id.z, weather);
}
