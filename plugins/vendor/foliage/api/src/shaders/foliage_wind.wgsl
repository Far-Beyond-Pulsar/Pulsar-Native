// Helio foliage wind — the three-band displacement model, shared.
//
// Prepended to any shader whose source contains `//!use helio_foliage_wind`.
// See helio_core::shader.
//
// This exists because grass blades, tree world-position-offset and impostor cards
// are three different rasterisation paths that draw the *same plant* at different
// distances, and two of them are on screen simultaneously inside every LOD
// cross-fade band. If those paths each grew their own `sin(time)` the silhouettes
// would shear against each other across the fade, which is exactly the artefact
// the stochastic cross-fade is supposed to hide. One implementation, included
// everywhere, is the only way the three stay in phase.
//
// ── THE `time` PARAMETER IS NOT REDUNDANT ───────────────────────────────────
//
// Every function here takes `time` explicitly instead of reading
// `wind.time_prev_time.x` itself. That looks like a parameter begging to be
// deleted. It is not.
//
// Callers evaluate the whole model for two times per vertex:
//
//     let w = helio_wind_offset_pair(wind, ..., t, t - dt);
//     out.clip_position      = camera.view_proj      * vec4(world + w.now,  1.0);
//     out.prev_clip_position = camera.prev_view_proj * vec4(world + w.prev, 1.0);
//
// The second evaluation is what fills the G-buffer velocity target for animated
// vegetation. Fold `time` back into the function and there is no way to ask for
// `t - dt`, every foliage vertex reports zero motion, and TAA reprojects a moving
// blade onto the history texel of whatever was behind it — i.e. every blade of
// grass in the frame smears. It also silently breaks the dithered LOD
// cross-fades, which only resolve when the motion vectors underneath them are
// right. `prev_time` is in the uniform for this and nothing else; see
// `libhelio::wind::GpuWind::time_prev_time`.
//
// ── No bindings ─────────────────────────────────────────────────────────────
//
// This module declares no bindings and samples no textures, for the same reason
// the prelude does not declare `camera`: group and binding are the caller's
// business, and a wind model that needed a bind group could not be called from a
// vertex stage that has already spent its groups on the scene database. The
// including shader declares its own:
//
//     //!use helio_foliage_wind
//     @group(0) @binding(3) var<uniform> wind: Wind;
//
// Consequently the noise below is arithmetic, not a texture fetch.

// ── Uniform layout ──────────────────────────────────────────────────────────
// Field-for-field mirror of `libhelio::wind::GpuWind` (48 bytes). The two must
// be edited together: a mismatch here does not error, it reinterprets the gust
// parameters as a direction and produces vegetation that either stands perfectly
// still or flies apart, with nothing to point at.
//
//   0..16  direction_speed  xyz = normalised direction, w = base speed (m/s)
//  16..32  gust             amplitude, frequency (Hz), phase (rad), turbulence (1/m)
//  32..40  time_prev_time   t, t - dt
//  40..48  _pad             reserved, never read
struct Wind {
    /// xyz normalised on the CPU, w = base speed in m/s.
    ///
    /// Never re-normalised here. `Wind::to_gpu` guarantees unit length and falls
    /// back to +X for a degenerate direction, so a non-unit vector arriving here
    /// is a CPU bug that would scale every amplitude in the model rather than
    /// failing visibly.
    direction_speed: vec4<f32>,
    /// `[amplitude, frequency, phase, turbulence_scale]`.
    gust: vec4<f32>,
    /// `[t, t - dt]`. Read by the *caller*, passed in as `time`; see the header.
    time_prev_time: vec2<f32>,
    _pad: vec2<f32>,
}

// ── Tuning ──────────────────────────────────────────────────────────────────
// All amplitudes are in metres per (m/s) of wind speed, so doubling `speed`
// doubles the displacement. All frequencies are in Hz and converted to angular
// frequency at the call site. Nothing here is per-foliage-type — per-type
// authoring goes through `response`, which is `GpuFoliageType::wind_response`.

const HELIO_WIND_TAU: f32 = 6.28318530718;

/// Speed at which the frequency ramp is fully applied, in m/s.
///
/// Real vegetation does not just swing further in stronger wind, it swings
/// *faster*. Without this the model reads as a slow-motion replay at high speeds.
const HELIO_WIND_REFERENCE_SPEED: f32 = 5.0;

