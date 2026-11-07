// The frame's interactors (`FoliageInteractorComponent` rows: feet, wheels, thrown
// things), gathered for the foliage draw: SceneDB keeps a row at its entity's index, so
// the live ones (a radius over 0) are scattered through the column. Each is appended to
// a small table in render space (its world position in f32 halves less the frame's
// origin), at most `FOLIAGE_MAX_INTERACTORS`, after the count was cleared this frame.

const FOLIAGE_MAX_INTERACTORS: u32 = 16u;

struct Gather {
    /// The frame's world origin in f32 halves (`hi + lo`), and the source's rows.
    origin_hi: vec4<f32>,
    origin_lo: vec4<f32>,
    rows: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

/// `FoliageInteractorComponent`.
struct Row {
    hi_radius: vec4<f32>,
    lo_strength: vec4<f32>,
    velocity_wake: vec4<f32>,
}

/// One gathered interactor: render-space position and radius, velocity and wake, and
/// strength (x).
struct Interactor {
    at_radius: vec4<f32>,
    velocity_wake: vec4<f32>,
    strength: vec4<f32>,
}

struct Interactors {
    count: atomic<u32>,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    rows: array<Interactor, FOLIAGE_MAX_INTERACTORS>,
}

@group(0) @binding(0) var<uniform> gather: Gather;
@group(0) @binding(1) var<storage, read> source: array<Row>;
@group(0) @binding(2) var<storage, read_write> live: Interactors;

@compute @workgroup_size(64)
fn cs_gather(@builtin(global_invocation_id) id: vec3<u32>) {
    if id.x >= gather.rows || id.x >= arrayLength(&source) {
        return;
    }
    let row = source[id.x];
    if !(row.hi_radius.w > 0.0) {
        return;
    }
    let slot = atomicAdd(&live.count, 1u);
    if slot >= FOLIAGE_MAX_INTERACTORS {
        return;
    }
    let at = (row.hi_radius.xyz - gather.origin_hi.xyz) + (row.lo_strength.xyz - gather.origin_lo.xyz);
    live.rows[slot] = Interactor(vec4<f32>(at, row.hi_radius.w), row.velocity_wake, vec4<f32>(max(row.lo_strength.w, 0.0), 0.0, 0.0, 0.0));
}