/// Frequency multiplier for a given wind speed, applied to **all three bands**.
///
/// Real vegetation swings both further and faster in stronger wind. Applying the ramp to
/// sway alone — which is what this model used to do — means the leaf and branch bands run
/// at a fixed tempo no matter the weather, so `speed` reads purely as an amplitude
/// control and dead-calm grass shimmers at exactly the rate gale-force grass does. It also
/// makes the wind speed knob feel broken: turning it down makes the motion smaller but no
/// calmer.
///
/// Floors at 0.35 rather than 0 so grass in near-still air drifts instead of freezing
/// solid, which reads as a paused animation rather than a calm day.
fn helio_wind_tempo(speed: f32) -> f32 {
    return 0.35 + 0.65 * clamp(speed / HELIO_WIND_REFERENCE_SPEED, 0.0, 2.0);
}

/// Spatial frequency of the sway phase field, in 1/m — ~16 m coherence patches.
///
/// This number is the single most important one in the file. Raise it far and
/// neighbouring plants decorrelate into per-instance phase, which is precisely
/// the "boiling" look; drop it to zero and an entire meadow becomes one rigid
/// object hinging in unison.
const HELIO_WIND_SWAY_COHERENCE: f32 = 0.06;

const HELIO_WIND_SWAY_HZ: f32 = 0.26;
const HELIO_WIND_SWAY_METRES_PER_MPS: f32 = 0.030;
/// Lateral sway as a fraction of downwind sway. A stem that only moves in the
/// wind plane reads as a hinge, not a plant.
const HELIO_WIND_SWAY_LATERAL: f32 = 0.35;

/// How fast the gust noise field is advected downwind, as a fraction of `speed`.
///
/// Non-zero is what makes a gust *front cross a field* instead of the whole
/// field breathing together. Zero here would leave a stationary blotch pattern
/// pulsing in place, which looks worse than no turbulence at all.
const HELIO_WIND_GUST_ADVECTION: f32 = 0.35;

const HELIO_WIND_FLUTTER_HZ: f32 = 1.05;
const HELIO_WIND_FLUTTER_METRES_PER_MPS: f32 = 0.008;
/// Flutter phase accumulated per metre travelled downwind (rad/m).
const HELIO_WIND_FLUTTER_DOWNWIND: f32 = 0.9;
/// Flutter phase accumulated per metre along the stem (rad/m).
const HELIO_WIND_FLUTTER_ALONG_STEM: f32 = 5.5;

const HELIO_WIND_JITTER_HZ: f32 = 3.2;
const HELIO_WIND_JITTER_METRES_PER_MPS: f32 = 0.0025;

// ── Deterministic hashing ───────────────────────────────────────────────────
// Integer bit-mixing, not `fract(sin(x) * 43758.5453)`. The sine trick is the
// usual shader-hash shortcut and it is wrong for this use: `sin` is only
// guaranteed to a few ULP and vendors disagree on large arguments, so the same
// blade hashes differently on two GPUs — and worse, the blade geometry path and
// the impostor path can disagree *on the same GPU* if the compiler contracts the
// multiply differently in each. Wind phase must be bit-stable, because the
// cross-fade blends two representations of one plant and any phase difference
// between them shows up as a shearing silhouette.

/// Bit-mixer (the "lowbias32" finaliser). Avalanches well enough that adjacent
/// seeds produce unrelated phases, which is the entire requirement.
fn helio_wind_hash_u32(x: u32) -> u32 {
    var h = x;
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return h;
}

/// Hash to [0, 1).
fn helio_wind_hash_unorm(x: u32) -> f32 {
    return f32(helio_wind_hash_u32(x)) * (1.0 / 4294967296.0);
}

/// Hash one integer lattice cell to [0, 1).
///
/// The coordinates are *bitcast*, not converted. Foliage tiles have negative
/// world coordinates as a matter of course, and converting a negative `i32` to
/// `u32` in WGSL yields an indeterminate value — which would make the noise
/// field differ across the world origin depending on the backend.
fn helio_wind_lattice(cell: vec2<i32>) -> f32 {
    let c = bitcast<vec2<u32>>(cell);
    return helio_wind_hash_unorm((c.x * 73856093u) ^ (c.y * 19349663u));
}

/// Smooth value noise over the XZ plane, in [-1, 1].
///
/// Two dimensions, not three, deliberately: a wind field varies horizontally,
/// and adding a Y axis would only decorrelate two plants standing at different
/// altitudes — while doubling the lattice taps from four to eight in a function
/// that runs twice per vertex (once at `t`, once at `t - dt`).
///
/// Value noise rather than a hash of the floored cell because the phase field
/// must be *continuous*: a hard phase step at a cell boundary draws a visible
/// seam straight across a meadow, with the plants either side of it swaying
/// against each other.
fn helio_wind_noise(p: vec2<f32>) -> f32 {
    let base = floor(p);
    let f = p - base;
    // Hermite fade, so the field is C1 across cell boundaries. Plain linear
    // interpolation is C0 and its derivative discontinuity is visible as a
    // faint grid in the sway pattern.
    let w = f * f * (3.0 - 2.0 * f);

    let c = vec2<i32>(base);
    let n00 = helio_wind_lattice(c);
    let n10 = helio_wind_lattice(c + vec2<i32>(1, 0));
    let n01 = helio_wind_lattice(c + vec2<i32>(0, 1));
    let n11 = helio_wind_lattice(c + vec2<i32>(1, 1));

    return mix(mix(n00, n10, w.x), mix(n01, n11, w.x), w.y) * 2.0 - 1.0;
}

// ── Frames ──────────────────────────────────────────────────────────────────

/// A unit axis perpendicular to the wind, in the horizontal plane.
///
/// Falls back to +X when the wind points straight up: `cross` degenerates there
/// and normalising it would put a NaN into the vertex position, which discards
/// the primitive. "All vegetation vanished, no shader error" is the worst
/// failure mode available, so it is guarded rather than assumed away.
fn helio_wind_side_axis(dir: vec3<f32>) -> vec3<f32> {
    let side = cross(dir, vec3<f32>(0.0, 1.0, 0.0));
    let len2 = dot(side, side);
    return select(
        vec3<f32>(1.0, 0.0, 0.0),
        side * inverseSqrt(max(len2, 1e-12)),
        len2 > 1e-8,
    );
}

// ── Band 1: gust envelope ───────────────────────────────────────────────────

/// Gust multiplier at a world position, >= 1.0 at zero gust amplitude.
///
/// Two terms: a slow global "breath" from `gust.yz`, and a turbulence field
/// sampled at a point advected downwind by `speed * time`. The advection is the
/// point of the whole function — a stationary turbulence field makes every plant
/// in a blotch pulse together forever, whereas an advected one produces a gust
/// front that visibly travels across the field, which is what the eye reads as
/// wind rather than as an animation.
///
/// `gust.w == 0` collapses the turbulence sample to a constant, which is a valid
/// "globally uniform gust" authoring mode, not a degenerate case.
fn helio_wind_gust(wind: Wind, world_pos: vec3<f32>, time: f32) -> f32 {
    let amplitude = wind.gust.x;
    let frequency = wind.gust.y;
    let phase = wind.gust.z;
    let turbulence = wind.gust.w;

    let dir = wind.direction_speed.xyz;
    let speed = wind.direction_speed.w;
    let advected = world_pos - dir * (speed * time * HELIO_WIND_GUST_ADVECTION);

    let front = helio_wind_noise(advected.xz * turbulence);
    let breath = sin(HELIO_WIND_TAU * frequency * time + phase);

    // Weighted so the global breath leads and turbulence decorrelates it, then
    // remapped to [0, 1]: a gust may only ever *add* to the base wind, never
    // subtract, or a strong gust setting would periodically freeze the scene.
    let envelope = 0.5 + 0.5 * (breath * 0.6 + front * 0.4);
    return 1.0 + amplitude * envelope;
}

// ── Bands 2-4 (sway, flutter, jitter), composed ────────────────────────────

/// The wind's offset of a point (`world_pos`, `height_frac` up its plant rooted at
/// `instance_origin`, of `seed`) now (`time_now`) and a frame ago (`time_prev`), the pair
/// a vertex needs for its motion vector: what does not depend on time (the sway's phase,
/// the flutter's along the stem, the jitter's per plant) is worked out once for both.
/// Three bands, scaled by `response` (sway, flutter, jitter): the whole plant swaying
/// with gusts, tips more; a ripple along the stem; a per-plant shiver.
struct HelioWindPair {
    now: vec3<f32>,
    prev: vec3<f32>,
}

fn helio_wind_offset_pair(
    wind: Wind,
    world_pos: vec3<f32>,
    instance_origin: vec3<f32>,
    height_frac: f32,
    seed: u32,
    response: vec3<f32>,
    time_now: f32,
    time_prev: f32,
) -> HelioWindPair {
    var out = HelioWindPair(vec3<f32>(0.0), vec3<f32>(0.0));
    let dir = wind.direction_speed.xyz;
    let speed = wind.direction_speed.w;
    if speed <= 0.0 {
        return out;
    }
    // Clamped rather than trusted: `height_frac` usually arrives from a packed u8 or a
    // divide by a per-type height, and an out-of-range value would be squared into a
    // large sway amplitude and throw the vertex across the scene.
    let h = clamp(height_frac, 0.0, 1.0);
    let side = helio_wind_side_axis(dir);
    let tempo = helio_wind_tempo(speed);
    let times = vec2<f32>(time_now, time_prev);

    // Sway: a phase from the plant's place (~16 m coherence patches, so neighbours move
    // together and a meadow neither boils nor hinges as one), a fundamental plus an
    // incommensurate second mode (2.17, not 2: no one-second loop), some sideways, the
    // gusts at each time.
    if response.x > 0.0 {
        let phase = helio_wind_noise(instance_origin.xz * HELIO_WIND_SWAY_COHERENCE) * HELIO_WIND_TAU;
        let amplitude = response.x * h * h * speed * HELIO_WIND_SWAY_METRES_PER_MPS;
        for (var i = 0u; i < 2u; i++) {
            let t = HELIO_WIND_TAU * HELIO_WIND_SWAY_HZ * tempo * times[i] + phase;
            let bend = sin(t) * 0.75 + sin(t * 2.17 + 1.3) * 0.25;
            let sideways = sin(t * 0.63 + phase * 1.7) * HELIO_WIND_SWAY_LATERAL;
            let sway = (dir * bend + side * sideways) * amplitude * helio_wind_gust(wind, instance_origin, times[i]);
            if i == 0u { out.now += sway; } else { out.prev += sway; }
        }
    }
    // Flutter: a ripple travelling downwind and up the stem.
    let stem_len = length(world_pos - instance_origin);
    if response.y > 0.0 {
        let phase = dot(instance_origin, dir) * HELIO_WIND_FLUTTER_DOWNWIND + stem_len * HELIO_WIND_FLUTTER_ALONG_STEM;
        let amplitude = response.y * h * speed * HELIO_WIND_FLUTTER_METRES_PER_MPS;
        for (var i = 0u; i < 2u; i++) {
            let t = HELIO_WIND_TAU * HELIO_WIND_FLUTTER_HZ * tempo * times[i] + phase;
            let flutter = (side * sin(t) + vec3<f32>(0.0, 1.0, 0.0) * (cos(t * 1.37) * 0.35)) * amplitude;
            if i == 0u { out.now += flutter; } else { out.prev += flutter; }
        }
    }
    // Jitter: three decorrelated phases from the plant's seed, so the axes do not share
    // a phase and collapse the shiver onto a diagonal.
    if response.z > 0.0 {
        let px = helio_wind_hash_unorm(seed) * HELIO_WIND_TAU;
        let py = helio_wind_hash_unorm(seed ^ 0x9e3779b9u) * HELIO_WIND_TAU;
        let pz = helio_wind_hash_unorm(seed ^ 0x85ebca6bu) * HELIO_WIND_TAU;
        let amplitude = response.z * h * speed * HELIO_WIND_JITTER_METRES_PER_MPS;
        for (var i = 0u; i < 2u; i++) {
            let w = HELIO_WIND_TAU * HELIO_WIND_JITTER_HZ * tempo * times[i];
            let jitter = vec3<f32>(sin(w + px), sin(w * 0.83 + py) * 0.5, sin(w * 1.19 + pz)) * amplitude;
            if i == 0u { out.now += jitter; } else { out.prev += jitter; }
        }
    }

    // Arc-length correction. The bands displace horizontally without shortening the
    // stem, so a strongly bent blade would be longer than it is and its tip would climb
    // as it leans; dropping the tip by the arc's sagitta keeps its distance from the root
    // (clamped: a displacement past the stem's length is fully flattened).
    if stem_len > 1e-4 {
        let reach_now = min(length(out.now.xz), stem_len);
        let reach_prev = min(length(out.prev.xz), stem_len);
        out.now.y -= stem_len - sqrt(max(stem_len * stem_len - reach_now * reach_now, 0.0));
        out.prev.y -= stem_len - sqrt(max(stem_len * stem_len - reach_prev * reach_prev, 0.0));
    }
    return out;
}
